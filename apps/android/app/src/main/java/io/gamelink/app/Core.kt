package io.gamelink.app

import org.json.JSONObject

/** Calls are serialized by BridgeService's main dispatcher. No key or invitation
 * enters Rust: the Go adapter owns canonical transport/invitation validation. */
object Core {
    init { System.loadLibrary("gamelink_ffi") }
    @JvmStatic external fun nativeReduce(input: String): String
    fun reduce(state: JSONObject, event: JSONObject): JSONObject = JSONObject(
        nativeReduce(JSONObject().put("state", state).put("event", event).toString())
    )
}
