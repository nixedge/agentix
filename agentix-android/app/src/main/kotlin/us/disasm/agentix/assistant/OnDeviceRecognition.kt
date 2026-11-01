package us.disasm.agentix.assistant

import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import java.util.Locale
import us.disasm.agentix.wake.WakeWordRuntime

internal interface RecognitionEvents {
    fun ready() {}
    fun partial(text: String) {}
    fun ended() {}
    fun result(text: String)
    fun error(code: Int)
}

internal class OnDeviceRecognition(
    private val context: Context,
    private val events: RecognitionEvents,
) {
    private val handler = Handler(Looper.getMainLooper())
    private var recognizer: SpeechRecognizer? = null
    private var closed = false
    private var starting = false
    private val timeout = Runnable { fail(SpeechRecognizer.ERROR_SPEECH_TIMEOUT) }

    fun start(request: Intent? = null) {
        if (closed || recognizer != null || starting) return
        starting = true
        WakeWordRuntime.hold(this) { if (!closed) startWhenQuiet(request) }
    }

    private fun startWhenQuiet(request: Intent?) {
        if (!available(context)) {
            fail(SpeechRecognizer.ERROR_CLIENT)
            return
        }
        if (!AssistantMicrophone.acquire(this)) {
            fail(SpeechRecognizer.ERROR_RECOGNIZER_BUSY)
            return
        }
        try {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return
            val engine = SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
            recognizer = engine
            engine.setRecognitionListener(object : RecognitionListener {
                override fun onReadyForSpeech(params: Bundle?) { if (!closed) events.ready() }
                override fun onBeginningOfSpeech() {}
                override fun onRmsChanged(rmsdB: Float) {}
                override fun onBufferReceived(buffer: ByteArray?) {}
                override fun onEndOfSpeech() { if (!closed) events.ended() }
                override fun onError(error: Int) = fail(error)
                override fun onResults(results: Bundle?) {
                    if (closed) return
                    val text = results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull().orEmpty()
                    close()
                    events.result(text)
                }
                override fun onPartialResults(partialResults: Bundle?) {
                    if (!closed) events.partial(partialResults?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull().orEmpty())
                }
                override fun onEvent(eventType: Int, params: Bundle?) {}
            })
            // Copy only supported language preferences; callers cannot request an audio-file source.
            val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
                .putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
                .putExtra(RecognizerIntent.EXTRA_LANGUAGE, request?.getStringExtra(RecognizerIntent.EXTRA_LANGUAGE) ?: Locale.getDefault().toLanguageTag())
                .putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
                .putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1)
                .putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
            handler.postDelayed(timeout, 45_000)
            engine.startListening(intent)
        } catch (_: SecurityException) {
            fail(SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS)
        } catch (_: RuntimeException) {
            fail(SpeechRecognizer.ERROR_CLIENT)
        }
    }

    fun stop() {
        if (closed) return
        try {
            recognizer?.stopListening()
            handler.removeCallbacks(timeout)
            handler.postDelayed(timeout, 8_000)
        } catch (_: RuntimeException) {
            fail(SpeechRecognizer.ERROR_CLIENT)
        }
    }

    fun close() {
        if (closed) return
        closed = true
        handler.removeCallbacks(timeout)
        val engine = recognizer
        recognizer = null
        try {
            engine?.cancel()
        } catch (_: RuntimeException) {
            // Release even if the recognition provider disconnected.
        } finally {
            try { engine?.destroy() } catch (_: RuntimeException) { /* Already disconnected. */ }
            AssistantMicrophone.release(this)
            WakeWordRuntime.release(this)
        }
    }

    private fun fail(code: Int) {
        if (closed) return
        close()
        events.error(code)
    }

    companion object {
        fun available(context: Context): Boolean =
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && try {
                SpeechRecognizer.isOnDeviceRecognitionAvailable(context)
            } catch (_: RuntimeException) { false }
    }
}
