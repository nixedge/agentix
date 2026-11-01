package us.disasm.agentix.assistant

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import us.disasm.agentix.R
import us.disasm.agentix.core.assistant.*
import java.util.Locale

internal class OfflineSpeaker(context: Context, private val status: (Int) -> Unit) {
    private val app = context.applicationContext
    private val preferences = app.getSharedPreferences("assistant_voice", Context.MODE_PRIVATE)
    private val handler = Handler(Looper.getMainLooper())
    private var closed = false
    private var ready = false
    private var initialized = false
    private var pending: String? = null
    private var generation = 0L
    private var cycle = 0L
    private var engine: TextToSpeech? = null
    private var engineName: String? = null
    var profile by mutableStateOf(runCatching { SpeechProfile.valueOf(preferences.getString("speech_profile", SpeechProfile.BRITISH_MEASURED.name).orEmpty()) }.getOrDefault(SpeechProfile.BRITISH_MEASURED))
        private set
    var voices by mutableStateOf<List<SpeechVoice>>(emptyList())
        private set
    var selected by mutableStateOf<String?>(null)
        private set
    var notice by mutableStateOf<String?>(null)
        private set

    init { startEngine() }
    private fun startEngine() {
        ready = false; initialized = false; voices = emptyList(); selected = null
        cycle++
        val current = cycle
        try {
            engine?.shutdown()
            engine = TextToSpeech(app) { result -> handler.post { if (!closed && current == cycle) initialize(result) } }
        } catch (_: RuntimeException) { initialized = true; status(R.string.assistant_tts_missing) }
    }
    private fun initialize(result: Int) {
        if (closed) return
        initialized = true
        val tts = engine ?: return
        if (result != TextToSpeech.SUCCESS) { status(R.string.assistant_tts_missing); return }
        try {
            engineName = tts.defaultEngine
            tts.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
                override fun onStart(utteranceId: String?) = report(utteranceId, R.string.assistant_speaking)
                override fun onDone(utteranceId: String?) = report(utteranceId, R.string.assistant_speech_done)
                @Deprecated("Required legacy callback")
                override fun onError(utteranceId: String?) = report(utteranceId, R.string.assistant_tts_missing)
            })
            configure()
            val queued = pending
            pending = null
            if (queued != null) speak(queued)
        } catch (_: RuntimeException) { ready = false; pending = null; status(R.string.assistant_tts_missing) }
    }
    fun refresh() {
        if (closed || !initialized) return
        try {
            if (engine?.defaultEngine != engineName) startEngine() else configure()
        } catch (_: RuntimeException) { ready = false; status(R.string.assistant_tts_missing) }
    }
    private fun configure() {
        val tts = engine ?: return
        val native = tts.voices.orEmpty().filter { it.name.isNotBlank() }
        val descriptors = native.map { SpeechVoice(it.name, it.locale, it.quality, it.latency, it.isNetworkConnectionRequired, TextToSpeech.Engine.KEY_FEATURE_NOT_INSTALLED !in it.features.orEmpty()) }
        voices = availableVoices(descriptors, profile, Locale.getDefault())
        val stored = if (preferences.getString("voice_engine", null) == engineName) preferences.getString("voice_name", null) else null
        val selection = selectVoice(descriptors, profile, Locale.getDefault(), stored)
        val voice = native.find { it.name == selection?.voice?.id }
        ready = voice != null && tts.setVoice(voice) == TextToSpeech.SUCCESS &&
            tts.setSpeechRate(profile.rate) == TextToSpeech.SUCCESS && tts.setPitch(profile.pitch) == TextToSpeech.SUCCESS
        selected = if (ready) voice?.name else null
        notice = when {
            !ready -> "No usable offline voice is installed for this profile. Install voice data in Android speech settings."
            selection?.savedUnavailable == true -> "Your saved voice is unavailable. Using another installed offline voice."
            selection?.britishUnavailable == true -> "No British offline voice is installed. Using another English voice."
            else -> null
        }
    }
    fun label(voice: SpeechVoice): String {
        val variants = voices.filter { it.locale == voice.locale }.sortedBy { it.id }
        return "${voice.locale.getDisplayName(Locale.getDefault())} · Voice ${variants.indexOfFirst { it.id == voice.id } + 1}"
    }
    fun select(id: String) {
        if (voices.none { it.id == id }) return
        stop()
        preferences.edit().putString("voice_name", id).putString("voice_engine", engineName).apply()
        refresh()
    }
    fun updateProfile(value: SpeechProfile) {
        stop()
        profile = value
        preferences.edit().putString("speech_profile", value.name).remove("voice_name").remove("voice_engine").apply()
        refresh()
    }
    fun speak(text: String) {
        if (closed) return
        if (!ready) {
            if (!initialized && engine != null) pending = text else status(R.string.assistant_tts_missing)
            return
        }
        generation++
        try {
            if (engine?.speak(text, TextToSpeech.QUEUE_FLUSH, null, generation.toString()) != TextToSpeech.SUCCESS) status(R.string.assistant_tts_missing)
        } catch (_: RuntimeException) { status(R.string.assistant_tts_missing) }
    }
    private fun report(id: String?, message: Int) {
        handler.post { if (!closed && id == generation.toString()) status(message) }
    }
    fun stop() {
        pending = null; generation++
        try { engine?.stop() } catch (_: RuntimeException) { /* Engine disconnected. */ }
    }
    fun close() {
        stop(); closed = true; cycle++
        try { engine?.shutdown() } catch (_: RuntimeException) { /* Engine disconnected. */ }
        engine = null; handler.removeCallbacksAndMessages(null)
    }
}
