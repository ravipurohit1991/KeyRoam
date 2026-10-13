package io.github.ravipurohit1991.keyroam

import android.content.Intent
import android.net.Uri
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        // The Rust engine cannot discover Android's app directories itself.
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "keyroam/platform")
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "paths" -> result.success(
                        mapOf(
                            // Private, backed-up-excluded app storage for the vault.
                            "data" to noBackupFilesDir.absolutePath,
                            // App-specific external storage, reachable over USB, for backups.
                            "backups" to (getExternalFilesDir(null) ?: filesDir).absolutePath,
                        )
                    )
                    "openUrl" -> {
                        // Google sign-in page; only https URLs are accepted.
                        val url = call.argument<String>("url")
                        if (url != null && url.startsWith("https://")) {
                            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)))
                            result.success(true)
                        } else {
                            result.success(false)
                        }
                    }
                    else -> result.notImplemented()
                }
            }
    }
}
