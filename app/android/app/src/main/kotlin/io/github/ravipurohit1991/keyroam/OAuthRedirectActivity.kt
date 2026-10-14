package io.github.ravipurohit1991.keyroam

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import org.json.JSONObject

/**
 * Receives Google's sign-in redirect on the app's private-use URI scheme
 * (the reversed mobile client id, set in build.gradle.kts), hands it to the
 * engine, which is waiting in `complete_sign_in`, and returns to KeyRoam.
 * The engine checks the `state` value and redeems the code with PKCE.
 */
class OAuthRedirectActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        intent?.data?.toString()?.let { url ->
            KeyRoamEngine.io.execute {
                runCatching { KeyRoamEngine.call("oauth_redirect", JSONObject().put("url", url)) }
            }
        }
        // Bring the existing KeyRoam task back to the front.
        packageManager.getLaunchIntentForPackage(packageName)?.let {
            startActivity(it.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_RESET_TASK_IF_NEEDED))
        }
        finish()
    }
}
