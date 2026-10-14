package io.github.ravipurohit1991.keyroam

import android.content.Context
import android.os.Handler
import android.os.Looper
import org.json.JSONObject
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * The Rust engine (crates/keyroam-ffi) for Kotlin code: the autofill service,
 * the Credential Manager provider, background sync and biometric unlock.
 *
 * It is the same native library, and therefore the same process-wide engine
 * and unlocked vault, that the Flutter UI uses through Dart FFI. Calls take
 * the engine's session lock and some run Argon2, so make them from [io].
 */
object KeyRoamEngine {
    init {
        System.loadLibrary("keyroam_ffi")
    }

    @JvmStatic
    private external fun nativeCall(request: String): String

    /** Background thread for engine calls. */
    val io: ExecutorService = Executors.newSingleThreadExecutor { r -> Thread(r, "keyroam-engine") }

    val main = Handler(Looper.getMainLooper())

    class EngineException(val code: String, message: String) : Exception(message)

    /** Call [method]; returns the `ok` value or throws [EngineException]. */
    fun call(method: String, params: JSONObject = JSONObject()): Any? {
        val request = JSONObject().put("method", method).put("params", params).toString()
        val response = JSONObject(nativeCall(request))
        response.optJSONObject("error")?.let {
            throw EngineException(it.optString("code", "internal"), it.optString("message", "KeyRoam error"))
        }
        return response.opt("ok")
    }

    fun callObject(method: String, params: JSONObject = JSONObject()): JSONObject =
        call(method, params) as? JSONObject ?: JSONObject()

    /** The vault folder; the Flutter UI uses the same one (MainActivity "paths"). */
    fun dataDir(context: Context): String = context.noBackupFilesDir.absolutePath

    /** Open the vault folder unless this process already did (e.g. the UI). */
    fun ensureOpen(context: Context) {
        if (callObject("state").isNull("data_dir")) {
            call("open", JSONObject().put("data_dir", dataDir(context)))
        }
    }

    fun isUnlocked(): Boolean = callObject("state").optBoolean("unlocked")
}

/** Tells the Flutter UI (when running) that the vault changed outside it. */
object VaultEvents {
    @Volatile
    var listener: (() -> Unit)? = null

    fun changed() {
        KeyRoamEngine.main.post { listener?.invoke() }
    }
}

/**
 * Locks the vault again after it was unlocked outside the UI (from an
 * autofill or Credential Manager prompt). While the UI is visible, its own
 * inactivity timer applies instead.
 */
object AutoLock {
    const val TIMEOUT_MS = 5 * 60 * 1000L

    @Volatile
    var uiVisible = false

    private val lockNow = Runnable {
        KeyRoamEngine.io.execute {
            runCatching { KeyRoamEngine.call("lock") }
            VaultEvents.changed()
        }
    }

    fun schedule() {
        KeyRoamEngine.main.removeCallbacks(lockNow)
        if (!uiVisible) KeyRoamEngine.main.postDelayed(lockNow, TIMEOUT_MS)
    }

    fun cancel() {
        KeyRoamEngine.main.removeCallbacks(lockNow)
    }
}
