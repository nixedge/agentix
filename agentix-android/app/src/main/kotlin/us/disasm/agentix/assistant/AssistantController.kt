package us.disasm.agentix.assistant

import android.Manifest
import android.app.KeyguardManager
import android.content.Context
import android.content.pm.PackageManager
import android.speech.SpeechRecognizer
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import us.disasm.agentix.R
import us.disasm.agentix.core.assistant.AssistantClient
import us.disasm.agentix.core.assistant.AssistantReply
import us.disasm.agentix.core.assistant.AssistantTurn
import us.disasm.agentix.core.assistant.TellCommand
import us.disasm.agentix.core.assistant.TurnPhase
import us.disasm.agentix.core.assistant.GeminiRequest
import us.disasm.agentix.core.assistant.geminiRequest
import us.disasm.agentix.core.assistant.SpeechProfile
import us.disasm.agentix.recording.CapturePhase
import us.disasm.agentix.recording.PhoneCaptureState
import us.disasm.agentix.wake.WakeWordRuntime

internal class AssistantController(
    private val context: Context,
    private val client: AssistantClient = BackendClient.get(context),
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val turn = AssistantTurn()
    private val preferences = context.getSharedPreferences("assistant_voice", Context.MODE_PRIVATE)
    private var recognition: OnDeviceRecognition? = null
    private var request: Job? = null
    private var resumed = false
    var phase by mutableStateOf(TurnPhase.IDLE)
        private set
    var draft by mutableStateOf("")
        private set
    var reply by mutableStateOf("")
        private set
    var pendingText by mutableStateOf<String?>(null)
        private set
    var message by mutableStateOf(R.string.assistant_idle)
        private set
    var speechStatus by mutableStateOf<Int?>(null)
        private set
    var speakReplies by mutableStateOf(preferences.getBoolean("speak_replies", true))
        private set
    private val speaker = AssistantSpeaker(context) { speechStatus = it }
    val speechOutput get() = speaker.output
    val canPreviewVoice get() = speaker.canPreview
    fun chooseSpeechOutput(output: us.disasm.agentix.core.assistant.SpeechOutput) {
        if (phase != TurnPhase.LISTENING && phase != TurnPhase.PROCESSING) { stopSpeaking(); speaker.choose(output) }
    }
    val voiceProfile get() = speaker.profile
    val voices get() = speaker.voices
    val selectedVoice get() = speaker.selected
    val voiceNotice get() = speaker.notice
    fun voiceLabel(voice: us.disasm.agentix.core.assistant.SpeechVoice) = speaker.label(voice)
    fun selectVoice(id: String) { if (phase != TurnPhase.LISTENING && phase != TurnPhase.PROCESSING) speaker.select(id) }
    fun updateVoiceProfile(profile: SpeechProfile) { if (phase != TurnPhase.LISTENING && phase != TurnPhase.PROCESSING) speaker.updateProfile(profile) }
    fun previewVoice() {
        if (canUse() && phase != TurnPhase.LISTENING && phase != TurnPhase.PROCESSING) speaker.speak(context.getString(R.string.assistant_voice_sample))
    }

    fun resume() {
        if (!resumed) speaker.refresh()
        resumed = true
        refreshBackend()
    }

    private fun refreshBackend() {
        try {
            val state = (client as? BackendClient)?.snapshot() ?: return
            pendingText = state.pending?.text
            if (phase != TurnPhase.LISTENING && phase != TurnPhase.PROCESSING) {
                if (pendingText != null) {
                    draft = pendingText.orEmpty()
                    message = R.string.assistant_pending
                } else {
                    state.lastResult?.let { reply = it.reply }
                }
            }
        } catch (_: Exception) {
            message = R.string.assistant_storage_failed
        }
    }

    fun retryPending() {
        refreshBackend()
        val text = pendingText ?: return
        draft = text
        submitTyped()
    }

    fun pause() {
        resumed = false
        cancel()
    }

    fun updateDraft(text: String) {
        if (phase != TurnPhase.LISTENING && phase != TurnPhase.PROCESSING) draft = text
    }

    fun updateSpeakReplies(enabled: Boolean) {
        speakReplies = enabled
        preferences.edit().putBoolean("speak_replies", enabled).apply()
        if (!enabled) stopSpeaking()
    }

    fun permissionResult(granted: Boolean) {
        message = if (granted) R.string.assistant_permission_granted else R.string.assistant_permission
    }

    fun listen() {
        if (!canUse()) return
        refreshBackend()
        if (pendingText != null) {
            message = R.string.assistant_pending
            return
        }
        if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            message = R.string.assistant_permission
            return
        }
        if (PhoneCaptureState.phase != CapturePhase.IDLE || AssistantMicrophone.active) {
            message = R.string.assistant_mic_busy
            return
        }
        if (!OnDeviceRecognition.available(context)) {
            message = R.string.assistant_recognizer_missing
            return
        }
        cancel()
        val id = turn.begin()
        phase = turn.phase
        draft = ""
        reply = ""
        message = R.string.assistant_listening
        val capture = OnDeviceRecognition(context, object : RecognitionEvents {
            override fun partial(text: String) {
                if (turn.partial(id, text)) draft = text
            }
            override fun ended() {
                if (id == turn.identity && phase == TurnPhase.LISTENING) message = R.string.assistant_transcribing
            }
            override fun result(text: String) {
                if (id != turn.identity) return
                recognition = null
                val command = turn.recognize(id, text)
                phase = turn.phase
                if (command == null) {
                    message = R.string.assistant_no_speech
                } else {
                    draft = command.text
                    dispatch(id, command)
                }
            }
            override fun error(code: Int) {
                if (!turn.fail(id)) return
                recognition = null
                phase = turn.phase
                message = when (code) {
                    SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS -> R.string.assistant_permission
                    SpeechRecognizer.ERROR_LANGUAGE_NOT_SUPPORTED, SpeechRecognizer.ERROR_LANGUAGE_UNAVAILABLE -> R.string.assistant_language_missing
                    SpeechRecognizer.ERROR_RECOGNIZER_BUSY -> R.string.assistant_mic_busy
                    SpeechRecognizer.ERROR_NO_MATCH, SpeechRecognizer.ERROR_SPEECH_TIMEOUT -> R.string.assistant_no_speech
                    else -> R.string.assistant_recognition_failed
                }
            }
        })
        recognition = capture
        capture.start()
    }

    fun stopListening() {
        if (phase != TurnPhase.LISTENING) return
        message = R.string.assistant_transcribing
        recognition?.stop()
    }

    fun submitTyped() {
        if (!canUse() || draft.isBlank()) return
        val text = draft
        cancel()
        val id = turn.begin()
        val command = turn.recognize(id, text) ?: return
        reply = ""
        dispatch(id, command)
    }

    private fun dispatch(id: Long, command: TellCommand) {
        phase = turn.phase
        when (val gemini = geminiRequest(command.text, pendingText != null)) {
            GeminiRequest.NotRequested -> Unit
            else -> {
                stopSpeaking()
                recognition?.close(); recognition = null
                val text = when (gemini) {
                    GeminiRequest.MissingQuestion -> R.string.assistant_gemini_question
                    GeminiRequest.TooLong -> R.string.assistant_gemini_too_long
                    is GeminiRequest.Question -> if (GeminiHandoff.open(context, gemini.text)) R.string.assistant_gemini_opened else R.string.assistant_gemini_unavailable
                    else -> R.string.assistant_gemini_unavailable
                }
                if (turn.complete(id)) {
                    phase = turn.phase; reply = context.getString(text); message = R.string.assistant_reply
                    if (speakReplies && text != R.string.assistant_gemini_opened) speaker.speak(reply)
                }
                return
            }
        }
        message = R.string.assistant_processing
        val wakeHold = Any()
        WakeWordRuntime.hold(wakeHold)
        request = scope.launch {
            try {
                val result = client.tell(command)
                refreshBackend()
                if (!resumed || !turn.complete(id)) return@launch
                phase = turn.phase
                reply = when (result) {
                    AssistantReply.NotConnected -> context.getString(R.string.assistant_not_connected)
                    is AssistantReply.Message -> result.text
                }
                message = if (result == AssistantReply.NotConnected) R.string.assistant_preview else R.string.assistant_reply
                if (speakReplies) speaker.speak(reply)
            } catch (cancelled: CancellationException) {
                refreshBackend()
                throw cancelled
            } catch (_: Exception) {
                if (turn.fail(id)) {
                    phase = turn.phase
                    message = R.string.assistant_request_failed
                    refreshBackend()
                }
            } finally {
                WakeWordRuntime.release(wakeHold)
            }
        }
    }

    private fun canUse(): Boolean {
        if (!resumed) return false
        if (context.getSystemService(KeyguardManager::class.java).isDeviceLocked) {
            message = R.string.assistant_unlock
            return false
        }
        return true
    }

    fun stopSpeaking() {
        speaker.stop()
        speechStatus = null
    }

    fun cancel() {
        if (phase == TurnPhase.LISTENING) draft = ""
        turn.cancel()
        recognition?.close()
        recognition = null
        request?.cancel()
        request = null
        stopSpeaking()
        phase = turn.phase
        message = R.string.assistant_idle
        refreshBackend()
    }

    fun close() {
        pause()
        speaker.close()
        scope.cancel()
    }
}
