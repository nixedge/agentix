package us.disasm.agentix.core.assistant

import org.junit.Assert.*
import org.junit.Test

class SpeechOutputTest {
    @Test fun stockVoiceIdentityIsExactAndExistingPreferencesUseAndroid() {
        assertEquals(SpeechOutput.ANDROID, SpeechOutput.fromPreference(null))
        assertEquals(SpeechOutput.ANDROID, SpeechOutput.fromPreference("removed-voice"))
        assertEquals("bm_george", SpeechOutput.fromPreference("kokoro:bm_george").kokoroVoice)
        assertEquals("bm_daniel", SpeechOutput.fromPreference("kokoro:bm_daniel").kokoroVoice)
        SpeechOutput.entries.forEach { assertEquals(it, SpeechOutput.fromPreference(it.preference)) }
    }

    @Test fun chunksPreserveEveryCharacterAndSurrogatePairWithinBounds() {
        val text = "Good evening, captain. 🖖 Tea, Earl Grey, hot.\n".repeat(30)
        val chunks = speechChunks(text, 32)
        assertEquals(text, chunks.joinToString(""))
        assertTrue(chunks.size > 1)
        chunks.forEach {
            assertTrue(it.codePointCount(0, it.length) <= 32)
            assertFalse(Character.isLowSurrogate(it.first()))
            assertFalse(Character.isHighSurrogate(it.last()))
        }
    }

    @Test fun unbrokenUnicodeWordsAreBoundedWithoutDroppingText() {
        val text = "🖖".repeat(400)
        val chunks = speechChunks(text)
        assertEquals(listOf(240, 160), chunks.map { it.codePointCount(0, it.length) })
        assertEquals(text, chunks.joinToString(""))
        assertTrue(speechChunks(" \n ").isEmpty())
    }

    @Test fun tooLongSpeechIsRejectedInsteadOfSilentlyTruncated() {
        assertTrue(speechChunks("a".repeat(8192)).isNotEmpty())
        assertThrows(IllegalArgumentException::class.java) { speechChunks("a".repeat(8193)) }
    }

    @Test fun cancelledOrReplacedSpeechNeverBecomesCurrentAgain() {
        val generation = SpeechGeneration()
        val first = generation.begin()
        assertTrue(generation.current(first))
        generation.cancel()
        assertFalse(generation.current(first))
        val second = generation.begin()
        assertTrue(generation.current(second))
        assertFalse(generation.current(first))
    }
}
