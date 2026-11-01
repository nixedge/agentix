package us.disasm.agentix.wake

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.media.AudioManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.SystemClock
import android.service.voice.VoiceInteractionService
import us.disasm.agentix.MainActivity
import us.disasm.agentix.R
import us.disasm.agentix.assistant.AgentixVoiceService
import us.disasm.agentix.core.assistant.WakeAdmission

class WakeWordService : Service() {
    private val handler = Handler(Looper.getMainLooper())
    private val admission = WakeAdmission()
    private var capture: KeywordCapture? = null
    private var enabled = false
    private var destroyed = false
    private var endStatus = "Stopped."
    private val launchHold = Any()
    private val poll = object : Runnable {
        override fun run() { reconcile(); if (!destroyed && enabled) handler.postDelayed(this, 1000) }
    }

    override fun onBind(intent: Intent?): IBinder? = null
    override fun onCreate() { super.onCreate(); WakeWordRuntime.changed = ::reconcile }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == STOP) {
            preferences(this).edit().putBoolean("enabled", false).apply()
            stopListening("Stopped.")
        } else if (intent?.action == START && !enabled) {
            if (!eligible(this)) { stopListening("Select Agentix as default assistant and allow microphone and notifications, then Start listening."); return START_NOT_STICKY }
            try {
                val manager = getSystemService(NotificationManager::class.java)
                manager.createNotificationChannel(NotificationChannel(CHANNEL, "Hey Agentix listening", NotificationManager.IMPORTANCE_LOW))
                if (manager.getNotificationChannel(CHANNEL).importance == NotificationManager.IMPORTANCE_NONE) {
                    stopListening("Enable the Hey Agentix notification channel, then Start listening.")
                    return START_NOT_STICKY
                }
                val notification = notification("Starting local wake-word detector…")
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
                else startForeground(ID, notification)
                enabled = true
                WakeWordRuntime.running = true
                preferences(this).edit().putBoolean("enabled", true).apply()
                handler.removeCallbacks(poll)
                handler.post(poll)
            } catch (_: RuntimeException) { stopListening("Android could not start the microphone service. Open setup and try Start listening again.") }
        } else if (!enabled) stopSelf()
        return START_NOT_STICKY
    }

    private fun reconcile() {
        if (destroyed) return
        if (enabled && !eligible(this)) { stopListening("Wake listening stopped: default assistant or microphone/notification permission changed."); return }
        if (!enabled || WakeWordRuntime.held || getSystemService(AudioManager::class.java).mode != AudioManager.MODE_NORMAL) {
            admission.invalidate()
            capture?.stopped?.set(true)
            if (enabled) status("Paused during assistant interaction, recording or a call.")
            if (capture == null && !enabled) finishStop()
            return
        }
        if (capture != null || !WakeWordRuntime.reserve()) return
        val worker = KeywordCapture(applicationContext)
        capture = worker
        val token = admission.begin()
        status("Starting local wake-word detector…")
        Thread({
            val result = try {
                worker.run {
                    handler.post { if (!destroyed && enabled && capture === worker && !worker.stopped.get()) status("Listening locally for Hey Agentix.") }
                }
            } catch (_: RuntimeException) { CaptureResult.FAILED }
            catch (_: LinkageError) { CaptureResult.FAILED }
            handler.post {
                if (capture !== worker) return@post
                capture = null
                WakeWordRuntime.quiet()
                if (destroyed) { WakeWordRuntime.status = endStatus; return@post }
                if (result == CaptureResult.FAILED && !worker.stopped.get()) {
                    stopListening("Wake detector stopped: microphone, native runtime or model unavailable. Check microphone privacy and audio use, then Start listening again.")
                } else if (result == CaptureResult.DETECTED && admission.accept(token, SystemClock.elapsedRealtime(), enabled && !worker.stopped.get() && !WakeWordRuntime.held && eligible(this))) {
                    WakeWordRuntime.hold(launchHold)
                    status("Wake phrase detected. Opening assistant…")
                    if (!AgentixVoiceService.showWakeSession()) stopListening("Could not open the selected assistant. Invoke it once with the system gesture, then Start listening again.")
                    handler.postDelayed({ WakeWordRuntime.release(launchHold) }, 5000)
                } else reconcile()
            }
        }, "agentix-wake-word").start()
    }

    private fun status(text: String) {
        if (WakeWordRuntime.status == text) return
        WakeWordRuntime.status = text
        if (enabled) getSystemService(NotificationManager::class.java).notify(ID, notification(text))
    }

    private fun stopListening(message: String) {
        endStatus = message
        enabled = false
        admission.invalidate()
        capture?.stopped?.set(true)
        handler.removeCallbacks(poll)
        WakeWordRuntime.status = if (capture == null) message else "Stopping wake microphone…"
        if (capture == null) finishStop()
    }

    private fun finishStop() {
        WakeWordRuntime.running = false
        WakeWordRuntime.status = endStatus
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun notification(text: String): Notification {
        val open = PendingIntent.getActivity(this, 300, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val stop = PendingIntent.getService(this, 301, Intent(this, WakeWordService::class.java).setAction(STOP), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        return Notification.Builder(this, CHANNEL).setSmallIcon(R.drawable.ic_microphone)
            .setContentTitle("Hey Agentix").setContentText(text).setContentIntent(open).setOngoing(true)
            .setCategory(Notification.CATEGORY_SERVICE).setOnlyAlertOnce(true)
            .addAction(Notification.Action.Builder(null, "Stop listening", stop).build()).build()
    }

    override fun onDestroy() {
        destroyed = true
        enabled = false
        admission.invalidate()
        capture?.stopped?.set(true)
        handler.removeCallbacks(poll)
        WakeWordRuntime.changed = null
        WakeWordRuntime.release(launchHold)
        WakeWordRuntime.running = false
        WakeWordRuntime.status = if (capture == null) endStatus else "Stopping wake microphone…"
        super.onDestroy()
    }

    companion object {
        const val START = "us.disasm.agentix.START_WAKE_WORD"
        const val STOP = "us.disasm.agentix.STOP_WAKE_WORD"
        private const val CHANNEL = "wake-word"
        private const val ID = 300
        fun preferences(context: Context) = context.getSharedPreferences("wake_word", Context.MODE_PRIVATE)
        fun eligible(context: Context): Boolean =
            VoiceInteractionService.isActiveService(context, ComponentName(context, AgentixVoiceService::class.java)) &&
                context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED &&
                (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU || context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) &&
                context.getSystemService(NotificationManager::class.java).areNotificationsEnabled()
    }
}
