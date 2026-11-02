package us.disasm.agentix.assistant

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.google.mediapipe.tasks.audio.texttospeech.SynthesisOptions
import com.google.mediapipe.tasks.audio.texttospeech.TextToSpeech
import com.google.mediapipe.tasks.audio.texttospeech.TextToSpeechOptions
import com.google.mediapipe.tasks.core.BaseOptions
import com.google.mediapipe.tasks.core.Delegate
import java.io.IOException
import java.util.concurrent.CancellationException
import java.util.concurrent.Executors
import java.util.concurrent.locks.ReentrantReadWriteLock
import us.disasm.agentix.R
import us.disasm.agentix.core.assistant.SpeechGeneration
import us.disasm.agentix.core.assistant.speechChunks
import us.disasm.agentix.wake.WakeWordRuntime

internal class KokoroSpeaker(context: Context, private val status: (Int) -> Unit) {
    private val app = context.applicationContext
    private val handler = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadExecutor { runnable -> Thread(runnable, "agentix-kokoro") }
    private val generation = SpeechGeneration()
    private val nativeAccess = ReentrantReadWriteLock()
    private var engine: TextToSpeech? = null
    @Volatile private var closed = false
    private class Utterance(val token: Long) { var submitted = false }
    private data class Work(val job: Utterance, val chunks: List<String>, val voice: String)
    private var active: Utterance? = null
    private var executing = false
    private var queued: Work? = null
    var notice by mutableStateOf<String?>(null)
        private set
    val supported get() = "arm64-v8a" in Build.SUPPORTED_ABIS

    fun speak(text: String, voice: String) {
        stop()
        if (closed) return
        if (!supported) { notice = "Kokoro requires an arm64 phone. Choose an Android voice on this device."; return }
        require(voice == "bm_george" || voice == "bm_daniel")
        val chunks = try { speechChunks(text) } catch (_: IllegalArgumentException) {
            notice = "This reply is too long to speak. The full text remains available."; return
        }
        if (chunks.isEmpty()) return
        val job = Utterance(generation.begin())
        active = job
        notice = "Preparing local Kokoro speech…"
        WakeWordRuntime.hold(job) {
            if (!closed && generation.current(job.token)) {
                val work = Work(job, chunks, voice)
                if (executing) queued = work else start(work)
            }
        }
    }

    private fun start(work: Work) {
        executing = true
        work.job.submitted = true
        worker.execute { run(work.job, work.chunks, work.voice) }
    }

