package io.github.ravipurohit1991.keyroam

import android.app.Activity
import android.app.assist.AssistStructure
import android.content.Context
import android.content.Intent
import android.content.IntentSender
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import android.text.InputType
import android.util.TypedValue
import android.view.View
import android.view.autofill.AutofillManager
import android.view.inputmethod.EditorInfo
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView
import androidx.annotation.RequiresApi
import androidx.credentials.CreatePasswordRequest
import androidx.credentials.CreatePasswordResponse
import androidx.credentials.GetCredentialResponse
import androidx.credentials.PasswordCredential
import androidx.credentials.provider.BeginGetPasswordOption
import androidx.credentials.provider.PendingIntentHandler
import androidx.fragment.app.FragmentActivity
import org.json.JSONObject

/**
 * Unlocks KeyRoam for a system request (autofill, autofill save, or a
 * Credential Manager sign-in or save) and then completes that request.
 * If the vault is already unlocked, it completes at once.
 */
class UnlockActivity : FragmentActivity() {
    companion object {
        const val EXTRA_MODE = "io.github.ravipurohit1991.keyroam.MODE"
        const val EXTRA_ENTRY_ID = "io.github.ravipurohit1991.keyroam.ENTRY_ID"
        const val MODE_AUTOFILL = "autofill"
        const val MODE_AUTOFILL_SAVE = "autofill_save"
        const val MODE_CREDENTIAL_LIST = "credential_list"
        const val MODE_CREDENTIAL_GET = "credential_get"
        const val MODE_CREDENTIAL_CREATE = "credential_create"

        fun intent(context: Context, mode: String): Intent =
            Intent(context, UnlockActivity::class.java).putExtra(EXTRA_MODE, mode)

        @RequiresApi(Build.VERSION_CODES.O)
        fun autofillIntent(context: Context): IntentSender =
            activityIntentSender(context, intent(context, MODE_AUTOFILL), 0x4b20)

        @RequiresApi(Build.VERSION_CODES.O)
        fun saveIntent(context: Context): IntentSender =
            activityIntentSender(context, intent(context, MODE_AUTOFILL_SAVE), 0x4b21)
    }

