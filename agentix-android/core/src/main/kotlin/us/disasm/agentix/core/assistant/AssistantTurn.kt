package us.disasm.agentix.core.assistant

data class TellCommand(val text: String) {
    init { require(text.isNotBlank()) { "An instruction must not be blank" } }
}

sealed interface AssistantReply {
    data class Message(val text: String) : AssistantReply
    data object NotConnected : AssistantReply
}

fun interface AssistantClient {
    suspend fun tell(command: TellCommand): AssistantReply
}

object DisconnectedAssistantClient : AssistantClient {
    override suspend fun tell(command: TellCommand): AssistantReply = AssistantReply.NotConnected
}

enum class TurnPhase { IDLE, LISTENING, PROCESSING, READY, ERROR }

class AssistantTurn {
    var identity: Long = 0
        private set
    var phase: TurnPhase = TurnPhase.IDLE
        private set
    var transcript: String = ""
        private set

    fun begin(): Long {
        identity++
        phase = TurnPhase.LISTENING
        transcript = ""
        return identity
    }

    fun partial(id: Long, text: String): Boolean {
        if (id != identity || phase != TurnPhase.LISTENING) return false
        transcript = text
        return true
    }

    fun recognize(id: Long, text: String): TellCommand? {
        if (id != identity || phase != TurnPhase.LISTENING) return null
        if (text.isBlank()) {
            phase = TurnPhase.ERROR
            return null
        }
        transcript = text
        phase = TurnPhase.PROCESSING
        return TellCommand(text)
    }

    fun complete(id: Long): Boolean {
        if (id != identity || phase != TurnPhase.PROCESSING) return false
        phase = TurnPhase.READY
        return true
    }

    fun fail(id: Long): Boolean {
        if (id != identity || phase !in setOf(TurnPhase.LISTENING, TurnPhase.PROCESSING)) return false
        phase = TurnPhase.ERROR
        return true
    }

    fun cancel() {
        identity++
        phase = TurnPhase.IDLE
    }
}