    private fun run(job: Utterance, chunks: List<String>, voice: String) {
        var track: AudioTrack? = null
        var focus: AudioFocusRequest? = null
        val audio = app.getSystemService(AudioManager::class.java)
        val current = { !closed && generation.current(job.token) }
        try {
            if (!current()) return
            val tts = engine ?: run {
                val model = KokoroModel.install(app, current) { text -> post(job) { notice = text } }
                if (!current()) return
                post(job) { notice = "Loading Kokoro locally. First use may take a moment…" }
                val cache = java.io.File(app.cacheDir, "kokoro").also { check(it.isDirectory || it.mkdirs()) }
                val created = TextToSpeech.create(TextToSpeechOptions(
                    baseOptions = BaseOptions.builder().setModelAssetPath(model.absolutePath).setDelegate(Delegate.CPU).build(),
                    defaultVoiceId = voice, defaultLocale = "en-GB", speakingRate = 0.95f,
                    targetBucket = 128, cacheDirectory = cache.absolutePath,
                ))
                nativeAccess.writeLock().lock()
                try { engine = created } finally { nativeAccess.writeLock().unlock() }
                created
            }
            if (!current()) return
            check(tts.listVoices().any { it.voiceId == voice })
            val attributes = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ASSISTANT).setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build()
            focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK).setAudioAttributes(attributes)
                .setWillPauseWhenDucked(true).setOnAudioFocusChangeListener({ change ->
                    if (generation.current(job.token) && change != AudioManager.AUDIOFOCUS_GAIN) stop()
                }, handler).build()
            if (!current()) return
            if (audio.requestAudioFocus(focus) != AudioManager.AUDIOFOCUS_REQUEST_GRANTED) throw IOException("Audio focus unavailable")
            var writtenFrames = 0L
            for (chunk in chunks) {
                if (!current()) return
                if (chunk.isBlank()) continue
                post(job) { notice = "Synthesizing ${voice} on this phone…" }
                val result = tts.synthesize(chunk, SynthesisOptions(voiceId = voice, locale = "en-GB"))
                if (!current()) return
                check(result.sampleRateHz == 24000 && result.samples.isNotEmpty() && result.samples.size <= 24000 * 120 && result.samples.all { it.isFinite() })
                if (track == null) {
                    val minimum = AudioTrack.getMinBufferSize(result.sampleRateHz, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_FLOAT)
                    check(minimum in 1..192000)
                    track = AudioTrack.Builder().setAudioAttributes(attributes)
                        .setAudioFormat(AudioFormat.Builder().setSampleRate(result.sampleRateHz).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).setEncoding(AudioFormat.ENCODING_PCM_FLOAT).build())
                        .setBufferSizeInBytes(maxOf(minimum, 9600)).setTransferMode(AudioTrack.MODE_STREAM).build()
                    track.play()
                }
                post(job) { notice = "Speaking with $voice, offline."; status(R.string.assistant_speaking) }
                var offset = 0
                var progressed = SystemClock.elapsedRealtime()
                while (offset < result.samples.size && current()) {
                    val count = track.write(result.samples, offset, minOf(1200, result.samples.size - offset), AudioTrack.WRITE_NON_BLOCKING)
                    if (count < 0) throw IOException("Audio output unavailable")
                    if (count == 0) {
                        if (SystemClock.elapsedRealtime() - progressed > 5000) throw IOException("Audio output stalled")
                        Thread.sleep(10)
                    } else { offset += count; writtenFrames += count; progressed = SystemClock.elapsedRealtime() }
                }
            }
            val drainDeadline = SystemClock.elapsedRealtime() + 5000
            while (current() && track != null && (track.playbackHeadPosition.toLong() and 0xffffffffL) < writtenFrames) {
                if (SystemClock.elapsedRealtime() >= drainDeadline) throw IOException("Audio output stalled")
                Thread.sleep(10)
            }
            post(job) { notice = "Kokoro voice ready, offline."; status(R.string.assistant_speech_done) }
        } catch (_: CancellationException) { /* Stop during model installation. */ }
        catch (_: InterruptedException) { Thread.currentThread().interrupt() }
        catch (_: Exception) { post(job) { notice = "Kokoro speech unavailable. Try Preview again or select an Android voice."; status(R.string.assistant_tts_missing) } }
        catch (_: LinkageError) { post(job) { notice = "Kokoro native runtime unavailable on this device. Select an Android voice."; status(R.string.assistant_tts_missing) } }
        finally {
            try { track?.pause(); track?.flush() } catch (_: RuntimeException) { /* Audio service disconnected. */ }
            try { track?.release() } catch (_: RuntimeException) { /* Continue focus cleanup. */ }
            try { if (focus != null) audio.abandonAudioFocusRequest(focus) } catch (_: RuntimeException) { /* Continue wake cleanup. */ }
            handler.post {
                if (active === job) active = null
                WakeWordRuntime.release(job)
                executing = false
                val next = queued
                queued = null
                if (next != null) {
                    if (!closed && generation.current(next.job.token)) start(next)
                    else WakeWordRuntime.release(next.job)
                }
            }
        }
    }

    private fun post(job: Utterance, action: () -> Unit) {
        handler.post { if (!closed && generation.current(job.token)) action() }
    }

    fun stop() {
        generation.cancel()
        val job = active
        active = null
        if (job != null) {
            if (queued?.job === job) queued = null
            if (!job.submitted) WakeWordRuntime.release(job)
            notice = "Kokoro speech stopped."
            // cancel() is lock-free; tryLock avoids waiting on worker-side native destruction.
            val lock = nativeAccess.readLock()
            if (lock.tryLock()) {
                try { engine?.cancel() } catch (_: RuntimeException) { /* Engine disconnected. */ }
                finally { lock.unlock() }
            }
        }
    }

    fun close() {
        if (closed) return
        stop()
        closed = true
        worker.execute {
            nativeAccess.writeLock().lock()
            try { val old = engine; engine = null; old?.close() }
            finally { nativeAccess.writeLock().unlock() }
        }
        worker.shutdown()
    }
}
