package us.disasm.agentix.reminders

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.MediaPlayer
import android.media.RingtoneManager
import android.net.Uri
import android.os.Handler
import android.os.IBinder
import android.os.HandlerThread
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import us.disasm.agentix.MainActivity
import us.disasm.agentix.R
import us.disasm.agentix.core.reminders.Entry
import us.disasm.agentix.core.reminders.AlarmTarget
import java.util.UUID

class ReminderActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val pending = goAsync()
        reminderExecutor.execute {
            try {
                val uri = intent.data ?: return@execute
                if (uri.scheme !in setOf("agentix-dismiss", "agentix-snooze")) return@execute
                val server = UUID.fromString(uri.authority)
                val targets = uri.getQueryParameters("target").map {
                    val parts = it.split('/')
                    require(parts.size == 3)
                    AlarmTarget(UUID.fromString(parts[0]), parts[1].toLong(), parts[2].toLong())
                }
                if (uri.scheme == "agentix-snooze") ReminderEngine.snooze(context, server, targets)
                else ReminderEngine.dismiss(context, server, targets)
            } catch (_: Exception) { android.util.Log.w("AgentixReminders", "Reminder action needs recovery") }
            finally { pending.finish() }
        }
    }
}

/** One alarm stream for all undismissed revisions; no networking on the playback path. */
class ReminderAlarmService : Service() {
    private val worker = HandlerThread("agentix-alarm-audio").apply { start() }
    private val handler = Handler(worker.looper)
    private val audio by lazy { getSystemService(AudioManager::class.java) }
    private val attributes = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ALARM).setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION).build()
    private var player: MediaPlayer? = null
    private var prepared = false
    private var focused = false
    private var requestedFocus = false
    private var wake: PowerManager.WakeLock? = null
    private var shown: Pair<UUID?, List<Entry>>? = null
    private val focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
        .setAudioAttributes(attributes).setAcceptsDelayedFocusGain(true)
        .setOnAudioFocusChangeListener({ change ->
            focused = change == AudioManager.AUDIOFOCUS_GAIN
            if (focused) play() else player?.let { if (prepared && it.isPlaying) it.pause() }
            if (change == AudioManager.AUDIOFOCUS_LOSS) fail()
        }, handler).build()
    private val tick = object : Runnable {
        override fun run() {
            try { refresh() } catch (_: Exception) { fail() }
            if (running) handler.postDelayed(this, 500)
        }
    }
    override fun onBind(intent: Intent?): IBinder? = null
    override fun onCreate() {
        super.onCreate()
        running = true
        ReminderEngine.channel(this)
    }
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Meet the foreground-service deadline before disk/audio work, even on recovery.
        try { startForeground(NOTIFICATION, notification(null, emptyList())) }
        catch (_: RuntimeException) { handler.post { fail() }; return START_NOT_STICKY }
        running = true
        handler.post { handler.removeCallbacks(tick); shown = null; tick.run() }
        return START_STICKY
    }
    private fun notification(server: UUID?, entries: List<Entry>): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
            .setData(Uri.parse("agentix://reminder/${entries.firstOrNull()?.reminder?.id ?: "inbox"}")), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val text = entries.take(3).joinToString("\n") { it.reminder.text } + if (entries.size > 3) "\n${entries.size - 3} more in Inbox" else ""
        val builder = NotificationCompat.Builder(this, ReminderEngine.CHANNEL).setSmallIcon(R.drawable.ic_agentix)
            .setContentTitle(if (entries.size > 1) "${entries.size} reminder alarms" else "Reminder alarm")
            .setContentText(text).setStyle(NotificationCompat.BigTextStyle().bigText(text)).setContentIntent(open)
            .setWhen(entries.firstOrNull()?.reminder?.dueAt?.toEpochMilli() ?: System.currentTimeMillis()).setShowWhen(true)
            .setCategory(NotificationCompat.CATEGORY_ALARM).setPriority(NotificationCompat.PRIORITY_HIGH)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE).setOngoing(true).setOnlyAlertOnce(true).setSilent(true)
        if (server != null && entries.isNotEmpty()) {
            fun action(scheme: String): PendingIntent {
                val uri = Uri.Builder().scheme(scheme).authority(server.toString())
                entries.forEach { uri.appendQueryParameter("target", "${it.reminder.id}/${it.reminder.revision}/${it.generation}") }
                return PendingIntent.getBroadcast(this, 0, Intent(this, ReminderActionReceiver::class.java).setData(uri.build()), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
            }
            builder.addAction(0, if (entries.size > 1) "Snooze all 10 min" else "Snooze 10 min", action("agentix-snooze"))
            builder.addAction(0, if (entries.size > 1) "Dismiss all" else "Dismiss", action("agentix-dismiss"))
        }
        return builder.build()
    }
    private fun refresh() {
        val (server, entries) = ReminderEngine.activeAlarms(this)
        if (entries.isEmpty()) { ReminderEngine.stopIfIdle(this) { finish() }; return }
        if (!ReminderEngine.canNotify(this) || !ReminderEngine.canAlarm(this)) { fail(); return }
        if (shown != (server to entries)) {
            getSystemService(android.app.NotificationManager::class.java).notify(NOTIFICATION, notification(server, entries))
            shown = server to entries
        }
        if (!requestedFocus) {
            requestedFocus = true
            val result = audio.requestAudioFocus(focus)
            focused = result == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
            if (result == AudioManager.AUDIOFOCUS_REQUEST_FAILED) { fail(); return }
        }
        if (player == null) {
            wake = getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "agentix:reminder-alarm").apply { setReferenceCounted(false); acquire(60000) }
            val media = MediaPlayer()
            player = media
            media.setAudioAttributes(attributes)
            media.isLooping = true
            media.setDataSource(this, RingtoneManager.getDefaultUri(RingtoneManager.TYPE_ALARM))
            media.setOnPreparedListener { if (player == media) { prepared = true; play() } }
            media.setOnErrorListener { _, _, _ -> if (player == media) fail(); true }
            media.prepareAsync()
        }
        wake?.acquire(60000)
        if (prepared && focused) play()
    }
    private fun play() {
        if (!prepared || !focused) return
        try {
            if (!ReminderEngine.startSound(this) { player?.let { if (!it.isPlaying) it.start() } }) finish()
        } catch (_: Exception) { fail() }
    }
    private fun fail() {
        runCatching { ReminderEngine.soundFailed(this) }
        finish()
    }
    private fun finish() {
        running = false
        handler.removeCallbacks(tick)
        player?.release(); player = null; prepared = false
        if (requestedFocus) audio.abandonAudioFocusRequest(focus)
        requestedFocus = false; focused = false
        wake?.let { if (it.isHeld) it.release() }; wake = null
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }
    override fun onDestroy() {
        handler.post { finish(); worker.quitSafely() }
        super.onDestroy()
    }
    companion object {
        private const val NOTIFICATION = 28003
        @Volatile internal var running = false
            private set
    }
}
