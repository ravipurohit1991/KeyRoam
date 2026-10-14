package io.github.ravipurohit1991.keyroam

import android.content.Context
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import java.util.concurrent.TimeUnit

/**
 * Background sync through WorkManager. The vault key is needed to merge and
 * re-encrypt, so a worker can only sync while the vault is unlocked in this
 * process; when it is locked the worker does nothing.
 */
class SyncWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
    override fun doWork(): Result = try {
        KeyRoamEngine.ensureOpen(applicationContext)
        val r = KeyRoamEngine.callObject("sync_if_unlocked")
        if (r.optJSONObject("report")?.optBoolean("local_changed") == true) VaultEvents.changed()
        Result.success()
    } catch (e: KeyRoamEngine.EngineException) {
        if (e.code == "offline") Result.retry() else Result.success()
    }
}

object SyncScheduler {
    private val online = Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()

    /** Every 15 minutes (WorkManager's minimum) while the device is online. */
    fun schedulePeriodic(context: Context) {
        val request = PeriodicWorkRequestBuilder<SyncWorker>(15, TimeUnit.MINUTES)
            .setConstraints(online)
            .build()
        WorkManager.getInstance(context)
            .enqueueUniquePeriodicWork("keyroam-sync", ExistingPeriodicWorkPolicy.KEEP, request)
    }

    /** Once, as soon as the device has a network connection. */
    fun syncWhenOnline(context: Context) {
        val request = OneTimeWorkRequestBuilder<SyncWorker>()
            .setConstraints(online)
            .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 30, TimeUnit.SECONDS)
            .build()
        WorkManager.getInstance(context)
            .enqueueUniqueWork("keyroam-sync-online", ExistingWorkPolicy.REPLACE, request)
    }
}
