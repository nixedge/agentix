package us.disasm.agentix.recording

import android.Manifest
import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.SystemClock
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import java.io.FileOutputStream
import java.io.IOException
import us.disasm.agentix.MainActivity
import us.disasm.agentix.R
import us.disasm.agentix.assistant.AssistantMicrophone
import us.disasm.agentix.core.audio.PcmWave

enum class CapturePhase { IDLE, STARTING, RECORDING, STOPPING }

object PhoneCaptureState {
    var phase by mutableStateOf(CapturePhase.IDLE)
    var message by mutableStateOf<Int?>(null)
    var revision by mutableStateOf(0)
    var startedElapsedMillis by mutableStateOf(0L)
    @Volatile var activeFileName: String? = null

    fun refreshLibrary() {
        revision++
    }
}

class PhoneRecordingService : Service() {
    private val handler = Handler(Looper.getMainLooper())
    @Volatile private var stopRequested = false
    private var worker: Thread? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopRequested = true
                if (worker == null) stopSelf() else PhoneCaptureState.phase = CapturePhase.STOPPING
            }
            ACTION_START -> if (worker == null) begin()
            else -> if (worker == null) stopSelf()
        }
        return START_NOT_STICKY
    }

    private fun begin() {
        if (AssistantMicrophone.active) {
            PhoneCaptureState.message = R.string.assistant_mic_busy
            stopSelf()
            return
        }
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            PhoneCaptureState.message = R.string.record_permission
            stopSelf()
            return
        }
        try {
            val manager = getSystemService(NotificationManager::class.java)
            manager.createNotificationChannel(NotificationChannel(CHANNEL, getString(R.string.record_channel), NotificationManager.IMPORTANCE_LOW))
            val open = PendingIntent.getActivity(
                this, 0, Intent(this, MainActivity::class.java).setAction(MainActivity.ACTION_OPEN_RECORD),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            val stop = PendingIntent.getService(
                this, 1, Intent(this, PhoneRecordingService::class.java).setAction(ACTION_STOP),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            val notification = Notification.Builder(this, CHANNEL)
                .setSmallIcon(R.drawable.ic_microphone)
                .setContentTitle(getString(R.string.record_notification))
                .setContentText(getString(R.string.record_notification_detail))
                .setContentIntent(open)
                .setOngoing(true)
                .setCategory(Notification.CATEGORY_SERVICE)
                .addAction(Notification.Action.Builder(null, getString(R.string.record_stop), stop).build())
                .build()
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }
            stopRequested = false
            PhoneCaptureState.phase = CapturePhase.STARTING
            PhoneCaptureState.message = null
            val store = PhoneRecordingStore(filesDir)
            val partial = store.newPartial()
            PhoneCaptureState.activeFileName = partial.name
            worker = Thread({ capture(store, partial) }, "agentix-phone-capture").also { it.start() }
        } catch (_: RuntimeException) {
            PhoneCaptureState.phase = CapturePhase.IDLE
            PhoneCaptureState.activeFileName = null
            PhoneCaptureState.message = R.string.record_failed
            stopSelf()
        }
    }

    @SuppressLint("MissingPermission")
    private fun capture(store: PhoneRecordingStore, partial: java.io.File) {
        // begin() checks RECORD_AUDIO; revocation during capture is handled below.
        var audio: AudioRecord? = null
        var resultMessage = R.string.record_failed
        try {
            store.prepareDirectory()
            val minimum = AudioRecord.getMinBufferSize(PcmWave.SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
            if (minimum <= 0) throw IOException("Unsupported microphone format")
            val bufferSize = maxOf(minimum, 4096)
            val recorder = AudioRecord.Builder()
                .setAudioSource(MediaRecorder.AudioSource.MIC)
                .setAudioFormat(AudioFormat.Builder()
                    .setSampleRate(PcmWave.SAMPLE_RATE)
                    .setChannelMask(AudioFormat.CHANNEL_IN_MONO)
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT).build())
                .setBufferSizeInBytes(bufferSize * 2)
                .build()
            audio = recorder
            if (recorder.state != AudioRecord.STATE_INITIALIZED) throw IOException("Microphone initialization failed")
            FileOutputStream(partial).use { output ->
                output.write(PcmWave.header(0))
                recorder.startRecording()
                if (recorder.recordingState != AudioRecord.RECORDSTATE_RECORDING) throw IOException("Microphone did not start")
                val started = SystemClock.elapsedRealtime()
                handler.post {
                    if (!stopRequested) {
                        PhoneCaptureState.startedElapsedMillis = started
                        PhoneCaptureState.phase = CapturePhase.RECORDING
                    }
                }
                val buffer = ByteArray(bufferSize)
                var dataBytes = 0L
                while (!stopRequested) {
                    val count = recorder.read(buffer, 0, buffer.size, AudioRecord.READ_BLOCKING)
                    if (count < 0) throw IOException("Microphone read failed")
                    if (count > 0) {
                        output.write(buffer, 0, count)
                        dataBytes += count
                    }
                    if (dataBytes + buffer.size > PcmWave.MAX_DATA_BYTES) stopRequested = true
                }
                output.fd.sync()
            }
            resultMessage = if (store.finish(partial) == null) R.string.record_empty else R.string.record_saved
        } catch (_: IOException) {
            // Keep the partial file for PCM recovery on the next library refresh.
        } catch (_: RuntimeException) {
            // Covers microphone revocation, audio-server failure, and invalid recorder state.
        } finally {
            try {
                if (audio?.recordingState == AudioRecord.RECORDSTATE_RECORDING) audio.stop()
            } catch (_: RuntimeException) {
                // Always release even when stopping the audio server fails.
            } finally {
                try {
                    audio?.release()
                } catch (_: RuntimeException) {
                    resultMessage = R.string.record_failed
                }
            }
            handler.post {
                worker = null
                PhoneCaptureState.activeFileName = null
                PhoneCaptureState.phase = CapturePhase.IDLE
                PhoneCaptureState.message = resultMessage
                PhoneCaptureState.refreshLibrary()
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
            }
        }
    }

    override fun onDestroy() {
        stopRequested = true
        super.onDestroy()
    }

    companion object {
        const val ACTION_START = "us.disasm.agentix.START_PHONE_RECORDING"
        const val ACTION_STOP = "us.disasm.agentix.STOP_PHONE_RECORDING"
        private const val CHANNEL = "phone-recording"
        private const val NOTIFICATION_ID = 1
    }
}
