package io.gamelink.app

import android.content.Context
import android.util.Base64
import org.json.JSONArray
import org.json.JSONObject
import java.security.MessageDigest

data class SavedServer(val id: String, val name: String, val game: String, val bridgeVersion: Int) {
    fun metadata() = JSONObject().put("id", id).put("name", name).put("game", game)
        .put("bridgeVersion", bridgeVersion)
}

/** Synchronized because activity and foreground service may access storage at once.
 * Secrets are never saved in preferences, intents, UI state or diagnostics. */
class Repository private constructor(context: Context) {
    private val storage = SecretStore(context)
    @Synchronized private fun data(): JSONObject {
        val value = storage.read()
        if (!value.has("identity")) {
            val key = GoTransport.identity()
            try { value.put("identity", Base64.encodeToString(key, Base64.NO_WRAP)) }
            finally { key.fill(0) }
            value.put("servers", JSONArray())
            storage.write(value)
        }
        return value
    }
    @Synchronized fun publicKey(): String {
        val key = privateKey()
        return try { GoTransport.publicKey(key) } finally { key.fill(0) }
    }
    @Synchronized fun privateKey(): ByteArray = Base64.decode(data().getString("identity"), Base64.NO_WRAP)
    @Synchronized fun servers(): List<SavedServer> {
        val list = data().optJSONArray("servers") ?: JSONArray()
        return (0 until list.length()).map { i ->
            val v = list.getJSONObject(i)
            SavedServer(v.getString("id"), v.getString("name"), v.getString("game"), v.optInt("bridgeVersion"))
        }
    }
    @Synchronized fun import(text: String): SavedServer {
        require(text.length <= 8192) { "Invitation is too long" }
        val preview = GoTransport.preview(text)
        val id = MessageDigest.getInstance("SHA-256").digest(text.toByteArray())
            .take(16).joinToString("") { "%02x".format(it) }
        val record = JSONObject().put("id", id).put("name", preview.getString("name"))
            .put("game", preview.getString("game"))
            .put("bridgeVersion", preview.optInt("consoleBridge"))
            .put("invitation", text)
        val value = data()
        val list = value.getJSONArray("servers")
        val existing = (0 until list.length()).indexOfFirst { list.getJSONObject(it).getString("id") == id }
        if (existing >= 0) list.put(existing, record)
        else { require(list.length() < 32) { "At most 32 servers can be saved" }; list.put(record) }
        storage.write(value)
        return SavedServer(id, preview.getString("name"), preview.getString("game"), preview.optInt("consoleBridge"))
    }
    @Synchronized fun invitation(id: String): String {
        val list = data().getJSONArray("servers")
        return (0 until list.length()).map { list.getJSONObject(it) }
            .first { it.getString("id") == id }.getString("invitation")
    }
    @Synchronized fun remove(id: String) {
        val value = data()
        val list = value.getJSONArray("servers")
        value.put("servers", JSONArray((0 until list.length()).map { list.getJSONObject(it) }
            .filter { it.getString("id") != id }))
        storage.write(value)
    }
    companion object {
        @Volatile private var instance: Repository? = null
        fun get(context: Context): Repository = instance ?: synchronized(this) {
            instance ?: Repository(context.applicationContext).also { instance = it }
        }
    }
}
