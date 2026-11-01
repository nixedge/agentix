package us.disasm.agentix.core.assistant

import org.junit.Assert.*
import org.junit.Test
import java.util.Locale

class VoiceAndHandoffTest {
    private val uk = SpeechVoice("uk-local", Locale.UK, 400, 100, false, true)
    private val us = SpeechVoice("us-local", Locale.US, 500, 100, false, true)
    @Test fun britishPresetPrefersInstalledOfflineBritishVoice() {
        val online = uk.copy(id = "uk-network", quality = 500, network = true)
        val missing = uk.copy(id = "uk-missing", quality = 500, installed = false)
        val chosen = selectVoice(listOf(online, missing, us, uk), SpeechProfile.BRITISH_MEASURED, Locale.US, null)
        assertEquals(uk, chosen?.voice)
        assertFalse(chosen!!.britishUnavailable)
        assertEquals(listOf(uk, us), availableVoices(listOf(us, online, uk, missing), SpeechProfile.BRITISH_MEASURED, Locale.US))
        assertTrue(SpeechProfile.BRITISH_MEASURED.rate < 1)
        assertTrue(SpeechProfile.BRITISH_MEASURED.pitch < 1)
    }
    @Test fun rankingUsesQualityAndStableTieBreaksAndExplicitSavedVoice() {
        val best = uk.copy(id = "best", quality = 500)
        val tie = best.copy(id = "other", latency = 200)
        assertEquals(best, selectVoice(listOf(tie, uk, best), SpeechProfile.BRITISH_MEASURED, Locale.US, null)?.voice)
        assertEquals(us, selectVoice(listOf(best, us), SpeechProfile.BRITISH_MEASURED, Locale.US, us.id)?.voice)
        assertEquals(best, selectVoice(listOf(best, tie), SpeechProfile.BRITISH_MEASURED, Locale.US, "removed")?.voice)
        assertTrue(selectVoice(listOf(best), SpeechProfile.BRITISH_MEASURED, Locale.US, "removed")!!.savedUnavailable)
    }
    @Test fun fallbackIsHonestAndNeverUsesNetworkOrUninstalledVoices() {
        assertTrue(selectVoice(listOf(us), SpeechProfile.BRITISH_MEASURED, Locale.US, null)!!.britishUnavailable)
        assertNull(selectVoice(listOf(uk.copy(network = true), uk.copy(installed = false)), SpeechProfile.BRITISH_MEASURED, Locale.US, null))
        val french = us.copy(id = "fr", locale = Locale.FRANCE)
        assertEquals(french, selectVoice(listOf(uk, french), SpeechProfile.SYSTEM, Locale.FRANCE, null)?.voice)
        assertNull(selectVoice(listOf(french), SpeechProfile.BRITISH_MEASURED, Locale.FRANCE, null))
    }
    @Test fun explicitGeminiCommandPreservesQuestionAndCommonVoicePunctuation() {
        for (input in listOf("ask Gemini Why is the sky blue?", "Agentix, ask Gemini: Why is the sky blue?", "AGENTIX ASK GEMINI Why is the sky blue?")) {
            assertEquals(GeminiRequest.Question("Why is the sky blue?"), geminiRequest(input))
        }
        assertEquals(GeminiRequest.Question("Explain café ☕\nwith examples."), geminiRequest("ask Gemini Explain café ☕\nwith examples."))
    }
    @Test fun ordinaryMentionsAndNotesNeverDelegate() {
        for (input in listOf("save a note: ask Gemini about recipes", "what is Gemini?", "ask Geminis a question", "please record ask Gemini hello", "Agentix save a task to ask Gemini tomorrow")) {
            assertEquals(GeminiRequest.NotRequested, geminiRequest(input))
        }
    }
    @Test fun missingAndOversizedQuestionsRemainLocalErrors() {
        assertEquals(GeminiRequest.MissingQuestion, geminiRequest("ask Gemini"))
        assertEquals(GeminiRequest.MissingQuestion, geminiRequest("Agentix: ask Gemini: "))
        assertEquals(GeminiRequest.TooLong, geminiRequest("ask Gemini ${"洋".repeat(2731)}"))
        assertTrue(geminiRequest("ask Gemini ${"x".repeat(8192)}") is GeminiRequest.Question)
        assertEquals(GeminiRequest.NotRequested, geminiRequest("ask Gemini explain gravity", hasPendingBackendTurn = true))
    }
}
