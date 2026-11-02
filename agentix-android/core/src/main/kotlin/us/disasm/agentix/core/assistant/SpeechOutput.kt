package us.disasm.agentix.core.assistant

import java.util.concurrent.atomic.AtomicLong

enum class SpeechOutput(val preference: String, val label: String, val kokoroVoice: String?) {
    ANDROID("android", "Android voices", null),
    KOKORO_GEORGE("kokoro:bm_george", "Kokoro · bm_george", "bm_george"),
    KOKORO_DANIEL("kokoro:bm_daniel", "Kokoro · bm_daniel", "bm_daniel");

    companion object {
        fun fromPreference(value: String?) = entries.firstOrNull { it.preference == value } ?: ANDROID
    }
}

fun speechChunks(text: String, maxCodePoints: Int = 240): List<String> {
    require(maxCodePoints >= 16)
    require(text.codePointCount(0, text.length) <= 8192) { "Speech is too long" }
    if (text.isBlank()) return emptyList()
    val chunks = mutableListOf<String>()
    var start = 0
    while (start < text.length) {
        val remaining = text.codePointCount(start, text.length)
        var end = text.offsetByCodePoints(start, minOf(remaining, maxCodePoints))
        if (end < text.length) {
            var boundary = end
            while (boundary > start) {
                val cp = text.codePointBefore(boundary)
                if (Character.isWhitespace(cp) || cp in listOf('.'.code, '!'.code, '?'.code, ';'.code)) break
                boundary -= Character.charCount(cp)
            }
            if (boundary > start) end = boundary
        }
        chunks += text.substring(start, end)
        start = end
    }
    return chunks
}

class SpeechGeneration {
    private val generation = AtomicLong()
    fun begin(): Long = generation.incrementAndGet()
    fun cancel() { generation.incrementAndGet() }
    fun current(token: Long): Boolean = generation.get() == token
}
