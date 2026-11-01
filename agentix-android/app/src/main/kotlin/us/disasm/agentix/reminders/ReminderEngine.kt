package us.disasm.agentix.reminders

import android.Manifest
import android.app.AlarmManager
import android.app.job.JobInfo
import android.app.job.JobScheduler
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.util.AtomicFile
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import org.json.JSONArray
import org.json.JSONObject
import us.disasm.agentix.settings.AssistantSettings
import us.disasm.agentix.core.assistant.OwnerAuthentication
import us.disasm.agentix.core.reminders.*
import us.disasm.agentix.core.inbox.InboxItem
import us.disasm.agentix.core.inbox.InboxKind
import us.disasm.agentix.core.inbox.reminderKey
import java.io.File
import java.time.Instant
import java.util.UUID
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow

object ReminderEngine {
    internal const val CHANNEL = "personal-reminder-alarms"
    private val updates = MutableStateFlow(0L)
    val changes = updates.asStateFlow()
    private val gate = Any()
    private var faulted = false
    private fun file(context: Context) = AtomicFile(File(context.filesDir, "reminders-v1.json"))
    fun enabled(context: Context): Boolean = context.getSharedPreferences("assistant", Context.MODE_PRIVATE).getBoolean("reminders_enabled", false)
    fun canNotify(context: Context): Boolean {
        if (Build.VERSION.SDK_INT >= 33 && ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) return false
        if (!NotificationManagerCompat.from(context).areNotificationsEnabled()) return false
        return context.getSystemService(NotificationManager::class.java).getNotificationChannel(CHANNEL)?.importance != NotificationManager.IMPORTANCE_NONE
    }
    fun canAlarm(context: Context): Boolean = Build.VERSION.SDK_INT < 31 || context.getSystemService(AlarmManager::class.java).canScheduleExactAlarms()
    private fun load(context: Context): Journal? {
        check(!faulted) { "Reminder storage needs recovery" }
        val source = file(context)
        if (!source.baseFile.exists() && !File(source.baseFile.path + ".bak").exists()) return null
        return try {
            val bytes = source.openRead().use { input ->
                val out = java.io.ByteArrayOutputStream(); val buffer = ByteArray(4096)
                while (true) { val count = input.read(buffer); if (count < 0) break; check(out.size() + count <= 8388608); out.write(buffer, 0, count) }
                out.toByteArray()
            }
            ReminderWire.journal(JSONObject(bytes.toString(Charsets.UTF_8)))
        } catch (_: Exception) { faulted = true; error("Reminder storage needs recovery") }
    }
    private fun store(context: Context, state: Journal) {
        check(!faulted) { "Reminder storage needs recovery" }
        val target = file(context); val stream = target.startWrite()
        try { stream.write(ReminderWire.journal(state).toString().toByteArray(Charsets.UTF_8)); target.finishWrite(stream); updates.value += 1 }
        catch (error: Exception) { target.failWrite(stream); faulted = true; throw IllegalStateException("Reminder state could not be persisted", error) }
    }
    fun deviceId(context: Context): UUID = synchronized(gate) { checkNotNull(load(context)).deviceId }
    fun enable(context: Context) {
        check(canNotify(context) && canAlarm(context)) { "Allow notifications and alarms before enabling reminders" }
        val settings = AssistantSettings(context)
        val state = synchronized(gate) {
            val current = load(context) ?: Journal(settings.endpoint(), UUID.randomUUID()).also { store(context, it) }
            check(current.endpoint == settings.endpoint()) { "Reminders belong to another backend; restore its endpoint first" }
            current
        }
        val json = ReminderWire.request(state.endpoint, "/v1/assistant/reminders/enroll", OwnerAuthentication.authorization(settings.token()), JSONObject().put("device_id", state.deviceId))
        apply(context, ReminderWire.snapshot(json))
        check(context.getSharedPreferences("assistant", Context.MODE_PRIVATE).edit().putBoolean("reminders_enabled", true).commit()) { "Reminder setting could not be persisted" }
        channel(context); reconcile(context); scheduleJobs(context); sync(context)
    }
    private fun apply(context: Context, snapshot: Snapshot) = synchronized(gate) {
        val old = checkNotNull(load(context))
        check(old.endpoint == AssistantSettings(context).endpoint()) { "Reminder backend changed" }
        val next = old.apply(snapshot)
        store(context, next)
        old.entries.filter { entry -> next.entries.none { it.reminder == entry.reminder } }.forEach { cancel(context, old, it) }
    }
    fun sync(context: Context, retryUncertain: Boolean = true) {
        if (!enabled(context)) return
        val state = synchronized(gate) { checkNotNull(load(context)) }
        val settings = AssistantSettings(context)
        check(state.endpoint == settings.endpoint()) { "Reminder backend changed" }
        val authorization = OwnerAuthentication.authorization(settings.token())
        val json = ReminderWire.request(state.endpoint, "/v1/assistant/reminders/snapshot?device_id=${state.deviceId}", authorization)
        apply(context, ReminderWire.snapshot(json)); reconcile(context, retryUncertain = retryUncertain)
        repeat(4) {
            val current = synchronized(gate) { checkNotNull(load(context)) }
            val batch = current.reports.take(32)
            if (batch.isEmpty()) return
            val response = ReminderWire.request(state.endpoint, "/v1/assistant/reminders/report", authorization,
                JSONObject().put("server_id", current.serverId).put("device_id", current.deviceId).put("reports", JSONArray(batch.map { ReminderWire.observation(it) })))
            val accepted = response.getJSONArray("accepted")
            require(accepted.length() <= 32)
            val ids = (0 until accepted.length()).map { UUID.fromString(accepted.getString(it)) }.toSet()
            require(ids == batch.map { it.reportId }.toSet())
            synchronized(gate) { val latest = checkNotNull(load(context)); check(latest.serverId == current.serverId); store(context, latest.acknowledge(ids)) }
        }
        requestSync(context)
    }
    internal fun channel(context: Context) {
        context.getSystemService(NotificationManager::class.java).createNotificationChannel(NotificationChannel(CHANNEL, "Reminder alarms", NotificationManager.IMPORTANCE_HIGH).apply { setSound(null, null) })
    }
    private fun operation(context: Context, state: Journal, entry: Entry): PendingIntent {
        val item = entry.reminder
        val uri = Uri.Builder().scheme("agentix-reminder").authority(state.serverId.toString()).appendPath(item.id.toString()).appendPath(item.revision.toString()).appendPath(entry.generation.toString()).build()
        val intent = Intent(context, ReminderAlarmReceiver::class.java).setData(uri)
        return PendingIntent.getBroadcast(context, 0, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }
    private fun tag(state: Journal, id: UUID) = "${state.serverId}/$id"
    private fun cancel(context: Context, state: Journal, entry: Entry) {
        context.getSystemService(AlarmManager::class.java).cancel(operation(context, state, entry))
        NotificationManagerCompat.from(context).cancel(tag(state, entry.reminder.id), 0)
    }
    private fun transition(context: Context, id: UUID, revision: Long, phase: Delivery): Journal {
        val next = checkNotNull(load(context)).transition(id, revision, phase); store(context, next); return next
    }
    fun reconcile(context: Context, restoreAlarms: Boolean = false, retryUncertain: Boolean = true) = synchronized(gate) {
        if (!enabled(context)) return@synchronized
        channel(context)
        val initial = checkNotNull(load(context))
        if (initial.endpoint != AssistantSettings(context).endpoint()) return@synchronized
        var state = initial
        initial.entries.filter { !it.reminder.cancelled && !it.phase.completed() && it.phase != Delivery.RINGING && (restoreAlarms || it.phase != Delivery.ARMED) && it.deadline() > Instant.now() }.forEach {
            state = state.transition(it.reminder.id, it.reminder.revision, Delivery.ALARM_INTENT)
        }
        if (state != initial) store(context, state)
        state.entries.toList().forEach { entry ->
            val item = entry.reminder
            if (restoreAlarms && entry.generation == 0L) {
                // Remove pre-snooze PendingIntents when upgrading to occurrence-aware alarm identities.
                val legacyUri = Uri.Builder().scheme("agentix-reminder").authority(initial.serverId.toString()).appendPath(item.id.toString()).appendPath(item.revision.toString()).build()
                PendingIntent.getBroadcast(context, 0, Intent(context, ReminderAlarmReceiver::class.java).setData(legacyUri), PendingIntent.FLAG_NO_CREATE or PendingIntent.FLAG_IMMUTABLE)?.let {
                    context.getSystemService(AlarmManager::class.java).cancel(it)
                    it.cancel()
                }
            }
            if (item.cancelled) {
                cancel(context, initial, entry)
                state = state.transition(item.id, item.revision, Delivery.CANCEL_APPLIED)
            } else if (!entry.phase.completed()) {
                if (!canNotify(context) || !canAlarm(context)) {
                    context.getSystemService(AlarmManager::class.java).cancel(operation(context, initial, entry))
                    state = state.transition(item.id, item.revision, Delivery.BLOCKED)
                } else if (entry.deadline() <= Instant.now() || entry.phase == Delivery.POST_PENDING || entry.phase == Delivery.RINGING) {
                    if (entry.phase == Delivery.UNCERTAIN && !retryUncertain) return@forEach
                    // Recovery also goes through an exact alarm: boot/jobs cannot start media playback directly.
                    if (!ReminderAlarmService.running || entry.phase !in setOf(Delivery.POST_PENDING, Delivery.RINGING)) {
                        try {
                            context.getSystemService(AlarmManager::class.java).setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, System.currentTimeMillis() + 100, operation(context, initial, entry))
                        } catch (_: SecurityException) { state = state.transition(item.id, item.revision, Delivery.BLOCKED) }
                    }
                } else if (entry.phase != Delivery.ARMED || restoreAlarms) {
                    try {
                        val deadline = entry.deadline()
                        val millis = Math.addExact(deadline.toEpochMilli(), if (deadline.nano % 1000000 == 0) 0L else 1L)
                        context.getSystemService(AlarmManager::class.java).setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, millis, operation(context, initial, entry))
                        state = state.transition(item.id, item.revision, Delivery.ARMED)
                    } catch (_: SecurityException) { state = state.transition(item.id, item.revision, Delivery.BLOCKED) }
                }
            }
        }
        if (state != load(context)) store(context, state)
    }
    private fun post(context: Context, target: AlarmTarget) {
        val current = checkNotNull(load(context))
        val (id, revision) = target
        if (!current.eligible(target, Instant.now())) return
        if (!canNotify(context) || !canAlarm(context)) { transition(context, id, revision, Delivery.BLOCKED); return }
        transition(context, id, revision, Delivery.POST_PENDING)
        try {
            ContextCompat.startForegroundService(context, Intent(context, ReminderAlarmService::class.java))
        } catch (_: SecurityException) { transition(context, id, revision, Delivery.BLOCKED) }
        catch (_: RuntimeException) { transition(context, id, revision, Delivery.UNCERTAIN) }
    }
    fun inbox(context: Context): Journal? = synchronized(gate) { load(context) }
    fun organize(context: Context, server: UUID, target: AlarmTarget, change: (InboxItem) -> Unit) = synchronized(gate) {
        val state = checkNotNull(load(context))
        check(state.serverId == server) { "Reminder backend changed" }
        val entry = checkNotNull(state.entries.find { it.target() == target }) { "Reminder changed" }
        check(!entry.firing()) { "Dismiss or snooze the active alarm before archiving" }
        val item = InboxItem(reminderKey(server, entry.reminder.id, entry.reminder.revision), InboxKind.REMINDER, entry.deadline(), resolved = entry.reminder.cancelled || entry.phase.completed())
        change(item)
    }
    private fun active(state: Journal) = state.entries.filter { !it.reminder.cancelled && it.phase in setOf(Delivery.POST_PENDING, Delivery.RINGING) }.sortedBy { it.reminder.dueAt }
    internal fun activeAlarms(context: Context): Pair<UUID?, List<Entry>> = synchronized(gate) {
        val state = load(context) ?: return@synchronized null to emptyList()
        state.serverId to if (enabled(context) && state.endpoint == AssistantSettings(context).endpoint()) active(state) else emptyList()
    }
    internal fun stopIfIdle(context: Context, stop: () -> Unit) = synchronized(gate) {
        val state = load(context)
        if (state == null || !enabled(context) || state.endpoint != AssistantSettings(context).endpoint() || active(state).isEmpty()) stop()
    }
    // Recheck under the same lock used by dismissal/cancellation immediately before the sound effect.
    internal fun startSound(context: Context, play: () -> Unit): Boolean = synchronized(gate) {
        val state = checkNotNull(load(context))
        val entries = active(state)
        if (!enabled(context) || state.endpoint != AssistantSettings(context).endpoint() || entries.isEmpty()) return@synchronized false
        check(canNotify(context) && canAlarm(context))
        play()
        var next = state
        entries.filter { it.phase != Delivery.RINGING }.forEach { next = next.transition(it.reminder.id, it.reminder.revision, Delivery.RINGING) }
        if (next != state) store(context, next)
        true
    }
    internal fun soundFailed(context: Context) = synchronized(gate) {
        val state = load(context) ?: return@synchronized
        var next = state
        active(state).forEach { next = next.transition(it.reminder.id, it.reminder.revision, Delivery.UNCERTAIN) }
        if (next != state) store(context, next)
        // Periodic/app-entry recovery retries; an immediate sync here would form a tight retry loop.
    }
    fun snooze(context: Context, server: UUID, targets: List<AlarmTarget>) = synchronized(gate) {
        val state = load(context) ?: return@synchronized
        if (state.serverId != server || state.endpoint != AssistantSettings(context).endpoint()) return@synchronized
        val next = state.snooze(server, targets, Instant.now())
        if (next == state) return@synchronized
        store(context, next)
        state.entries.filter { old -> next.entries.any { it.reminder.id == old.reminder.id && it.generation != old.generation } }.forEach { cancel(context, state, it) }
        reconcile(context)
        requestSync(context)
    }
    fun dismiss(context: Context, server: UUID, targets: List<AlarmTarget>) = synchronized(gate) {
        val state = load(context) ?: return@synchronized
        if (state.serverId != server || state.endpoint != AssistantSettings(context).endpoint()) return@synchronized
        val next = state.dismiss(server, targets, Instant.now())
        if (next != state) {
            store(context, next)
            state.entries.filter { old -> next.entries.any { it.reminder.id == old.reminder.id && it.phase == Delivery.DISMISSED && old.phase != Delivery.DISMISSED } }.forEach { cancel(context, state, it) }
        }
        requestSync(context)
    }
    fun fire(context: Context, uri: Uri?) = synchronized(gate) {
        if (!enabled(context) || uri?.scheme != "agentix-reminder" || uri.pathSegments.size != 3) return@synchronized
        val state = checkNotNull(load(context))
        if (uri.authority != state.serverId.toString() || state.endpoint != AssistantSettings(context).endpoint()) return@synchronized
        post(context, AlarmTarget(UUID.fromString(uri.pathSegments[0]), uri.pathSegments[1].toLong(), uri.pathSegments[2].toLong()))
    }
    fun outcome(context: Context, id: UUID, revision: Long): String = synchronized(gate) {
        val entry = load(context)?.entries?.find { it.reminder.id == id } ?: return@synchronized "Saved on the backend; awaiting phone synchronization."
        if (entry.reminder.revision != revision) return@synchronized "The reminder has a newer change; the current phone revision is applied."
        when (entry.phase) {
            Delivery.ARMED -> "Scheduled on this phone."
            Delivery.CANCEL_APPLIED -> "Cancellation applied on this phone."
            Delivery.POSTED -> "The reminder notification was posted on this phone."
            Delivery.RINGING -> "The reminder alarm is active on this phone."
            Delivery.DISMISSED -> "Dismissed on this phone."
            Delivery.BLOCKED -> "Saved, but notifications or alarm access are blocked on this phone."
            else -> "Saved; phone delivery is pending or uncertain."
        }
    }
    fun synchronizeReceipt(context: Context, id: UUID, revision: Long) {
        try { sync(context) }
        catch (error: Exception) {
            requestSync(context)
            val applied = synchronized(gate) { load(context)?.entries?.any { it.reminder.id == id && it.reminder.revision >= revision && it.phase != Delivery.ALARM_INTENT } == true }
            if (!applied) throw error
        }
    }
    fun scheduleJobs(context: Context) {
        if (!enabled(context)) return
        val component = ComponentName(context, ReminderSyncJob::class.java)
        context.getSystemService(JobScheduler::class.java).schedule(JobInfo.Builder(28001, component).setRequiredNetworkType(JobInfo.NETWORK_TYPE_ANY)
            .setPersisted(true).setPeriodic(maxOf(15 * 60000L, JobInfo.getMinPeriodMillis())).build())
        requestSync(context)
    }
    fun requestSync(context: Context) {
        if (!enabled(context)) return
        context.getSystemService(JobScheduler::class.java).schedule(JobInfo.Builder(28002, ComponentName(context, ReminderSyncJob::class.java))
            .setRequiredNetworkType(JobInfo.NETWORK_TYPE_ANY).setBackoffCriteria(30000, JobInfo.BACKOFF_POLICY_EXPONENTIAL).build())
    }
}
