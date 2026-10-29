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
import us.disasm.agentix.core.assistant.DisconnectedAssistantClient
import us.disasm.agentix.core.assistant.TellCommand
import us.disasm.agentix.core.assistant.TurnPhase
import us.disasm.agentix.recording.CapturePhase
import us.disasm.agentix.recording.PhoneCaptureState

internal class AssistantController(
    private val context: Context,
    private val client: AssistantClient = DisconnectedAssistantClient,
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
    var message by mutableStateOf(R.string.assistant_idle)
        private set
    var speechStatus by mutableStateOf<Int?>(null)
        private set
    var speakReplies by mutableStateOf(preferences.getBoolean("speak_replies", true))
        private set
    private val speaker = OfflineSpeaker(context) { speechStatus = it }

    fun resume() { resumed = true }

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
        message = R.string.assistant_processing
        request = scope.launch {
            try {
                val result = client.tell(command)
                if (!resumed || !turn.complete(id)) return@launch
                phase = turn.phase
                reply = when (result) {
                    AssistantReply.NotConnected -> context.getString(R.string.assistant_not_connected)
                    is AssistantReply.Message -> result.text
                }
                message = if (result == AssistantReply.NotConnected) R.string.assistant_preview else R.string.assistant_reply
                if (speakReplies) speaker.speak(reply)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                if (turn.fail(id)) {
                    phase = turn.phase
                    message = R.string.assistant_request_failed
                }
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
    }

    fun close() {
        pause()
        speaker.close()
        scope.cancel()
    }
}
