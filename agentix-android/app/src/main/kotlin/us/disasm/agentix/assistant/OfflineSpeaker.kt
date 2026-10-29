package us.disasm.agentix.assistant

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import java.util.Locale

internal class OfflineSpeaker(context: Context, private val status: (Int) -> Unit) {
    private val handler = Handler(Looper.getMainLooper())
    private var closed = false
    private var ready = false
    private var initialized = false
    private var pending: String? = null
    private var generation = 0L
    private var engine: TextToSpeech? = null

    init {
        try {
            engine = TextToSpeech(context.applicationContext) { result ->
                handler.post { initialize(result) }
            }
        } catch (_: RuntimeException) {
            status(us.disasm.agentix.R.string.assistant_tts_missing)
        }
    }

    private fun initialize(result: Int) {
        if (closed) return
        val tts = engine ?: return
        initialized = true
        try {
            val locale = Locale.getDefault()
            val voice = if (result == TextToSpeech.SUCCESS) tts.voices
                ?.filter {
                    !it.isNetworkConnectionRequired &&
                        it.locale.language == locale.language &&
                        TextToSpeech.Engine.KEY_FEATURE_NOT_INSTALLED !in it.features.orEmpty()
                }
                ?.sortedByDescending { if (it.locale == locale) 1 else 0 }?.firstOrNull() else null
            ready = voice != null && tts.setVoice(voice) == TextToSpeech.SUCCESS
            tts.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
                override fun onStart(utteranceId: String?) = report(utteranceId, us.disasm.agentix.R.string.assistant_speaking)
                override fun onDone(utteranceId: String?) = report(utteranceId, us.disasm.agentix.R.string.assistant_speech_done)
                @Deprecated("Required legacy callback")
                override fun onError(utteranceId: String?) = report(utteranceId, us.disasm.agentix.R.string.assistant_tts_missing)
            })
            val queued = pending
            pending = null
            if (queued != null) {
                if (ready) speak(queued) else status(us.disasm.agentix.R.string.assistant_tts_missing)
            }
        } catch (_: RuntimeException) {
            ready = false
            pending = null
            status(us.disasm.agentix.R.string.assistant_tts_missing)
        }
    }

    fun speak(text: String) {
        if (closed) return
        if (!ready) {
            // Queue only during initialization; a missing offline voice stays a visible error.
            if (!initialized && engine != null) pending = text else status(us.disasm.agentix.R.string.assistant_tts_missing)
            return
        }
        generation++
        try {
            if (engine?.speak(text, TextToSpeech.QUEUE_FLUSH, null, generation.toString()) != TextToSpeech.SUCCESS) {
                status(us.disasm.agentix.R.string.assistant_tts_missing)
            }
        } catch (_: RuntimeException) { status(us.disasm.agentix.R.string.assistant_tts_missing) }
    }

    private fun report(id: String?, message: Int) {
        handler.post { if (!closed && id == generation.toString()) status(message) }
    }

    fun stop() {
        pending = null
        generation++
        try { engine?.stop() } catch (_: RuntimeException) { /* Engine disconnected. */ }
    }

    fun close() {
        stop()
        closed = true
        try { engine?.shutdown() } catch (_: RuntimeException) { /* Engine disconnected. */ }
        engine = null
        handler.removeCallbacksAndMessages(null)
    }
}
