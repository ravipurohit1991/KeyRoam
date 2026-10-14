package io.github.ravipurohit1991.keyroam

import android.app.PendingIntent
import android.app.assist.AssistStructure
import android.content.Context
import android.content.Intent
import android.content.IntentSender
import android.os.Build
import android.os.CancellationSignal
import android.service.autofill.AutofillService
import android.service.autofill.Dataset
import android.service.autofill.FillCallback
import android.service.autofill.FillRequest
import android.service.autofill.FillResponse
import android.service.autofill.SaveCallback
import android.service.autofill.SaveInfo
import android.service.autofill.SaveRequest
import android.view.View
import android.view.autofill.AutofillId
import android.view.autofill.AutofillValue
import android.widget.RemoteViews
import androidx.annotation.RequiresApi
import org.json.JSONArray
import org.json.JSONObject

/**
 * A screen to fill, as described by Android's view structure: the app that
 * shows it, the web domain its views report (browsers) and its text fields.
 * Which fields are the username and password, and which logins the screen
 * may see, is decided by the engine (`android_logins`).
 */
@RequiresApi(Build.VERSION_CODES.O)
class Screen(
    val packageName: String,
    val webDomain: String?,
    val webScheme: String?,
    val ids: List<AutofillId>,
    val fields: JSONArray,
) {
    fun params(): JSONObject = JSONObject()
        .put("package", packageName)
        .put("web_domain", webDomain ?: JSONObject.NULL)
        .put("web_scheme", webScheme ?: JSONObject.NULL)
        .put("fields", fields)

    companion object {
        /** Read [structure]; with [withValues], include the typed text (save requests). */
        fun parse(structure: AssistStructure, withValues: Boolean = false): Screen {
            val ids = mutableListOf<AutofillId>()
            val fields = JSONArray()
            var webDomain: String? = null
            var webScheme: String? = null

            fun visit(node: AssistStructure.ViewNode) {
                if (webDomain == null && !node.webDomain.isNullOrEmpty()) {
                    webDomain = node.webDomain
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) webScheme = node.webScheme
                }
                val id = node.autofillId
                if (id != null && node.autofillType == View.AUTOFILL_TYPE_TEXT && node.visibility == View.VISIBLE) {
                    val field = JSONObject()
                        .put("hints", JSONArray(node.autofillHints?.toList() ?: emptyList<String>()))
                        .put("input_type", node.inputType)
                        .put("id_entry", node.idEntry ?: "")
                        .put("hint", node.hint ?: "")
                    node.htmlInfo?.takeIf { it.tag.equals("input", ignoreCase = true) }?.attributes?.forEach { a ->
                        when (a.first?.lowercase()) {
                            "type" -> field.put("html_type", a.second ?: "")
                            "name", "id" -> if (field.optString("html_name").isEmpty()) field.put("html_name", a.second ?: "")
                            "autocomplete" -> field.put("html_autocomplete", a.second ?: "")
                        }
                    }
                    if (withValues) {
                        val text = node.autofillValue?.takeIf { it.isText }?.textValue?.toString()
                        field.put("value", text ?: JSONObject.NULL)
                    }
                    ids.add(id)
                    fields.put(field)
                }
                for (i in 0 until node.childCount) visit(node.getChildAt(i))
            }

            for (w in 0 until structure.windowNodeCount) visit(structure.getWindowNodeAt(w).rootViewNode)
            return Screen(structure.activityComponent.packageName, webDomain, webScheme, ids, fields)
        }
    }
}

@RequiresApi(Build.VERSION_CODES.O)
object FillResponses {
    private fun presentation(context: Context, text: String): RemoteViews =
        RemoteViews(context.packageName, android.R.layout.simple_list_item_1).apply {
            setTextViewText(android.R.id.text1, text)
        }

    private fun index(fields: JSONObject, name: String): Int? =
        if (fields.isNull(name)) null else fields.optInt(name, -1).takeIf { it >= 0 }

