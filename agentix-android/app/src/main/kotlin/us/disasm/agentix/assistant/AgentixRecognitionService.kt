package us.disasm.agentix.assistant

import android.content.Intent
import android.os.Bundle
import android.os.RemoteException
import android.speech.RecognitionService
import android.speech.SpeechRecognizer
import us.disasm.agentix.recording.CapturePhase
import us.disasm.agentix.recording.PhoneCaptureState

class AgentixRecognitionService : RecognitionService() {
    private var recognition: OnDeviceRecognition? = null

    override fun onStartListening(recognizerIntent: Intent, listener: Callback) {
        if (recognition != null || PhoneCaptureState.phase != CapturePhase.IDLE || AssistantMicrophone.active) {
            deliver { listener.error(SpeechRecognizer.ERROR_RECOGNIZER_BUSY) }
            return
        }
        val events = object : RecognitionEvents {
            override fun ready() = deliver { listener.readyForSpeech(Bundle()) }
            override fun partial(text: String) = deliver { listener.partialResults(results(text)) }
            override fun ended() = deliver { listener.endOfSpeech() }
            override fun result(text: String) {
                recognition = null
                deliver { listener.results(results(text)) }
            }
            override fun error(code: Int) {
                recognition = null
                deliver { listener.error(code) }
            }
        }
        val capture = OnDeviceRecognition(this, events)
        recognition = capture
        capture.start(recognizerIntent)
    }

    override fun onStopListening(listener: Callback) { recognition?.stop() }
    override fun onCancel(listener: Callback) {
        recognition?.close()
        recognition = null
    }

    override fun onDestroy() {
        recognition?.close()
        recognition = null
        super.onDestroy()
    }

    private fun results(text: String) = Bundle().apply {
        putStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION, arrayListOf(text))
    }

    private fun deliver(block: () -> Unit) {
        try { block() } catch (_: RemoteException) {
            recognition?.close()
            recognition = null
        }
    }
}

internal object AssistantMicrophone {
    private var owner: Any? = null
    val active: Boolean get() = owner != null

    fun acquire(requester: Any): Boolean {
        if (owner != null || PhoneCaptureState.phase != CapturePhase.IDLE) return false
        owner = requester
        return true
    }

    fun release(requester: Any) {
        if (owner === requester) owner = null
    }
}
