package io.gamelink.app

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { MaterialTheme { ConsoleBridge() } }
    }

    @Composable private fun ConsoleBridge() {
        val scope = rememberCoroutineScope()
        val bridge by BridgeService.view.collectAsStateWithLifecycle()
        var servers by remember { mutableStateOf(emptyList<SavedServer>()) }
        var selectedId by remember { mutableStateOf("") }
        var publicKey by remember { mutableStateOf("") }
        var invitation by remember { mutableStateOf("") }
        var error by remember { mutableStateOf("") }
        var busy by remember { mutableStateOf(false) }
        var wifi by remember { mutableStateOf(emptyList<WifiNetwork>()) }
        var networkId by remember { mutableStateOf("") }
        var consent by remember { mutableStateOf(false) }
        var permissionsVersion by remember { mutableIntStateOf(0) }
        val active = bridge.phase in listOf("connecting", "connected", "reconnecting", "unreachable") && bridge.address.isNotEmpty() || bridge.phase == "connecting"
        val permissions = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
            permissionsVersion++
        }
        fun requiredPermissions(): Array<String> = buildList {
            if (Build.VERSION.SDK_INT >= 33) add(Manifest.permission.POST_NOTIFICATIONS)
            if (Build.VERSION.SDK_INT >= 37) add(Manifest.permission.ACCESS_LOCAL_NETWORK)
        }.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }.toTypedArray()
        val missing = remember(permissionsVersion) { requiredPermissions() }

        LaunchedEffect(Unit) {
            try {
                val repo = Repository.get(this@MainActivity)
                val result = withContext(Dispatchers.IO) { repo.publicKey() to repo.servers() }
                publicKey = result.first; servers = result.second
                selectedId = servers.firstOrNull()?.id ?: ""
            } catch (_: Exception) {
                error = "Cannot unlock device storage. Reinstalling creates a new identity that needs host approval."
            }
        }
        DisposableEffect(Unit) {
            val watcher = WifiNetworks(this@MainActivity) { networks -> runOnUiThread {
                wifi = networks
                if (networks.none { it.id == networkId }) networkId = networks.firstOrNull()?.id ?: ""
            } }
            watcher.start()
            onDispose { watcher.close() }
        }
        Surface(Modifier.fillMaxSize()) {
            Column(Modifier.safeDrawingPadding().verticalScroll(rememberScrollState()).padding(24.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp)) {
                Text("GameLink", style = MaterialTheme.typography.headlineLarge)
                Text("Android Console Bridge", style = MaterialTheme.typography.titleLarge)
                Text("Connect Xbox or PlayStation to a private Bedrock server through this phone’s Wi-Fi. Console compatibility is awaiting hardware validation.")
                Card(Modifier.fillMaxWidth()) { Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(bridge.phase.replaceFirstChar { it.uppercase() }, style = MaterialTheme.typography.titleMedium)
                    Text(bridge.message)
                    if (bridge.address.isNotEmpty()) Text("Wi-Fi: ${bridge.address} · Sessions: ${bridge.sessions}")
                    if (active) Button(onClick = {
                        startService(Intent(this@MainActivity, BridgeService::class.java).setAction(BridgeService.STOP))
                    }) { Text("Stop bridge") }
                } }
                if (error.isNotBlank()) Text(error, color = MaterialTheme.colorScheme.error)
                Text("1. Approve this phone", style = MaterialTheme.typography.titleMedium)
                Text("Send this public key to the server host. The host approves the phone for this game; every console using the bridge shares that access. Minecraft player authentication and allowlists still apply.")
                if (publicKey.isNotEmpty()) {
                    Text(publicKey, style = MaterialTheme.typography.bodySmall)
                    OutlinedButton(onClick = {
                        getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("GameLink public key", publicKey))
                    }) { Text("Copy public key") }
                }
                Text("2. Import the host’s invitation", style = MaterialTheme.typography.titleMedium)
                OutlinedTextField(value = invitation, onValueChange = { if (it.length <= 8192) invitation = it },
                    label = { Text("Private GameLink invitation") }, modifier = Modifier.fillMaxWidth(),
                    minLines = 2, maxLines = 4, enabled = !busy && !active)
                Button(enabled = invitation.isNotBlank() && !busy && !active, onClick = {
                    busy = true; error = ""
                    scope.launch {
                        try {
                            val imported = withContext(Dispatchers.IO) { Repository.get(this@MainActivity).import(invitation) }
                            servers = withContext(Dispatchers.IO) { Repository.get(this@MainActivity).servers() }
                            selectedId = imported.id; invitation = ""
                        } catch (_: Exception) { error = "Invalid or unsupported invitation. Ask the host for a fresh invitation from an agent with Console Bridge enabled." }
                        finally { busy = false }
                    }
                }) { Text(if (busy) "Importing…" else "Import invitation") }
                Text("3. Select a server and Wi-Fi", style = MaterialTheme.typography.titleMedium)
                servers.forEach { server ->
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        RadioButton(selected = selectedId == server.id, enabled = !active,
                            onClick = { selectedId = server.id })
                        Column(Modifier.weight(1f)) {
                            Text(server.name)
                            Text(if (server.bridgeVersion == 1) "Console Bridge v1" else "Host upgrade required for Console Bridge",
                                style = MaterialTheme.typography.bodySmall)
                        }
                        TextButton(enabled = !active, onClick = {
                            scope.launch {
                                try {
                                    servers = withContext(Dispatchers.IO) {
                                        Repository.get(this@MainActivity).apply { remove(server.id) }.servers()
                                    }
                                    if (selectedId == server.id) selectedId = servers.firstOrNull()?.id ?: ""
                                } catch (_: Exception) { error = "Cannot update device storage." }
                            }
                        }) { Text("Remove") }
                    }
                }
                if (wifi.isEmpty()) Text("Connect to a home Wi-Fi network with a private IPv4 address. It must be Android’s active network.")
                wifi.forEach { n -> Row {
                    RadioButton(selected = networkId == n.id, enabled = !active, onClick = { networkId = n.id })
                    Text(n.label, Modifier.padding(top = 12.dp))
                } }
                Row {
                    Checkbox(checked = consent, enabled = !active, onCheckedChange = { consent = it })
                    Text("Allow consoles on this Wi-Fi to use this phone’s access while the bridge runs.", Modifier.padding(top = 8.dp))
                }
                if (missing.isNotEmpty()) OutlinedButton(onClick = { permissions.launch(missing) }) {
                    Text("Allow notifications and local network")
                }
                Button(modifier = Modifier.fillMaxWidth(), enabled = !active && !busy && consent && missing.isEmpty() &&
                    publicKey.isNotEmpty() && selectedId.isNotEmpty() && networkId.isNotEmpty(), onClick = {
                    error = ""
                    try {
                        startForegroundService(Intent(this@MainActivity, BridgeService::class.java)
                            .putExtra("serverId", selectedId).putExtra("networkId", networkId))
                    } catch (_: Exception) { error = "Android could not start the foreground service. Check permissions and try again." }
                }) { Text("Start console bridge") }
                Text("On the console, open Minecraft → Play → Friends / LAN games and select the advertised world. Keep the phone on this Wi-Fi. No VPN, DNS interception, or router change is required.")
                Text("Diagnostics", style = MaterialTheme.typography.titleMedium)
                Text("State: ${bridge.phase}\nConnection stage: ${bridge.stage}\nPath: ${bridge.path}\nActive sessions: ${bridge.sessions}\nStorage: Android Keystore, backup disabled\nConsole and screen-off gameplay: unverified",
                    style = MaterialTheme.typography.bodySmall)
                OutlinedButton(onClick = {
                    val diagnostic = "GameLink Android ${BuildConfig.VERSION_NAME}\nAndroid API ${Build.VERSION.SDK_INT}\nState: ${bridge.phase}\nStage: ${bridge.stage}\nPath: ${bridge.path}\nSessions: ${bridge.sessions}\nWi-Fi available: ${wifi.isNotEmpty()}\nConsole/screen-off validation: unverified"
                    getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("GameLink diagnostics", diagnostic))
                }) { Text("Copy safe diagnostics") }
            }
        }
    }
}
