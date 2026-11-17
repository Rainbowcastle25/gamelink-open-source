package io.gamelink.app

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** All persistent identity and invitation bytes are device-bound AEAD ciphertext.
 * Files live in noBackupFilesDir. Losing the key requires a new identity/approval. */
class SecretStore(context: Context) {
    private val file = java.io.File(context.noBackupFilesDir, "secrets-v1")
    private val atomic = android.util.AtomicFile(file)
    private val key: SecretKey by lazy {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(ALIAS, null) as? SecretKey) ?: KeyGenerator.getInstance(
            KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore"
        ).apply {
            init(KeyGenParameterSpec.Builder(ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setRandomizedEncryptionRequired(true).build())
        }.generateKey()
    }

    @Synchronized fun read(): org.json.JSONObject {
        // openRead restores AtomicFile's backup after an interrupted write on
        // older Android versions, rather than silently creating a new identity.
        val body = try { atomic.openRead().bufferedReader().use { it.readText() } }
            catch (_: java.io.FileNotFoundException) { return org.json.JSONObject() }
        val encoded = org.json.JSONObject(body)
        val iv = Base64.decode(encoded.getString("iv"), Base64.NO_WRAP)
        val ciphertext = Base64.decode(encoded.getString("data"), Base64.NO_WRAP)
        require(iv.size == 12 && ciphertext.size <= 512 * 1024)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, iv))
        cipher.updateAAD(AAD)
        val clear = cipher.doFinal(ciphertext)
        return try { org.json.JSONObject(clear.toString(Charsets.UTF_8)) } finally { clear.fill(0) }
    }

    @Synchronized fun write(value: org.json.JSONObject) {
        val clear = value.toString().toByteArray()
        require(clear.size <= 500 * 1024)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key)
        cipher.updateAAD(AAD)
        val encrypted = try { cipher.doFinal(clear) } finally { clear.fill(0) }
        val body = org.json.JSONObject()
            .put("iv", Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
            .put("data", Base64.encodeToString(encrypted, Base64.NO_WRAP)).toString()
        val out = atomic.startWrite()
        try {
            out.write(body.toByteArray())
            atomic.finishWrite(out)
        } catch (e: Exception) { atomic.failWrite(out); throw e }
    }

    companion object {
        private const val ALIAS = "gamelink-storage-v1"
        private val AAD = "io.gamelink.app/secrets/v1".toByteArray()
    }
}