    /** Build the response for [screen] from an `android_logins` result. */
    fun build(context: Context, screen: Screen, result: JSONObject, unlockIntent: () -> IntentSender): FillResponse? {
        val fields = result.optJSONObject("fields") ?: return null
        val userId = index(fields, "username")?.let { screen.ids.getOrNull(it) }
        val passwordId = index(fields, "password")?.let { screen.ids.getOrNull(it) }
        val newPasswordId = index(fields, "new_password")?.let { screen.ids.getOrNull(it) }
        val fillIds = listOfNotNull(userId, passwordId ?: newPasswordId)
        if (fillIds.isEmpty()) return null
        val label = result.optJSONObject("target")?.optString("label") ?: screen.packageName

        val builder = FillResponse.Builder()
        val saveTarget = newPasswordId ?: passwordId
        if (saveTarget != null) {
            val type = SaveInfo.SAVE_DATA_TYPE_PASSWORD or (if (userId != null) SaveInfo.SAVE_DATA_TYPE_USERNAME else 0)
            val save = SaveInfo.Builder(type, arrayOf(saveTarget))
                .setFlags(SaveInfo.FLAG_SAVE_ON_ALL_VIEWS_INVISIBLE)
            if (userId != null) save.setOptionalIds(arrayOf(userId))
            builder.setSaveInfo(save.build())
        }

        when (result.optString("state")) {
            "unlocked" -> {
                val logins = result.optJSONArray("logins") ?: JSONArray()
                if (logins.length() == 0 && saveTarget == null) return null
                for (i in 0 until logins.length()) {
                    val login = logins.getJSONObject(i)
                    val username = login.optString("username")
                    val text = if (username.isEmpty()) login.optString("title") else "$username · ${login.optString("title")}"
                    val p = presentation(context, text)
                    val dataset = Dataset.Builder(p)
                    if (userId != null) dataset.setValue(userId, AutofillValue.forText(username), p)
                    (passwordId ?: newPasswordId)?.let { dataset.setValue(it, AutofillValue.forText(login.optString("password")), p) }
                    builder.addDataset(dataset.build())
                }
            }
            "locked" -> builder.setAuthentication(
                fillIds.toTypedArray(),
                unlockIntent(),
                presentation(context, "Unlock KeyRoam for $label"),
            )
            else -> return null
        }
        return builder.build()
    }
}

/** A save the user agreed to while the vault was locked; finished after unlocking. */
object PendingSave {
    @Volatile
    var params: JSONObject? = null
}

/**
 * Android autofill service (Android 8+). Offers logins of the app or site on
 * screen, an unlock prompt while KeyRoam is locked, and saves logins the user
 * agrees to save in the system's save prompt.
 */
@RequiresApi(Build.VERSION_CODES.O)
class KeyRoamAutofillService : AutofillService() {
    override fun onFillRequest(request: FillRequest, cancellationSignal: CancellationSignal, callback: FillCallback) {
        val structure = request.fillContexts.lastOrNull()?.structure ?: return callback.onSuccess(null)
        val screen = Screen.parse(structure)
        // Never fill KeyRoam's own screens (the master password field).
        if (screen.packageName == packageName || screen.ids.isEmpty()) return callback.onSuccess(null)
        KeyRoamEngine.io.execute {
            val response = runCatching {
                KeyRoamEngine.ensureOpen(this)
                val result = KeyRoamEngine.callObject("android_logins", screen.params())
                FillResponses.build(this, screen, result) { UnlockActivity.autofillIntent(this) }
            }.getOrNull()
            KeyRoamEngine.main.post {
                if (!cancellationSignal.isCanceled) callback.onSuccess(response)
            }
        }
    }

    override fun onSaveRequest(request: SaveRequest, callback: SaveCallback) {
        val structure = request.fillContexts.lastOrNull()?.structure ?: return callback.onFailure("Nothing to save")
        val screen = Screen.parse(structure, withValues = true)
        val params = screen.params().put("app_label", appLabel(screen.packageName))
        KeyRoamEngine.io.execute {
            val result = runCatching {
                KeyRoamEngine.ensureOpen(this)
                KeyRoamEngine.call("android_save", params)
            }
            KeyRoamEngine.main.post {
                val e = result.exceptionOrNull()
                when {
                    e == null -> {
                        VaultEvents.changed()
                        callback.onSuccess()
                    }
                    e is KeyRoamEngine.EngineException && e.code == "locked" &&
                        Build.VERSION.SDK_INT >= Build.VERSION_CODES.P -> {
                        // Unlock first; the activity finishes the save.
                        PendingSave.params = params
                        callback.onSuccess(UnlockActivity.saveIntent(this))
                    }
                    else -> callback.onFailure(e.message ?: "KeyRoam could not save this login")
                }
            }
        }
    }

    /** The app's name for the title of a new entry; browsers get the site instead. */
    private fun appLabel(pkg: String): String? = runCatching {
        packageManager.getApplicationLabel(packageManager.getApplicationInfo(pkg, 0)).toString()
    }.getOrNull()
}

internal fun pendingFlags(): Int =
    PendingIntent.FLAG_UPDATE_CURRENT or
        (if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) PendingIntent.FLAG_MUTABLE else 0)

internal fun activityIntentSender(context: Context, intent: Intent, requestCode: Int): IntentSender =
    PendingIntent.getActivity(context, requestCode, intent, pendingFlags()).intentSender
