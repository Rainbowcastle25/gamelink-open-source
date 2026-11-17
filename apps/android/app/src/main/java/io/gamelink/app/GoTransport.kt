package io.gamelink.app

import mobile.Mobile
import org.json.JSONObject

/** Narrow boundary. Go owns all sockets and gameplay bytes. Kotlin only calls
 * identity, validation, start/stop and safe status operations. */
object GoTransport {
    fun identity(): ByteArray = Mobile.generateIdentity()
    fun publicKey(key: ByteArray): String = Mobile.publicKey(key)
    fun preview(invitation: String): JSONObject = JSONObject(Mobile.previewInvitation(invitation))
    fun start(invitation: String, address: String, prefix: Int, interfaceIndex: Int, key: ByteArray): mobile.Runtime =
        Mobile.start(JSONObject().put("invitation", invitation).put("lanIPv4", address)
            .put("lanPrefix", prefix).put("interfaceIndex", interfaceIndex)
            .put("allowLocalRelay", false).toString(), key)
}
