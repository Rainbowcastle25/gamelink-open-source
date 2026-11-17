package io.gamelink.app

import android.content.ContextWrapper
import android.net.ConnectivityManager
import android.os.Build
import android.os.ParcelFileDescriptor
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.Assume.assumeTrue
import org.junit.runner.RunWith
import java.io.File
import java.net.DatagramSocket
import java.net.Inet4Address
import java.net.InetSocketAddress
import java.net.NetworkInterface
import java.util.UUID
import android.util.Base64

@RunWith(AndroidJUnit4::class)
class NativeSandboxTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val targetContext get() = instrumentation.targetContext

    @Test
    fun secretStoreEncryptsAndPersistsWithinAnIsolatedDirectory() {
        val isolatedDir = File(
            targetContext.cacheDir,
            "secret-store-test-${UUID.randomUUID()}"
        ).apply { check(mkdirs()) }
        val isolatedContext = object : ContextWrapper(targetContext) {
            override fun getNoBackupFilesDir(): File = isolatedDir
        }
        try {
            val marker = "dummy-secret-${UUID.randomUUID()}"
            val store = SecretStore(isolatedContext)
            store.write(JSONObject().put("probe", marker).put("padding", "x".repeat(4096)))

            val file = File(isolatedDir, "secrets-v1")
            assertTrue("SecretStore should persist its ciphertext", file.isFile)
            val ciphertext = file.readText()
            assertFalse("the stored file must not contain the plaintext marker", ciphertext.contains(marker))
            assertTrue("the stored file should contain ciphertext data", JSONObject(ciphertext).getString("data").isNotEmpty())

            val restored = SecretStore(isolatedContext).read()
            assertEquals(marker, restored.getString("probe"))
            assertEquals(4096, restored.getString("padding").length)

            // Simulate the backup left by an interrupted AtomicFile write on
            // supported older Android releases. It must retain the identity.
            assertTrue(file.renameTo(File(isolatedDir, "secrets-v1.bak")))
            assertEquals(marker, SecretStore(isolatedContext).read().getString("probe"))
        } finally {
            isolatedDir.deleteRecursively()
        }
    }

    @Test
    fun actualJniReducerReturnsSchemaAndRejectsStaleGeneration() {
        val initial = JSONObject()
            .put("phase", "idle")
            .put("generation", 0)
            .put("message", JSONObject.NULL)
            .put("name", JSONObject.NULL)
            .put("serverId", JSONObject.NULL)
        val metadata = JSONObject()
            .put("id", "instrumented-world")
            .put("name", "Sandbox world")
            .put("game", "minecraft-bedrock")
            .put("bridgeVersion", 1)
        val started = Core.reduce(
            initial,
            JSONObject()
                .put("type", "start")
                .put("server", metadata)
                .put("localIpv4", "192.168.1.44")
        )
        val state = started.getJSONObject("state")
        assertEquals("connecting", state.getString("phase"))
        assertEquals(1, state.getLong("generation"))
        assertEquals("Sandbox world", state.getString("name"))
        assertEquals("instrumented-world", state.getString("serverId"))
        val effects = started.getJSONArray("effects")
        assertEquals(1, effects.length())
        assertEquals("start_transport", effects.getJSONObject(0).getString("type"))
        assertEquals("192.168.1.44", effects.getJSONObject(0).getString("local_ipv4"))

        val stoppedByNetworkLoss = Core.reduce(
            state,
            JSONObject().put("type", "network_lost").put("generation", 1)
        )
        assertEquals("reconnecting", stoppedByNetworkLoss.getJSONObject("state").getString("phase"))
        assertEquals(2, stoppedByNetworkLoss.getJSONObject("state").getLong("generation"))
        assertEquals("stop_transport", stoppedByNetworkLoss.getJSONArray("effects")
            .getJSONObject(0).getString("type"))

        val stale = Core.reduce(
            stoppedByNetworkLoss.getJSONObject("state"),
            JSONObject().put("type", "transport_status")
                .put("generation", 1).put("status", "connected")
        )
        assertEquals("reconnecting", stale.getJSONObject("state").getString("phase"))
        assertEquals(0, stale.getJSONArray("effects").length())
    }

    @Test
    fun goIdentityAndInvitationErrorsStayBoundedAndDoNotLeakInput() {
        val key = GoTransport.identity()
        try {
            assertTrue(key.isNotEmpty())
            val publicKey = GoTransport.publicKey(key)
            assertTrue(publicKey.startsWith("nodekey:"))
            assertNotEquals(key.decodeToString(), publicKey)
        } finally {
            key.fill(0)
        }
        assertTrue("test must confirm the caller zeroed the key", key.all { it == 0.toByte() })

        val malicious = "secret-marker-${UUID.randomUUID()}"
        val error = try {
            GoTransport.preview(malicious)
            throw AssertionError("malformed invitation unexpectedly accepted")
        } catch (expected: Exception) {
            expected.message.orEmpty()
        }
        assertFalse("native validation errors must not echo the invitation", error.contains(malicious))
        assertTrue(error.length <= 256)
    }

    @Test
    fun goNetworkSandboxDiagnosticsAreOptInAndRedacted() {
        assumeTrue(
            "Set instrumentation argument network=true to probe the public endpoint",
            InstrumentationRegistry.getArguments().getString("network") == "true"
        )
        grantLocalNetworkPermissionIfRequired()

        val diagnostics = JSONObject(mobile.Mobile.sandboxDiagnostics())
        instrumentation.sendStatus(0, android.os.Bundle().apply { putString("native-sandbox", diagnostics.toString()) })
        assertTrue(diagnostics.has("dnsOK"))
        assertTrue(diagnostics.has("tlsOK"))
        assertTrue(diagnostics.getInt("interfaces") >= 0)
        assertTrue(diagnostics.getString("goVersion").startsWith("go"))
        assertFalse(diagnostics.has("addresses"))
        assertFalse(diagnostics.has("error"))
        assertTrue("DNS lookup from the Go sandbox failed", diagnostics.getBoolean("dnsOK"))
        assertTrue("TLS roots/request from the Go sandbox failed", diagnostics.getBoolean("tlsOK"))
    }

    @Test
    fun actualTailcatFixtureConnectsThroughGoSandboxAndReleasesDiscoveryPort() {
        assumeTrue(
            "Set instrumentation argument fixture=true after publishing android-fixture.json",
            InstrumentationRegistry.getArguments().getString("fixture") == "true"
        )
        grantLocalNetworkPermissionIfRequired()

        val fixtureFile = File(targetContext.filesDir, "android-fixture.json")
        assertTrue("protected Tailcat fixture was not copied into target app files", fixtureFile.isFile)
        val fixture = JSONObject(fixtureFile.readText())
        val invitation = fixture.getString("invitation")
        val privateKey = Base64.decode(fixture.getString("privateKey"), Base64.DEFAULT)
        var runtime: mobile.Runtime? = null
        try {
            val cm = targetContext.getSystemService(ConnectivityManager::class.java)
            val active = cm.activeNetwork ?: throw AssertionError("no active network")
            val properties = cm.getLinkProperties(active) ?: throw AssertionError("no active link properties")
            val address = properties.linkAddresses.firstOrNull {
                it.address is Inet4Address && it.address.isSiteLocalAddress
            } ?: throw AssertionError("no private IPv4 address on active network")
            val interfaceName = properties.interfaceName ?: throw AssertionError("no active interface name")
            val interfaceIndex = NetworkInterface.getByName(interfaceName)?.index
                ?: throw AssertionError("active interface is unavailable")

            // The fixture is an explicit local test relay. Production UI
            // always leaves this opt-in disabled.
            val startedRuntime = mobile.Mobile.start(JSONObject()
                .put("invitation", invitation)
                .put("lanIPv4", address.address.hostAddress ?: throw AssertionError("private IPv4 address is unavailable"))
                .put("lanPrefix", address.prefixLength)
                .put("interfaceIndex", interfaceIndex)
                .put("allowLocalRelay", true).toString(), privateKey)
            runtime = startedRuntime
            privateKey.fill(0)

            val deadline = android.os.SystemClock.elapsedRealtime() + 45_000
            var status = JSONObject(startedRuntime.status())
            while (status.optString("state") != "connected" &&
                android.os.SystemClock.elapsedRealtime() < deadline) {
                Thread.sleep(500)
                status = JSONObject(startedRuntime.status())
            }
            assertEquals("Go sandbox did not complete Tailcat plus discovery (stage=${status.optString("stage")})", "connected", status.getString("state"))
            assertTrue(status.optString("path") in setOf("direct", "relay", "unknown"))
            instrumentation.sendStatus(0, android.os.Bundle().apply {
                putString("tailcat-fixture", "connected:${status.optString("path")}")
            })
        } finally {
            privateKey.fill(0)
            runtime?.stop()
            fixtureFile.delete()
        }

        val stopped = JSONObject(runtime?.status() ?: "{}")
        assertEquals("stopped", stopped.getString("state"))
        val socket = DatagramSocket(null)
        try {
            socket.reuseAddress = false
            val cm = targetContext.getSystemService(ConnectivityManager::class.java)
            val active = cm.activeNetwork ?: throw AssertionError("no active network after stop")
            val properties = cm.getLinkProperties(active) ?: throw AssertionError("no active link properties after stop")
            val lanAddress = properties.linkAddresses.firstOrNull {
                it.address is Inet4Address && it.address.isSiteLocalAddress
            }?.address ?: throw AssertionError("no active private IPv4 after stop")
            socket.bind(InetSocketAddress(lanAddress, 7551))
        } finally {
            socket.close()
        }
    }

    private fun grantLocalNetworkPermissionIfRequired() {
        if (Build.VERSION.SDK_INT < 37) return
        val permission = "android.permission.ACCESS_LOCAL_NETWORK"
        if (targetContext.checkSelfPermission(permission) == android.content.pm.PackageManager.PERMISSION_GRANTED) return
        val descriptor = instrumentation.uiAutomation.executeShellCommand(
            "pm grant ${targetContext.packageName} $permission"
        )
        ParcelFileDescriptor.AutoCloseInputStream(descriptor).bufferedReader().use { it.readText() }
        assertEquals(
            android.content.pm.PackageManager.PERMISSION_GRANTED,
            targetContext.checkSelfPermission(permission)
        )
    }
}
