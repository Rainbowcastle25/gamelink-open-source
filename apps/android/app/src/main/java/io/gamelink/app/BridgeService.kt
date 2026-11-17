package io.gamelink.app

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONObject

data class BridgeView(val phase: String = "idle", val message: String = "Bridge stopped",
                      val sessions: Int = 0, val address: String = "",
                      val path: String = "unknown", val stage: String = "stopped")

/** Foreground service is the sole runtime owner. It survives activity closure;
 * process death is fail-closed and requires another explicit user start.
 * All reducer events/effects run on Main. Blocking Go operations run on IO. */
class BridgeService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var state = JSONObject().put("phase", "idle").put("generation", 0)
    private var runtime: mobile.Runtime? = null
    private var runtimeGeneration = -1L
    private var poll: Job? = null
    private var network: WifiNetwork? = null
    private var watcher: WifiNetworks? = null
    private var wake: PowerManager.WakeLock? = null
    private var multicast: WifiManager.MulticastLock? = null
    private var wifi: WifiManager.WifiLock? = null
    private var destroying = false

    override fun onCreate() {
        super.onCreate()
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, "Console bridge", NotificationManager.IMPORTANCE_LOW))
        watcher = WifiNetworks(this) { networks -> scope.launch {
            val selected = network ?: return@launch
            if (networks.none { it.id == selected.id && it.address == selected.address }) {
                reduce(JSONObject().put("type", "network_lost").put("generation", state.getLong("generation")))
                stopSelf()
            }
        } }.also { it.start() }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == STOP) {
            scope.launch { reduce(JSONObject().put("type", "stop")); stopSelf() }
            return START_NOT_STICKY
        }
        // A second tap must not replace network/startup ownership while the
        // first start is suspended reading encrypted credentials.
        if (network != null && state.getString("phase") !in listOf("idle", "error")) return START_NOT_STICKY
        // Called only from the visible activity. Foreground promptly, before IO.
        if (Build.VERSION.SDK_INT >= 29) startForeground(NOTIFICATION, notification("Starting console bridge"),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        else startForeground(NOTIFICATION, notification("Starting console bridge"))
        val id = intent?.getStringExtra("serverId")
        val handle = intent?.getStringExtra("networkId")
        if (id == null || handle == null ||
            (Build.VERSION.SDK_INT >= 37 && checkSelfPermission(Manifest.permission.ACCESS_LOCAL_NETWORK) != PackageManager.PERMISSION_GRANTED)) {
            mutableView.value = BridgeView("error", "Grant local network access and start the bridge from GameLink.")
            stopSelf()
            return START_NOT_STICKY
        }
        scope.launch {
            try {
                val cm = getSystemService(ConnectivityManager::class.java)
                val n = cm.allNetworks.firstOrNull { it.networkHandle.toString() == handle }
                    ?: error("network")
                val props = cm.getLinkProperties(n) ?: error("network")
                val addr = props.linkAddresses.firstOrNull {
                    it.address is java.net.Inet4Address && it.address.isSiteLocalAddress
                } ?: error("network")
                // Go sockets use native routing; require selected Wi-Fi to be
                // Android's current default, and bind LAN listeners explicitly.
                require(n == cm.activeNetwork)
                network = WifiNetwork(n, addr.address.hostAddress!!, addr.prefixLength, props.interfaceName ?: "Wi-Fi")
                val server = withContext(Dispatchers.IO) { Repository.get(this@BridgeService).servers().first { it.id == id } }
                reduce(JSONObject().put("type", "start").put("server", server.metadata())
                    .put("localIpv4", network!!.address))
                if (state.getString("phase") == "error") stopSelf()
            } catch (_: Exception) {
                mutableView.value = BridgeView("error", "Cannot start. Check storage, invitation, and the selected Wi-Fi network.")
                stopSelf()
            }
        }
        return START_NOT_STICKY
    }

    private suspend fun reduce(event: JSONObject) {
        val result = Core.reduce(state, event)
        state = result.getJSONObject("state")
        val effects = result.getJSONArray("effects")
        for (i in 0 until effects.length()) {
            val e = effects.getJSONObject(i)
            when (e.getString("type")) {
                "stop_transport" -> stopRuntime(e.getLong("generation"))
                "start_transport" -> startRuntime(e)
            }
        }
        publish()
    }

    private suspend fun startRuntime(effect: JSONObject) {
        val generation = effect.getLong("generation")
        val id = effect.getString("server_id")
        val address = effect.getString("local_ipv4")
        try {
            acquireLocks()
            val credentials = withContext(Dispatchers.IO) {
                val repo = Repository.get(this@BridgeService)
                repo.invitation(id) to repo.privateKey()
            }
            // Start validates and returns before any network work. Create and
            // retain the handle on Main, with no cancellable suspension between
            // the allocation and assignment (which could orphan a Go runtime).
            val created = try {
                if (destroying || state.getLong("generation") != generation) return
                val selected = network ?: error("network")
                val index = java.net.NetworkInterface.getByInetAddress(java.net.InetAddress.getByName(address)).index
                GoTransport.start(credentials.first, address, selected.prefix, index, credentials.second)
            } finally { credentials.second.fill(0) }
            runtime = created
            runtimeGeneration = generation
            poll = scope.launch {
                var lastStatus = ""
                var lockRenewed = android.os.SystemClock.elapsedRealtime()
                while (isActive && runtime === created) {
                    val s = withContext(Dispatchers.IO) { JSONObject(created.status()) }
                    val status = s.getString("state")
                    mutableView.value = mutableView.value.copy(sessions = s.optInt("sessions"),
                        path = s.optString("path", "unknown"), stage = s.optString("stage", "connecting"))
                    if (status != lastStatus) {
                        lastStatus = status
                        if (status == "error" || status == "stopped") {
                            reduce(JSONObject().put("type", "error").put("generation", generation)
                                .put("code", s.optString("code", "transport_failed")))
                            stopSelf()
                            break
                        }
                        reduce(JSONObject().put("type", "transport_status").put("generation", generation).put("status", status))
                    }
                    if (android.os.SystemClock.elapsedRealtime() - lockRenewed > 10 * 60 * 1000) {
                        wake?.acquire(15 * 60 * 1000L)
                        lockRenewed = android.os.SystemClock.elapsedRealtime()
                    }
                    delay(if (status == "connected") 5000 else 1000)
                }
            }
        } catch (_: Exception) {
            releaseLocks()
            reduce(JSONObject().put("type", "error").put("generation", generation).put("code", "transport_failed"))
            stopSelf()
        }
    }

    private suspend fun stopRuntime(generation: Long) {
        if (runtimeGeneration != generation && runtime != null) return
        val currentPoll = poll
        poll = null
        // A terminal event may come from the polling job itself.
        if (currentPoll != currentCoroutineContext()[Job]) currentPoll?.cancel()
        val old = runtime
        runtime = null
        runtimeGeneration = -1
        if (old != null) withContext(Dispatchers.IO) { old.stop() }
        releaseLocks()
    }

    private fun publish() {
        val phase = state.getString("phase")
        val message = state.optString("message").takeUnless { it == "null" || it.isBlank() } ?: "Bridge stopped"
        mutableView.value = BridgeView(phase, message, if (runtime != null) mutableView.value.sessions else 0,
            if (runtime != null) network?.address ?: "" else "",
            if (runtime != null) mutableView.value.path else "unknown",
            if (runtime != null) mutableView.value.stage else "stopped")
        if (!destroying) getSystemService(NotificationManager::class.java).notify(NOTIFICATION, notification(message))
    }

    @Suppress("DEPRECATION") private fun acquireLocks() {
        if (wake != null) return
        wake = getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK,
            "GameLink:console-bridge").apply { setReferenceCounted(false); acquire(15 * 60 * 1000L) }
        val manager = applicationContext.getSystemService(WifiManager::class.java)
        multicast = manager.createMulticastLock("GameLink:discovery").apply { setReferenceCounted(false); acquire() }
        wifi = manager.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "GameLink:bridge")
            .apply { setReferenceCounted(false); acquire() }
    }
    private fun releaseLocks() {
        wake?.let { if (it.isHeld) it.release() }; wake = null
        multicast?.let { if (it.isHeld) it.release() }; multicast = null
        wifi?.let { if (it.isHeld) it.release() }; wifi = null
    }

    private fun notification(message: String): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        val stop = PendingIntent.getService(this, 1, Intent(this, BridgeService::class.java).setAction(STOP), PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, CHANNEL).setSmallIcon(android.R.drawable.stat_sys_upload_done)
            .setContentTitle("GameLink Console Bridge").setContentText(message).setContentIntent(open)
            .setOngoing(true).setCategory(Notification.CATEGORY_SERVICE)
            .addAction(Notification.Action.Builder(null, "Stop", stop).build()).build()
    }
    override fun onBind(intent: Intent?): IBinder? = null
    override fun onDestroy() {
        destroying = true
        watcher?.close()
        watcher = null
        // Go Stop is synchronous and bounded. Teardown survives cancellation of
        // the service's main scope. No new startup can publish after this flag.
        val old = runtime
        runtime = null
        scope.cancel()
        if (old != null) CoroutineScope(Dispatchers.IO).launch { old.stop() }
        releaseLocks()
        if (mutableView.value.phase !in listOf("error", "reconnecting")) mutableView.value = BridgeView()
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }
    companion object {
        const val STOP = "io.gamelink.app.STOP"
        private const val CHANNEL = "console-bridge"
        private const val NOTIFICATION = 100
        private val mutableView = MutableStateFlow(BridgeView())
        val view = mutableView.asStateFlow()
    }
}
