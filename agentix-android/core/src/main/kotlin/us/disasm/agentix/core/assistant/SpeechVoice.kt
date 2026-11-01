package us.disasm.agentix.core.assistant

import java.util.Locale

enum class SpeechProfile(val rate: Float, val pitch: Float) {
    BRITISH_MEASURED(0.95f, 0.95f), SYSTEM(1f, 1f),
}
data class SpeechVoice(val id: String, val locale: Locale, val quality: Int, val latency: Int, val network: Boolean, val installed: Boolean)
data class VoiceSelection(val voice: SpeechVoice, val britishUnavailable: Boolean, val savedUnavailable: Boolean)
fun availableVoices(voices: List<SpeechVoice>, profile: SpeechProfile, device: Locale): List<SpeechVoice> {
    val language = if (profile == SpeechProfile.BRITISH_MEASURED) "en" else device.language
    return voices.filter { !it.network && it.installed && it.locale.language == language && it.id.isNotBlank() }
        .sortedWith(compareBy<SpeechVoice> { it.locale.toLanguageTag() }.thenBy { it.id })
}
fun selectVoice(voices: List<SpeechVoice>, profile: SpeechProfile, device: Locale, saved: String?): VoiceSelection? {
    val available = availableVoices(voices, profile, device)
    val selected = available.find { it.id == saved } ?: available.sortedWith(
        compareByDescending<SpeechVoice> { profile == SpeechProfile.BRITISH_MEASURED && it.locale.country == "GB" }
            .thenByDescending { it.locale == device }.thenByDescending { it.quality }.thenBy { it.latency }.thenBy { it.id },
    ).firstOrNull() ?: return null
    return VoiceSelection(selected,
        profile == SpeechProfile.BRITISH_MEASURED && selected.locale.country != "GB",
        saved != null && available.none { it.id == saved })
}
