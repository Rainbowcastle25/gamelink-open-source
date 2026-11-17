package io.gamelink.app

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import java.net.Inet4Address

data class WifiNetwork(val network: Network, val address: String, val prefix: Int,
                       val interfaceName: String) {
    val id: String get() = network.networkHandle.toString()
    val label: String get() = "$interfaceName · $address/$prefix"
}

/** Restrict bridge selection to a concrete Wi-Fi address; no cellular/wildcard bind.
 * A changed network/address is a stop boundary, even while the UI is closed. */
class WifiNetworks(context: Context, private val changed: (List<WifiNetwork>) -> Unit) {
    private val cm = context.getSystemService(ConnectivityManager::class.java)
    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) = update()
        override fun onLost(network: Network) = update()
        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) = update()
        override fun onLinkPropertiesChanged(network: Network, props: android.net.LinkProperties) = update()
    }
    private val defaultCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) = update()
        override fun onLost(network: Network) = update()
        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) = update()
    }
    fun start() {
        cm.registerNetworkCallback(NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI).build(), callback)
        cm.registerDefaultNetworkCallback(defaultCallback)
        update()
    }
    fun close() { cm.unregisterNetworkCallback(callback); cm.unregisterNetworkCallback(defaultCallback) }
    private fun update() {
        val list = cm.allNetworks.mapNotNull { n ->
            if (n != cm.activeNetwork) return@mapNotNull null
            val caps = cm.getNetworkCapabilities(n) ?: return@mapNotNull null
            if (!caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
                caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) return@mapNotNull null
            val props = cm.getLinkProperties(n) ?: return@mapNotNull null
            val address = props.linkAddresses.firstOrNull {
                it.address is Inet4Address && it.address.isSiteLocalAddress && !it.address.isLoopbackAddress
            } ?: return@mapNotNull null
            WifiNetwork(n, address.address.hostAddress!!, address.prefixLength,
                props.interfaceName ?: "Wi-Fi")
        }
        changed(list)
    }
}
