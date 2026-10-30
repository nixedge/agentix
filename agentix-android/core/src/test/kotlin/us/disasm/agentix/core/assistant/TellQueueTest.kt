package us.disasm.agentix.core.assistant

import java.util.UUID
import org.junit.Assert.*
import org.junit.Test

class TellQueueTest {
    private class Memory : TellJournal {
        var state = TellQueueState(UUID.randomUUID())
        var fail = false
        override fun load() = state
        override fun store(state: TellQueueState) {
            check(!fail) { "storage unavailable" }
            this.state = state
        }
    }
    private val endpoint = "https://ai.lan.disasm.us"
    private fun response(input: TellEnvelope, status: String = "saved", outcomes: Int = 1) =
        TellResult(input.requestId, input.conversationId, status, "Saved your note.", outcomes, 0)

    @Test fun enrollmentChangesCannotRetargetAnUnresolvedEnvelope() {
        val disk = Memory()
        val legacy = TellQueue(disk).begin("Save a note.", endpoint, "UTC")
        val device = UUID.randomUUID()
        assertEquals(legacy, TellQueue(disk).begin(legacy.text, endpoint, "Europe/London", device))
        TellQueue(disk).complete(legacy, response(legacy))
        val modern = TellQueue(disk).begin("Remind me in ten minutes.", endpoint, "UTC", device)
        assertEquals(device, modern.deviceId)
        assertEquals(modern, TellQueue(disk).begin(modern.text, endpoint, "Europe/London"))
    }

    @Test fun uncertainOutcomeAndRecreationKeepTheEntireEnvelope() {
        val disk = Memory()
        val original = TellQueue(disk).begin("Add pineapple.", endpoint, "America/New_York")
        val recreated = TellQueue(disk)
        assertEquals(original, recreated.begin("Add pineapple.", endpoint, "Europe/London"))
        assertThrows(IllegalStateException::class.java) { recreated.begin("Add mango.", endpoint, "UTC") }
        assertThrows(IllegalStateException::class.java) { recreated.begin("Add pineapple.", "https://other.invalid", "UTC") }
        recreated.complete(original, response(original))
        assertNull(disk.state.pending)
        assertEquals(response(original), disk.state.lastResult)
        val next = TellQueue(disk).begin("Add pineapple.", endpoint, "UTC")
        assertNotEquals(original.requestId, next.requestId)
        assertEquals(original.conversationId, next.conversationId)
    }

    @Test fun mismatchedOrUnknownRepliesCannotReleaseThePendingRequest() {
        val disk = Memory()
        val queue = TellQueue(disk)
        val original = queue.begin("Save a note.", endpoint, "UTC")
        assertThrows(IllegalArgumentException::class.java) {
            queue.complete(original, response(original).copy(requestId = UUID.randomUUID()))
        }
        assertThrows(IllegalArgumentException::class.java) {
            queue.complete(original, response(original).copy(conversationId = UUID.randomUUID()))
        }
        assertThrows(IllegalArgumentException::class.java) { response(original, "unknown") }
        assertThrows(IllegalArgumentException::class.java) { response(original, "saved", 0) }
        assertThrows(IllegalArgumentException::class.java) { response(original, "answer", 1) }
        assertEquals(original, disk.state.pending)
        queue.complete(original, response(original, "failed", 0))
        assertNull(disk.state.pending)
    }

    @Test fun persistenceFailureCannotDispatchOrReleaseARequest() {
        val disk = Memory()
        val queue = TellQueue(disk)
        disk.fail = true
        assertThrows(IllegalStateException::class.java) { queue.begin("Save a note.", endpoint, "UTC") }
        assertNull(disk.state.pending)
        disk.fail = false
        val original = queue.begin("Save a note.", endpoint, "UTC")
        disk.fail = true
        assertThrows(IllegalStateException::class.java) { queue.complete(original, response(original)) }
        assertEquals(original, disk.state.pending)
    }

    @Test fun textBytesTimezoneAndTransportAreBounded() {
        val queue = TellQueue(Memory())
        assertThrows(IllegalArgumentException::class.java) { queue.begin("é".repeat(2049), endpoint, "UTC") }
        assertThrows(IllegalArgumentException::class.java) { queue.begin("note", "http://ai.lan.disasm.us", "UTC") }
        assertThrows(java.time.DateTimeException::class.java) { queue.begin("note", endpoint, "bad/zone") }
        assertThrows(IllegalArgumentException::class.java) { queue.begin("note", endpoint, "+02:00") }
    }
}
