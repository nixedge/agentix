package us.disasm.agentix.core.assistant

import java.time.ZoneId
import java.util.UUID
import us.disasm.agentix.core.AssistantEndpoint

data class TellEnvelope(
    val requestId: UUID,
    val conversationId: UUID,
    val text: String,
    val timezone: String,
    val endpoint: String,
) {
    init {
        require(requestId != UUID(0, 0) && conversationId != UUID(0, 0))
        require(text.isNotBlank() && text.toByteArray(Charsets.UTF_8).size <= 4096)
        require(timezone.length <= 64)
        ZoneId.of(timezone)
        require(timezone in ZoneId.getAvailableZoneIds())
        require(AssistantEndpoint.normalize(endpoint) == endpoint)
    }
}

data class TellResult(
    val requestId: UUID,
    val conversationId: UUID,
    val status: String,
    val reply: String,
    val outcomes: Int,
    val records: Int,
) {
    init {
        require(requestId != UUID(0, 0) && conversationId != UUID(0, 0))
        require(reply.isNotBlank() && reply.toByteArray(Charsets.UTF_8).size <= 8192)
        require(outcomes in 0..2 && records in 0..50)
        when (status) {
            "saved", "partial" -> require(outcomes > 0)
            "answer", "clarification", "failed" -> require(outcomes == 0)
            else -> throw IllegalArgumentException("Unknown assistant response")
        }
    }
}

data class TellQueueState(
    val conversationId: UUID,
    val pending: TellEnvelope? = null,
    val lastResult: TellResult? = null,
) {
    init {
        require(conversationId != UUID(0, 0))
        require(pending == null || pending.conversationId == conversationId)
        require(lastResult == null || lastResult.conversationId == conversationId)
    }
}

interface TellJournal {
    fun load(): TellQueueState
    // Return only after the entire state is durably replaced, or throw.
    fun store(state: TellQueueState)
}

/** The host serializes admission, configuration changes and completion. */
class TellQueue(private val journal: TellJournal) {
    fun begin(text: String, endpoint: String, timezone: String): TellEnvelope {
        val state = journal.load()
        state.pending?.let {
            check(it.text == text && it.endpoint == endpoint) { "Resolve the pending instruction first" }
            return it
        }
        val envelope = TellEnvelope(UUID.randomUUID(), state.conversationId, text, timezone, endpoint)
        journal.store(state.copy(pending = envelope))
        return envelope
    }

    fun complete(envelope: TellEnvelope, result: TellResult) {
        val state = journal.load()
        check(state.pending == envelope) { "Pending instruction changed" }
        require(result.requestId == envelope.requestId && result.conversationId == envelope.conversationId)
        journal.store(state.copy(pending = null, lastResult = result))
    }
}
