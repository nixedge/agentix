package us.disasm.agentix.reminders

import android.app.job.JobParameters
import android.app.job.JobService
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.PowerManager
import java.util.concurrent.Executors

internal val reminderExecutor = Executors.newSingleThreadExecutor()
private val reminderSyncExecutor = Executors.newSingleThreadExecutor()

fun resumeReminders(context: Context) {
    if (!ReminderEngine.enabled(context)) return
    reminderExecutor.execute {
        runCatching { ReminderEngine.reconcile(context, restoreAlarms = true); ReminderEngine.scheduleJobs(context) }
    }
}

class ReminderAlarmReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val pending = goAsync()
        val wake = context.getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "agentix:reminder-delivery")
        wake.acquire(10000)
        reminderExecutor.execute {
            try { ReminderEngine.fire(context.applicationContext, intent.data); ReminderEngine.requestSync(context) }
            catch (_: Exception) { android.util.Log.w("AgentixReminders", "Reminder delivery needs recovery") }
            finally { if (wake.isHeld) wake.release(); pending.finish() }
        }
    }
}
class ReminderRecoveryReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val pending = goAsync()
        val wake = context.getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "agentix:reminder-recovery")
        wake.acquire(10000)
        reminderExecutor.execute {
            try { ReminderEngine.reconcile(context.applicationContext, restoreAlarms = true); ReminderEngine.scheduleJobs(context) }
            catch (_: Exception) { android.util.Log.w("AgentixReminders", "Reminder recovery is pending") }
            finally { if (wake.isHeld) wake.release(); pending.finish() }
        }
    }
}
class ReminderSyncJob : JobService() {
    override fun onStartJob(params: JobParameters): Boolean {
        reminderSyncExecutor.execute {
            val retry = runCatching { ReminderEngine.sync(applicationContext, retryUncertain = params.jobId == 28001) }.isFailure
            jobFinished(params, retry)
        }
        return true
    }
    override fun onStopJob(params: JobParameters): Boolean = true
}
