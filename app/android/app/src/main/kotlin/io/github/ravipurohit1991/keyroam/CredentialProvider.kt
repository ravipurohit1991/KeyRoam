package io.github.ravipurohit1991.keyroam

import android.app.PendingIntent
import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.CancellationSignal
import android.os.OutcomeReceiver
import androidx.annotation.RequiresApi
import androidx.credentials.exceptions.ClearCredentialException
import androidx.credentials.exceptions.CreateCredentialException
import androidx.credentials.exceptions.CreateCredentialUnknownException
import androidx.credentials.exceptions.GetCredentialException
import androidx.credentials.provider.AuthenticationAction
import androidx.credentials.provider.BeginCreateCredentialRequest
import androidx.credentials.provider.BeginCreateCredentialResponse
import androidx.credentials.provider.BeginCreatePasswordCredentialRequest
import androidx.credentials.provider.BeginGetCredentialRequest
import androidx.credentials.provider.BeginGetCredentialResponse
import androidx.credentials.provider.BeginGetPasswordOption
import androidx.credentials.provider.CreateEntry
import androidx.credentials.provider.CredentialEntry
import androidx.credentials.provider.CredentialProviderService
import androidx.credentials.provider.PasswordCredentialEntry
import androidx.credentials.provider.ProviderClearCredentialStateRequest
import org.json.JSONObject

/**
 * Credential Manager provider (Android 14+): offers KeyRoam's logins for the
 * calling app in the system's sign-in sheet and saves passwords apps create.
 * Apps are identified by the package name the system reports; logins match
 * through their `androidapp://` URLs. Websites in browsers are served by the
 * autofill service instead.
 */
@RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
class KeyRoamCredentialService : CredentialProviderService() {
    override fun onBeginGetCredentialRequest(
        request: BeginGetCredentialRequest,
        cancellationSignal: CancellationSignal,
        callback: OutcomeReceiver<BeginGetCredentialResponse, GetCredentialException>,
    ) {
        val pkg = request.callingAppInfo?.packageName
        val options = request.beginGetCredentialOptions.filterIsInstance<BeginGetPasswordOption>()
        if (pkg == null || pkg == packageName || options.isEmpty()) {
            return callback.onResult(BeginGetCredentialResponse())
        }
        KeyRoamEngine.io.execute {
            val response = runCatching {
                KeyRoamEngine.ensureOpen(this)
                CredentialEntries.response(this, pkg, options)
            }.getOrElse { BeginGetCredentialResponse() }
            callback.onResult(response)
        }
    }

    override fun onBeginCreateCredentialRequest(
        request: BeginCreateCredentialRequest,
        cancellationSignal: CancellationSignal,
        callback: OutcomeReceiver<BeginCreateCredentialResponse, CreateCredentialException>,
    ) {
        if (request !is BeginCreatePasswordCredentialRequest) {
            return callback.onError(CreateCredentialUnknownException("KeyRoam saves passwords only"))
        }
        val intent = UnlockActivity.intent(this, UnlockActivity.MODE_CREDENTIAL_CREATE)
        val entry = CreateEntry("KeyRoam", PendingIntent.getActivity(this, 0x4b01, intent, pendingFlags()))
        callback.onResult(BeginCreateCredentialResponse(createEntries = listOf(entry)))
    }

    override fun onClearCredentialStateRequest(
        request: ProviderClearCredentialStateRequest,
        cancellationSignal: CancellationSignal,
        callback: OutcomeReceiver<Void?, ClearCredentialException>,
    ) {
        // KeyRoam keeps no sign-in state for apps.
        callback.onResult(null)
    }
}

@RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
object CredentialEntries {
    /** Entries for [pkg], or an unlock action while the vault is locked. */
    fun response(context: Context, pkg: String, options: List<BeginGetPasswordOption>): BeginGetCredentialResponse {
        val result = KeyRoamEngine.callObject("android_logins", JSONObject().put("package", pkg))
        return when (result.optString("state")) {
            "unlocked" -> {
                val logins = result.optJSONArray("logins")
                val entries = mutableListOf<CredentialEntry>()
                for (option in options) {
                    for (i in 0 until (logins?.length() ?: 0)) {
                        val login = logins!!.getJSONObject(i)
                        val id = login.getString("id")
                        // A distinct data URI keeps each entry's PendingIntent separate.
                        val intent = UnlockActivity.intent(context, UnlockActivity.MODE_CREDENTIAL_GET)
                            .putExtra(UnlockActivity.EXTRA_ENTRY_ID, id)
                            .setData(Uri.parse("keyroam://credential/$id"))
                        val pending = PendingIntent.getActivity(context, 0x4b10 + entries.size, intent, pendingFlags())
                        val username = login.optString("username").ifEmpty { login.optString("title") }
                        entries += PasswordCredentialEntry.Builder(context, username, pending, option)
                            .setDisplayName(login.optString("title"))
                            .build()
                    }
                }
                BeginGetCredentialResponse(credentialEntries = entries)
            }
            "locked" -> {
                val intent = UnlockActivity.intent(context, UnlockActivity.MODE_CREDENTIAL_LIST)
                val pending = PendingIntent.getActivity(context, 0x4b02, intent, pendingFlags())
                BeginGetCredentialResponse(authenticationActions = listOf(AuthenticationAction("KeyRoam", pending)))
            }
            else -> BeginGetCredentialResponse()
        }
    }
}
