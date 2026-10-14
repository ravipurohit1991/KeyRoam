package io.github.ravipurohit1991.keyroam

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.ConnectivityManager
import android.net.Network
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.view.autofill.AutofillManager
import androidx.credentials.CredentialManager
import io.flutter.embedding.android.FlutterFragmentActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

/**
 * Hosts the Flutter UI. A FragmentActivity, because the biometric prompt
 * needs one. The `keyroam/platform` channel gives the UI what the Rust engine
 * cannot do on Android: app folders, opening the sign-in page, biometric
 * unlock, background sync and the system autofill settings.
 */
class MainActivity : FlutterFragmentActivity() {
    private var channel: MethodChannel? = null
    private var networkCallback: ConnectivityManager.NetworkCallback? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val ch = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "keyroam/platform")
        channel = ch
        ch.setMethodCallHandler { call, result ->
            when (call.method) {
                "paths" -> result.success(
                    mapOf(
                        // Private app storage, excluded from backups, for the vault.
                        "data" to KeyRoamEngine.dataDir(this),
                        // App-specific external storage, reachable over USB, for backups.
                        "backups" to (getExternalFilesDir(null) ?: filesDir).absolutePath,
                    ),
                )
                "openUrl" -> {
                    // The Google sign-in page; only https URLs are accepted.
                    val url = call.argument<String>("url")
                    result.success(url != null && url.startsWith("https://") && open(Intent(Intent.ACTION_VIEW, Uri.parse(url))))
                }
                "biometricStatus" -> result.success(
                    mapOf("available" to Biometrics.available(this), "enabled" to Biometrics.enabled(this)),
                )
                "enableBiometric" -> {
                    val password = call.argument<String>("password") ?: ""
                    Biometrics.enable(this, password) { r ->
                        r.fold({ result.success(true) }, { e -> result.error(errorCode(e), e.message, null) })
                    }
                }
                "disableBiometric" -> {
                    Biometrics.disable(this)
                    result.success(null)
                }
                "biometricUnlock" -> Biometrics.unlock(this) { r ->
                    r.fold({ state -> result.success(state) }, { e -> result.error(errorCode(e), e.message, null) })
                }
                "scheduleSync" -> {
                    SyncScheduler.schedulePeriodic(this)
                    result.success(null)
                }
                "syncWhenOnline" -> {
                    SyncScheduler.syncWhenOnline(this)
                    result.success(null)
                }
                "autofillStatus" -> result.success(autofillStatus())
                "openAutofillSettings" -> result.success(openAutofillSettings())
                "openCredentialSettings" -> result.success(openCredentialSettings())
                // iOS refreshes its QuickType suggestions; Android's services ask on demand.
                "credentialsChanged" -> result.success(null)
                else -> result.notImplemented()
            }
        }
        VaultEvents.listener = { ch.invokeMethod("vaultChanged", null) }
    }

    override fun onStart() {
        super.onStart()
        // Sync as soon as connectivity returns while the app is open.
        val cm = getSystemService(ConnectivityManager::class.java) ?: return
        val cb = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                KeyRoamEngine.main.post { channel?.invokeMethod("online", null) }
            }
        }
        runCatching { cm.registerDefaultNetworkCallback(cb) }.onSuccess { networkCallback = cb }
    }

    override fun onStop() {
        networkCallback?.let { cb ->
            runCatching { getSystemService(ConnectivityManager::class.java)?.unregisterNetworkCallback(cb) }
        }
        networkCallback = null
        super.onStop()
    }

    override fun onResume() {
        super.onResume()
        // The UI's own inactivity timer takes over.
        AutoLock.uiVisible = true
        AutoLock.cancel()
    }

    override fun onPause() {
        AutoLock.uiVisible = false
        super.onPause()
    }

    override fun onDestroy() {
        VaultEvents.listener = null
        channel = null
        super.onDestroy()
    }

    private fun errorCode(e: Throwable): String = when (e) {
        is Biometrics.BiometricException -> e.code
        is KeyRoamEngine.EngineException -> e.code
        else -> "failed"
    }

    private fun open(intent: Intent): Boolean = try {
        startActivity(intent)
        true
    } catch (e: ActivityNotFoundException) {
        false
    }

    private fun autofillStatus(): Map<String, Boolean> {
        val supported = Build.VERSION.SDK_INT >= Build.VERSION_CODES.O
        val manager = if (supported) getSystemService(AutofillManager::class.java) else null
        return mapOf(
            "autofill_supported" to (manager?.isAutofillSupported == true),
            // True when KeyRoam is the selected autofill service.
            "autofill_enabled" to (manager?.hasEnabledAutofillServices() == true),
            "credential_provider_supported" to (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE),
        )
    }

    private fun openAutofillSettings(): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return false
        val intent = Intent(Settings.ACTION_REQUEST_SET_AUTOFILL_SERVICE).setData(Uri.parse("package:$packageName"))
        return open(intent)
    }

    private fun openCredentialSettings(): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) return false
        return runCatching { CredentialManager.create(this).createSettingsPendingIntent().send() }.isSuccess
    }
}
