package us.disasm.agentix.assistant

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import us.disasm.agentix.core.assistant.SpeechOutput
import us.disasm.agentix.core.assistant.SpeechProfile
import us.disasm.agentix.core.assistant.SpeechVoice

internal class AssistantSpeaker(context: Context, status: (Int) -> Unit) {
    private val preferences = context.getSharedPreferences("assistant_voice", Context.MODE_PRIVATE)
    var output by mutableStateOf(SpeechOutput.fromPreference(preferences.getString("speech_output", null)))
        private set
    private val android = OfflineSpeaker(context) { if (output == SpeechOutput.ANDROID) status(it) }
    private val kokoro = KokoroSpeaker(context) { if (output != SpeechOutput.ANDROID) status(it) }
    val profile get() = android.profile
    val voices get() = android.voices
    val selected get() = android.selected
    val canPreview get() = if (output == SpeechOutput.ANDROID) selected != null else kokoro.supported
    val notice get() = if (output == SpeechOutput.ANDROID) android.notice
        else if (!kokoro.supported) "Kokoro requires an arm64 phone. Android voices remain available."
        else kokoro.notice ?: "British English, measured pace. Runs offline; first Preview loads the bundled model."

    fun choose(value: SpeechOutput) {
        stop()
        output = value
        preferences.edit().putString("speech_output", value.preference).apply()
    }
    fun refresh() = android.refresh()
    fun label(voice: SpeechVoice) = android.label(voice)
    fun select(id: String) = android.select(id)
    fun updateProfile(profile: SpeechProfile) = android.updateProfile(profile)
    fun speak(text: String) {
        val voice = output.kokoroVoice
        if (voice == null) android.speak(text) else kokoro.speak(text, voice)
    }
    fun stop() { android.stop(); kokoro.stop() }
    fun close() { android.close(); kokoro.close() }
}
