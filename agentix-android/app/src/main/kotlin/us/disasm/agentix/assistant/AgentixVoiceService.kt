package us.disasm.agentix.assistant

import android.content.Intent
import android.os.Bundle
import android.service.voice.VoiceInteractionService
import android.service.voice.VoiceInteractionSession
import android.service.voice.VoiceInteractionSessionService
import android.widget.Toast
import us.disasm.agentix.R

class AgentixVoiceService : VoiceInteractionService() {
    override fun onReady() {
        super.onReady()
        setDisabledShowContext(VoiceInteractionSession.SHOW_WITH_ASSIST or VoiceInteractionSession.SHOW_WITH_SCREENSHOT)
        current = this
    }

    override fun onShutdown() {
        if (current === this) current = null
        super.onShutdown()
    }

    override fun onDestroy() {
        if (current === this) current = null
        super.onDestroy()
    }

    companion object {
        private var current: AgentixVoiceService? = null
        internal const val WAKE = "agentix_wake_word"
        internal fun showWakeSession(): Boolean {
            val service = current ?: return false
            return try {
                service.showSession(Bundle().apply { putBoolean(WAKE, true) }, 0)
                true
            } catch (_: RuntimeException) { false }
        }
    }
}

class AgentixSessionService : VoiceInteractionSessionService() {
    override fun onNewSession(args: Bundle?): VoiceInteractionSession = AgentixSession(this)
}

private class AgentixSession(context: android.content.Context) : VoiceInteractionSession(context) {
    override fun onPrepareShow(args: Bundle?, showFlags: Int) {
        super.onPrepareShow(args, showFlags)
        setUiEnabled(false)
        setDisabledShowContext(SHOW_WITH_ASSIST or SHOW_WITH_SCREENSHOT)
    }

    override fun onShow(args: Bundle?, showFlags: Int) {
        super.onShow(args, showFlags)
        try {
            startAssistantActivity(
                Intent(context, AssistantActivity::class.java)
                    .setAction(AssistantActivity.ACTION_VOICE_TURN)
                    .putExtra(AgentixVoiceService.WAKE, args?.getBoolean(AgentixVoiceService.WAKE, false) == true)
                    .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            )
        } catch (_: RuntimeException) {
            Toast.makeText(context, R.string.assistant_launch_failed, Toast.LENGTH_LONG).show()
        } finally {
            hide()
        }
    }
}
