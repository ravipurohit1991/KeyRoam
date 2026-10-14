package io.github.ravipurohit1991.keyroam

import android.content.Context
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import org.json.JSONObject
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Biometric unlock.
 *
 * Turning it on asks the engine for the vault's key-encryption key (KEK, the
 * Argon2 output of the master password) and encrypts it with an AES-GCM key
 * in the Android Keystore. That key can only be used after a strong
 * biometric check, and Android destroys it when biometrics are enrolled or
 * removed. Unlocking decrypts the KEK after the check and hands it to the
 * engine, which unwraps the vault key with it. A master-password change or a
 * vault-key rotation gives the vault a new salt, so the stored KEK stops
 * working and biometric unlock turns itself off.
 */
object Biometrics {
    private const val KEY_ALIAS = "keyroam-biometric-kek"
    private const val FILE_NAME = "biometric-unlock.json"
    private const val TRANSFORMATION = "AES/GCM/NoPadding"
    private const val AUTHENTICATORS = BiometricManager.Authenticators.BIOMETRIC_STRONG

    class BiometricException(val code: String, message: String) : Exception(message)

    fun available(context: Context): Boolean =
        BiometricManager.from(context).canAuthenticate(AUTHENTICATORS) == BiometricManager.BIOMETRIC_SUCCESS

    fun enabled(context: Context): Boolean = file(context).exists()

    fun disable(context: Context) {
        file(context).delete()
        runCatching { keyStore().deleteEntry(KEY_ALIAS) }
    }

    /** Turn biometric unlock on; [password] is checked by the engine. */
    fun enable(activity: FragmentActivity, password: String, done: (Result<Unit>) -> Unit) {
        KeyRoamEngine.io.execute {
            val derived = runCatching {
                KeyRoamEngine.ensureOpen(activity)
                val r = KeyRoamEngine.callObject("biometric_key", JSONObject().put("password", password))
                Base64.decode(r.getString("key"), Base64.NO_WRAP) to r.getString("vault_id")
            }
            KeyRoamEngine.main.post {
                val (kek, vaultId) = derived.getOrElse { return@post done(Result.failure(it)) }
                val cipher = try {
                    newKey()
                    Cipher.getInstance(TRANSFORMATION).apply { init(Cipher.ENCRYPT_MODE, key()) }
                } catch (e: Exception) {
                    kek.fill(0)
                    return@post done(Result.failure(e))
                }
                prompt(activity, "Turn on biometric unlock", cipher) { result ->
                    try {
                        val c = result.getOrThrow()
                        save(activity, c.iv, c.doFinal(kek), vaultId)
                        done(Result.success(Unit))
                    } catch (e: Exception) {
                        done(Result.failure(e))
                    } finally {
                        kek.fill(0)
                    }
                }
            }
        }
    }

    /** Unlock the vault; yields the engine's state JSON. */
    fun unlock(activity: FragmentActivity, done: (Result<String>) -> Unit) {
        val stored = load(activity)
            ?: return done(Result.failure(BiometricException("not_enabled", "Biometric unlock is off.")))
        val cipher = try {
            Cipher.getInstance(TRANSFORMATION).apply {
                init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, stored.iv))
            }
        } catch (e: KeyPermanentlyInvalidatedException) {
            disable(activity)
            return done(
                Result.failure(
                    BiometricException(
                        "invalidated",
                        "Biometrics changed on this device. Unlock with your master password and turn biometric unlock on again.",
                    ),
                ),
            )
        } catch (e: Exception) {
            return done(Result.failure(e))
        }
        prompt(activity, "Unlock KeyRoam", cipher) { result ->
            val kek = try {
                result.getOrThrow().doFinal(stored.ciphertext)
            } catch (e: Exception) {
                return@prompt done(Result.failure(e))
            }
            KeyRoamEngine.io.execute {
                val state = runCatching {
                    KeyRoamEngine.ensureOpen(activity)
                    val key = Base64.encodeToString(kek, Base64.NO_WRAP)
                    KeyRoamEngine.callObject("unlock_with_key", JSONObject().put("key", key)).toString()
                }
                kek.fill(0)
                KeyRoamEngine.main.post {
                    val e = state.exceptionOrNull()
                    if (e is KeyRoamEngine.EngineException && e.code == "wrong_password") {
                        disable(activity)
                        done(
                            Result.failure(
                                BiometricException(
                                    "stale",
                                    "Your master password changed. Unlock with it and turn biometric unlock on again.",
                                ),
                            ),
                        )
                    } else {
                        done(state)
                    }
                }
            }
        }
    }

    private class Stored(val iv: ByteArray, val ciphertext: ByteArray)

    private fun file(context: Context) = File(context.noBackupFilesDir, FILE_NAME)

    private fun save(context: Context, iv: ByteArray, ciphertext: ByteArray, vaultId: String) {
        val json = JSONObject()
            .put("iv", Base64.encodeToString(iv, Base64.NO_WRAP))
            .put("ciphertext", Base64.encodeToString(ciphertext, Base64.NO_WRAP))
            .put("vault_id", vaultId)
        val tmp = File(context.noBackupFilesDir, "$FILE_NAME.tmp")
        tmp.writeText(json.toString())
        if (!tmp.renameTo(file(context))) throw IllegalStateException("could not store the biometric key")
    }

    private fun load(context: Context): Stored? = runCatching {
        val json = JSONObject(file(context).readText())
        Stored(
            Base64.decode(json.getString("iv"), Base64.NO_WRAP),
            Base64.decode(json.getString("ciphertext"), Base64.NO_WRAP),
        )
    }.getOrNull()

    private fun keyStore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private fun key(): SecretKey = keyStore().getKey(KEY_ALIAS, null) as SecretKey

    private fun newKey() {
        val spec = KeyGenParameterSpec.Builder(
            KEY_ALIAS,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .setUserAuthenticationRequired(true)
            .setInvalidatedByBiometricEnrollment(true)
            .apply {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
                }
            }
            .build()
        KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply { init(spec) }.generateKey()
    }

    private fun prompt(activity: FragmentActivity, title: String, cipher: Cipher, done: (Result<Cipher>) -> Unit) {
        val callback = object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                val c = result.cryptoObject?.cipher
                done(if (c != null) Result.success(c) else Result.failure(BiometricException("failed", "No key")))
            }

            override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                val cancelled = errorCode == BiometricPrompt.ERROR_NEGATIVE_BUTTON ||
                    errorCode == BiometricPrompt.ERROR_USER_CANCELED ||
                    errorCode == BiometricPrompt.ERROR_CANCELED
                done(Result.failure(BiometricException(if (cancelled) "cancelled" else "failed", errString.toString())))
            }
        }
        val info = BiometricPrompt.PromptInfo.Builder()
            .setTitle(title)
            .setNegativeButtonText("Use master password")
            .setAllowedAuthenticators(AUTHENTICATORS)
            .build()
        BiometricPrompt(activity, ContextCompat.getMainExecutor(activity), callback)
            .authenticate(info, BiometricPrompt.CryptoObject(cipher))
    }
}