    private lateinit var password: EditText
    private lateinit var error: TextView
    private lateinit var unlockButton: Button
    private val mode get() = intent.getStringExtra(EXTRA_MODE) ?: MODE_AUTOFILL

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setResult(Activity.RESULT_CANCELED)
        setFinishOnTouchOutside(false)
        buildUi()
        KeyRoamEngine.io.execute {
            val unlocked = runCatching {
                KeyRoamEngine.ensureOpen(this)
                KeyRoamEngine.isUnlocked()
            }.getOrDefault(false)
            KeyRoamEngine.main.post {
                when {
                    unlocked -> complete()
                    Biometrics.enabled(this) && Biometrics.available(this) -> unlockWithBiometrics()
                }
            }
        }
    }

    private fun dp(v: Int) = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v.toFloat(), resources.displayMetrics).toInt()

    private fun buildUi() {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(24), dp(20), dp(24), dp(16))
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                // KeyRoam must not offer to autofill its own master password.
                importantForAutofill = View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS
            }
        }
        root.addView(TextView(this).apply {
            text = "Unlock KeyRoam"
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 20f)
        })
        password = EditText(this).apply {
            hint = "Master password"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            imeOptions = EditorInfo.IME_ACTION_DONE
            setOnEditorActionListener { _, action, _ ->
                (action == EditorInfo.IME_ACTION_DONE).also { if (it) unlockWithPassword() }
            }
        }
        root.addView(password, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT).apply {
            topMargin = dp(16)
        })
        error = TextView(this).apply {
            setTextColor(Color.rgb(0xb3, 0x26, 0x1e))
            visibility = View.GONE
        }
        root.addView(error)
        val buttons = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = android.view.Gravity.END
        }
        buttons.addView(Button(this).apply {
            text = "Cancel"
            setOnClickListener { finish() }
        })
        if (Biometrics.enabled(this) && Biometrics.available(this)) {
            buttons.addView(Button(this).apply {
                text = "Biometrics"
                setOnClickListener { unlockWithBiometrics() }
            })
        }
        unlockButton = Button(this).apply {
            text = "Unlock"
            setOnClickListener { unlockWithPassword() }
        }
        buttons.addView(unlockButton)
        root.addView(buttons, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT).apply {
            topMargin = dp(12)
        })
        setContentView(root)
    }

    private fun showError(message: String?) {
        error.text = message ?: "KeyRoam could not unlock"
        error.visibility = View.VISIBLE
        unlockButton.isEnabled = true
    }

    private fun unlockWithPassword() {
        val pw = password.text.toString()
        if (pw.isEmpty()) return
        unlockButton.isEnabled = false
        error.visibility = View.GONE
        KeyRoamEngine.io.execute {
            val result = runCatching {
                KeyRoamEngine.ensureOpen(this)
                KeyRoamEngine.call("unlock", JSONObject().put("password", pw))
            }
            KeyRoamEngine.main.post {
                result.fold(
                    { unlocked() },
                    { e ->
                        val wrong = e is KeyRoamEngine.EngineException && e.code == "wrong_password"
                        showError(if (wrong) "Wrong master password" else e.message)
                    },
                )
            }
        }
        password.text.clear()
    }

    private fun unlockWithBiometrics() {
        Biometrics.unlock(this) { result ->
            result.fold(
                { unlocked() },
                { e -> if ((e as? Biometrics.BiometricException)?.code != "cancelled") showError(e.message) },
            )
        }
    }

    private fun unlocked() {
        AutoLock.schedule()
        VaultEvents.changed()
        complete()
    }

    /** Finish the system request that brought up this screen. */
    private fun complete() {
        val m = mode
        KeyRoamEngine.io.execute {
            val result = Intent()
            val outcome = runCatching {
                when (m) {
                    MODE_AUTOFILL -> Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && completeAutofill(result)
                    MODE_AUTOFILL_SAVE -> completeSave()
                    else -> Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE && completeCredential(result)
                }
            }
            KeyRoamEngine.main.post {
                outcome.fold(
                    { ok ->
                        if (ok) setResult(Activity.RESULT_OK, result)
                        finish()
                    },
                    { e -> showError(e.message) },
                )
            }
        }
    }

    @RequiresApi(Build.VERSION_CODES.O)
    private fun completeAutofill(result: Intent): Boolean {
        @Suppress("DEPRECATION")
        val structure = intent.getParcelableExtra<AssistStructure>(AutofillManager.EXTRA_ASSIST_STRUCTURE) ?: return false
        val screen = Screen.parse(structure)
        val logins = KeyRoamEngine.callObject("android_logins", screen.params())
        val response = FillResponses.build(this, screen, logins) { UnlockActivity.autofillIntent(this) } ?: return false
        result.putExtra(AutofillManager.EXTRA_AUTHENTICATION_RESULT, response)
        return true
    }

    private fun completeSave(): Boolean {
        val params = PendingSave.params ?: return false
        PendingSave.params = null
        KeyRoamEngine.call("android_save", params)
        VaultEvents.changed()
        return true
    }

    @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
    private fun completeCredential(result: Intent): Boolean {
        when (mode) {
            MODE_CREDENTIAL_LIST -> {
                val request = PendingIntentHandler.retrieveBeginGetCredentialRequest(intent) ?: return false
                val pkg = request.callingAppInfo?.packageName ?: return false
                val options = request.beginGetCredentialOptions.filterIsInstance<BeginGetPasswordOption>()
                PendingIntentHandler.setBeginGetCredentialResponse(result, CredentialEntries.response(this, pkg, options))
            }
            MODE_CREDENTIAL_GET -> {
                val request = PendingIntentHandler.retrieveProviderGetCredentialRequest(intent) ?: return false
                val id = intent.getStringExtra(EXTRA_ENTRY_ID) ?: return false
                val params = JSONObject().put("package", request.callingAppInfo.packageName).put("id", id)
                val login = KeyRoamEngine.callObject("android_credential", params)
                val credential = PasswordCredential(login.getString("username"), login.getString("password"))
                PendingIntentHandler.setGetCredentialResponse(result, GetCredentialResponse(credential))
            }
            MODE_CREDENTIAL_CREATE -> {
                val request = PendingIntentHandler.retrieveProviderCreateCredentialRequest(intent) ?: return false
                val create = request.callingRequest as? CreatePasswordRequest ?: return false
                val params = JSONObject()
                    .put("package", request.callingAppInfo.packageName)
                    .put("username", create.id)
                    .put("password", create.password)
                KeyRoamEngine.call("android_save", params)
                VaultEvents.changed()
                PendingIntentHandler.setCreateCredentialResponse(result, CreatePasswordResponse())
            }
            else -> return false
        }
        return true
    }
}
