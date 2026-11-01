package us.disasm.agentix.wake

import android.annotation.SuppressLint
import android.content.Context
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.Build
import android.os.PowerManager
import android.os.SystemClock
import com.k2fsa.sherpa.onnx.KeywordSpotter
import com.k2fsa.sherpa.onnx.KeywordSpotterConfig
import com.k2fsa.sherpa.onnx.OnlineModelConfig
import com.k2fsa.sherpa.onnx.OnlineStream
import com.k2fsa.sherpa.onnx.OnlineTransducerModelConfig
import java.util.concurrent.atomic.AtomicBoolean

internal enum class CaptureResult { STOPPED, DETECTED, FAILED }

internal class KeywordCapture(private val context: Context) {
    val stopped = AtomicBoolean(false)

    @SuppressLint("MissingPermission", "WakelockTimeout")
    fun run(ready: () -> Unit): CaptureResult {
        // Service checks permission before admission; revocation/read failure ends capture below.
        var recorder: AudioRecord? = null
        var spotter: KeywordSpotter? = null
        var stream: OnlineStream? = null
        var lock: PowerManager.WakeLock? = null
        try {
            val power = context.getSystemService(PowerManager::class.java)
            lock = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "agentix:wake-word").apply { setReferenceCounted(false); acquire(600_000) }
            val detector = KeywordSpotter(context.assets, KeywordSpotterConfig(
                modelConfig = OnlineModelConfig(
                    transducer = OnlineTransducerModelConfig("wake/encoder.onnx", "wake/decoder.onnx", "wake/joiner.onnx"),
                    tokens = "wake/tokens.txt", numThreads = 1,
                ), keywordsFile = "wake/keywords.txt", keywordsScore = 3f, keywordsThreshold = 0.25f, numTrailingBlanks = 2,
            ))
            spotter = detector
            var input = detector.createStream()
            stream = input
            if (stopped.get()) return CaptureResult.STOPPED
            val minimum = AudioRecord.getMinBufferSize(16000, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
            if (minimum <= 0) return CaptureResult.FAILED
            val audio = AudioRecord.Builder()
                .setAudioSource(MediaRecorder.AudioSource.VOICE_RECOGNITION)
                .setAudioFormat(AudioFormat.Builder().setSampleRate(16000).setChannelMask(AudioFormat.CHANNEL_IN_MONO)
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT).build())
                .setBufferSizeInBytes(maxOf(minimum, 6400)).build()
            recorder = audio
            if (audio.state != AudioRecord.STATE_INITIALIZED) return CaptureResult.FAILED
            if (stopped.get()) return CaptureResult.STOPPED
            var renewed = SystemClock.elapsedRealtime()
            var reset = renewed
            audio.startRecording()
            if (audio.recordingState != AudioRecord.RECORDSTATE_RECORDING) return CaptureResult.FAILED
            ready()
            val buffer = ShortArray(1600)
            while (!stopped.get()) {
                val count = audio.read(buffer, 0, buffer.size, AudioRecord.READ_NON_BLOCKING)
                if (count < 0) return CaptureResult.FAILED
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && audio.activeRecordingConfiguration?.isClientSilenced == true) return CaptureResult.FAILED
                if (count == 0) Thread.sleep(10)
                else {
                    input.acceptWaveform(FloatArray(count) { buffer[it] / 32768f }, 16000)
                    while (!stopped.get() && detector.isReady(input)) {
                        detector.decode(input)
                        if (detector.getResult(input).keyword == "HEY_AGENTIX") return CaptureResult.DETECTED
                    }
                }
                val now = SystemClock.elapsedRealtime()
                if (now - renewed >= 300_000) { lock.acquire(600_000); renewed = now }
                // Rotate the stream so features/history cannot grow over an all-day session.
                if (now - reset >= 15_000) {
                    stream = null
                    input.release()
                    input = detector.createStream()
                    stream = input
                    reset = now
                }
            }
            return CaptureResult.STOPPED
        } catch (_: RuntimeException) { return CaptureResult.FAILED }
        catch (_: LinkageError) { return CaptureResult.FAILED }
        catch (_: InterruptedException) { Thread.currentThread().interrupt(); return CaptureResult.STOPPED }
        finally {
            try { recorder?.stop() } catch (_: RuntimeException) { /* May not have started. */ }
            try { recorder?.release() } catch (_: RuntimeException) { /* Audio service disconnected. */ }
            try { stream?.release() } catch (_: RuntimeException) { /* Continue native cleanup. */ }
            try { spotter?.release() } catch (_: RuntimeException) { /* Continue wake-lock cleanup. */ }
            if (lock?.isHeld == true) lock.release()
        }
    }
}
