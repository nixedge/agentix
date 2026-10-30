package us.disasm.agentix.core.reminders

import org.junit.Assert.*
import org.junit.Test
import java.time.Instant
import java.util.UUID

class ReminderTest {
    private val device = UUID.randomUUID()
    private val server = UUID.randomUUID()
    private val now = Instant.parse("2026-10-30T12:00:00Z")
    private val reminder = Reminder(UUID.randomUUID(), 1, "laundry", now, "UTC", false)
    private fun initial() = Journal("https://ai.lan.disasm.us", device).apply(Snapshot(server, device, 1, listOf(reminder), emptyList()))
    @Test fun durableIntentSupportsCrashRecoveryAndStableRevision() {
        val state = initial().transition(reminder.id, 1, Delivery.ALARM_INTENT)
        assertTrue(state.eligible(reminder.id, 1, now))
        val armed = state.transition(reminder.id, 1, Delivery.ARMED)
        val again = armed.transition(reminder.id, 1, Delivery.ALARM_INTENT).transition(reminder.id, 1, Delivery.ARMED)
        assertEquals(armed.reports, again.reports)
        val posting = again.transition(reminder.id, 1, Delivery.POST_PENDING)
        assertTrue(posting.eligible(reminder.id, 1, now))
        val posted = posting.transition(reminder.id, 1, Delivery.POSTED)
        assertFalse(posted.eligible(reminder.id, 1, now))
        assertThrows(IllegalArgumentException::class.java) { posted.transition(reminder.id, 1, Delivery.ALARM_INTENT) }
        assertEquals(1, posted.reports.size)
    }
    @Test fun appliedCancellationSuppressesStaleAlarmAndSupersedesReports() {
        val armed = initial().transition(reminder.id, 1, Delivery.ARMED)
        val cancelled = reminder.copy(revision = 2, cancelled = true)
        val updated = armed.apply(Snapshot(server, device, 2, listOf(cancelled), emptyList()))
        assertFalse(updated.eligible(reminder.id, 1, now))
        assertTrue(updated.reports.isEmpty())
        val done = updated.transition(reminder.id, 2, Delivery.CANCEL_APPLIED)
        assertEquals(Delivery.CANCEL_APPLIED, done.reports.single().state)
        assertThrows(IllegalArgumentException::class.java) { done.transition(reminder.id, 1, Delivery.ARMED) }
    }
    @Test fun malformedOrStaleSnapshotsCannotClearSchedules() {
        val state = initial()
        assertThrows(IllegalArgumentException::class.java) { state.apply(Snapshot(server, device, 0, emptyList(), emptyList())) }
        assertThrows(IllegalArgumentException::class.java) { state.apply(Snapshot(server, device, 2, emptyList(), emptyList())) }
        assertThrows(IllegalArgumentException::class.java) { state.apply(Snapshot(UUID.randomUUID(), device, 2, listOf(reminder), emptyList())) }
        assertThrows(IllegalArgumentException::class.java) { state.apply(Snapshot(server, UUID.randomUUID(), 2, listOf(reminder), emptyList())) }
        assertThrows(IllegalArgumentException::class.java) { state.apply(Snapshot(server, device, 1, listOf(reminder.copy(text = "changed")), emptyList())) }
        assertTrue(state.eligible(reminder.id, 1, now))
    }
    @Test fun postedBackendObservationPreventsRealertAndAdvancesSequence() {
        val observation = Observation(UUID.randomUUID(), reminder.id, 1, 100, Delivery.POSTED)
        val state = Journal("https://ai.lan.disasm.us", device).apply(Snapshot(server, device, 2, listOf(reminder), listOf(observation)))
        assertFalse(state.eligible(reminder.id, 1, now))
        assertEquals(101L, state.nextReport)
    }
    @Test fun restoredPendingObservationsCannotRegressBackendPosting() {
        val oldCache = initial().copy(nextReport = 200).transition(reminder.id, 1, Delivery.BLOCKED)
        val posted = Observation(UUID.randomUUID(), reminder.id, 1, 100, Delivery.POSTED)
        val restored = oldCache.apply(Snapshot(server, device, 1, listOf(reminder), listOf(posted)))
        assertFalse(restored.eligible(reminder.id, 1, now))
        assertTrue(restored.reports.isEmpty())
        assertEquals(201L, restored.nextReport)
    }
    @Test fun acknowledgementCannotEraseNewerPendingReport() {
        val armed = initial().transition(reminder.id, 1, Delivery.ARMED)
        val id = armed.reports.single().reportId
        val posted = armed.transition(reminder.id, 1, Delivery.POST_PENDING).transition(reminder.id, 1, Delivery.POSTED)
        assertEquals(posted, posted.acknowledge(setOf(id)))
        assertTrue(posted.acknowledge(posted.reports.map { it.reportId }.toSet()).reports.isEmpty())
    }
    @Test fun invalidFieldsAndEarlyAlarmsAreRejected() {
        assertFalse(initial().eligible(reminder.id, 1, now.minusSeconds(1)))
        assertThrows(IllegalArgumentException::class.java) { reminder.copy(revision = 0) }
        assertThrows(IllegalArgumentException::class.java) { reminder.copy(text = " ") }
        assertThrows(IllegalArgumentException::class.java) { Snapshot(server, device, 1, listOf(reminder, reminder), emptyList()) }
    }
}
