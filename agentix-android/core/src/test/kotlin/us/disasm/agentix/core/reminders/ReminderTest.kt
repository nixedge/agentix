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
    @Test fun dismissalSurvivesReplayAndCannotRearmOrRegressToPosted() {
        val ringing = initial().transition(reminder.id, 1, Delivery.POST_PENDING).transition(reminder.id, 1, Delivery.RINGING)
        assertTrue(ringing.eligible(reminder.id, 1, now.minusSeconds(3600)))
        val dismissed = ringing.dismiss(server, listOf(AlarmTarget(reminder.id, 1, 0)), now)
        assertEquals(Delivery.DISMISSED, dismissed.entries.single().phase)
        assertFalse(dismissed.eligible(reminder.id, 1, now.plusSeconds(86400)))
        assertEquals(dismissed, dismissed.dismiss(server, listOf(AlarmTarget(reminder.id, 1, 0)), now))
        val stale = Observation(UUID.randomUUID(), reminder.id, 1, 1, Delivery.POSTED)
        val replay = dismissed.apply(Snapshot(server, device, 1, listOf(reminder), listOf(stale)))
        assertEquals(Delivery.DISMISSED, replay.entries.single().phase)
        assertEquals(Delivery.DISMISSED, replay.reports.single().state)
        assertThrows(IllegalArgumentException::class.java) { replay.transition(reminder.id, 1, Delivery.ARMED) }
        val observed = Observation(UUID.randomUUID(), reminder.id, 1, 100, Delivery.DISMISSED)
        val restored = initial().transition(reminder.id, 1, Delivery.BLOCKED).apply(Snapshot(server, device, 1, listOf(reminder), listOf(observed)))
        assertTrue(restored.reports.isEmpty())
        assertFalse(restored.eligible(reminder.id, 1, now))
    }
    @Test fun staleDismissalCannotDismissAnotherNamespaceRevisionOrUpcomingAlarm() {
        val ringing = initial().transition(reminder.id, 1, Delivery.RINGING)
        assertEquals(ringing, ringing.dismiss(UUID.randomUUID(), listOf(AlarmTarget(reminder.id, 1, 0)), now))
        val revised = reminder.copy(revision = 2, dueAt = now.plusSeconds(3600))
        val updated = ringing.apply(Snapshot(server, device, 2, listOf(revised), emptyList()))
        assertEquals(updated, updated.dismiss(server, listOf(AlarmTarget(reminder.id, 1, 0)), now))
        val armed = updated.transition(reminder.id, 2, Delivery.ARMED)
        assertEquals(armed, armed.dismiss(server, listOf(AlarmTarget(reminder.id, 2, 0)), now))
        val cancelled = ringing.apply(Snapshot(server, device, 2, listOf(reminder.copy(revision = 2, cancelled = true)), emptyList()))
        assertFalse(cancelled.eligible(reminder.id, 1, now))
        assertEquals(cancelled, cancelled.dismiss(server, listOf(AlarmTarget(reminder.id, 1, 0)), now))
    }
    @Test fun concurrentAlarmsDismissIndividuallyOrAsExactCapturedSet() {
        val second = reminder.copy(id = UUID.randomUUID())
        val both = Journal("https://ai.lan.disasm.us", device).apply(Snapshot(server, device, 1, listOf(reminder, second), emptyList()))
            .transition(reminder.id, 1, Delivery.RINGING).transition(second.id, 1, Delivery.RINGING)
        val firstDismissed = both.dismiss(server, listOf(AlarmTarget(reminder.id, 1, 0)), now)
        assertFalse(firstDismissed.eligible(reminder.id, 1, now))
        assertTrue(firstDismissed.eligible(second.id, 1, now))
        val all = firstDismissed.dismiss(server, listOf(AlarmTarget(reminder.id, 1, 0), AlarmTarget(second.id, 1, 0)), now)
        assertTrue(all.entries.all { it.phase == Delivery.DISMISSED })
        assertTrue(all.reports.all { it.state == Delivery.DISMISSED })
    }
    @Test fun restoredRingingObservationRecoversAfterClockRollbackButNeverRevivesDismissal() {
        val observation = Observation(UUID.randomUUID(), reminder.id, 1, 100, Delivery.RINGING)
        val restored = initial().apply(Snapshot(server, device, 1, listOf(reminder), listOf(observation)))
        assertTrue(restored.eligible(reminder.id, 1, now.minusSeconds(3600)))
        val failed = restored.transition(reminder.id, 1, Delivery.UNCERTAIN)
        assertEquals(Delivery.UNCERTAIN, failed.apply(Snapshot(server, device, 1, listOf(reminder), listOf(observation))).entries.single().phase)
        val dismissed = restored.dismiss(server, listOf(AlarmTarget(reminder.id, 1, 0)), now)
        assertEquals(Delivery.DISMISSED, dismissed.apply(Snapshot(server, device, 1, listOf(reminder), listOf(observation))).entries.single().phase)
    }
    @Test fun snoozePersistsExactDeadlineAndOccurrenceAcrossRecoveryAndSnapshots() {
        val ringing = initial().transition(reminder.id, 1, Delivery.RINGING)
        val target = ringing.entries.single().target()
        val snoozed = ringing.snooze(server, listOf(target), now)
        val entry = snoozed.entries.single()
        assertEquals(now.plusSeconds(600), entry.snoozeUntil)
        assertEquals(1L, entry.generation)
        assertEquals(Delivery.ALARM_INTENT, entry.phase)
        assertFalse(snoozed.eligible(entry.target(), now.plusSeconds(599)))
        assertFalse(snoozed.eligible(target, now.plusSeconds(600)))
        assertTrue(snoozed.eligible(entry.target(), now.plusSeconds(600)))
        val rebuilt = snoozed.copy(entries = snoozed.entries.map { it.copy() })
        val ringingObservation = Observation(UUID.randomUUID(), reminder.id, 1, 1, Delivery.RINGING)
        val synced = rebuilt.apply(Snapshot(server, device, 1, listOf(reminder), listOf(ringingObservation)))
        assertEquals(entry, synced.entries.single())
        val blocked = synced.transition(reminder.id, 1, Delivery.BLOCKED)
        assertEquals(entry.snoozeUntil, blocked.entries.single().deadline())
        val armed = blocked.transition(reminder.id, 1, Delivery.ALARM_INTENT).transition(reminder.id, 1, Delivery.ARMED)
        assertEquals(Delivery.ARMED, armed.reports.single().state)
        assertEquals(entry.snoozeUntil, armed.entries.single().snoozeUntil)
        val firing = armed.transition(reminder.id, 1, Delivery.POST_PENDING).transition(reminder.id, 1, Delivery.RINGING)
        assertNull(firing.entries.single().snoozeUntil)
        assertEquals(1L, firing.entries.single().generation)
        val starting = armed.transition(reminder.id, 1, Delivery.POST_PENDING)
        assertEquals(Delivery.POST_PENDING, starting.apply(Snapshot(server, device, 1, listOf(reminder), listOf(ringingObservation))).entries.single().phase)
    }
    @Test fun staleSnoozeAndDismissCannotExtendOrEndNextAlarmOccurrence() {
        val ringing = initial().transition(reminder.id, 1, Delivery.RINGING)
        val target = ringing.entries.single().target()
        assertEquals(ringing, ringing.snooze(UUID.randomUUID(), listOf(target), now))
        val snoozed = ringing.snooze(server, listOf(target), now)
        assertEquals(snoozed, snoozed.snooze(server, listOf(target), now.plusSeconds(30)))
        assertEquals(snoozed, snoozed.dismiss(server, listOf(target), now))
        val nextRing = snoozed.transition(reminder.id, 1, Delivery.POST_PENDING).transition(reminder.id, 1, Delivery.RINGING)
        assertEquals(nextRing, nextRing.snooze(server, listOf(target), now.plusSeconds(600)))
        assertEquals(nextRing, nextRing.dismiss(server, listOf(target), now.plusSeconds(600)))
        val twice = nextRing.snooze(server, listOf(nextRing.entries.single().target()), now.plusSeconds(600))
        assertEquals(2L, twice.entries.single().generation)
        assertEquals(now.plusSeconds(1200), twice.entries.single().snoozeUntil)
    }
    @Test fun dismissCancelAndRescheduleInvalidatePendingSnooze() {
        val snoozed = initial().transition(reminder.id, 1, Delivery.RINGING).snooze(server, listOf(AlarmTarget(reminder.id, 1, 0)), now)
        val target = snoozed.entries.single().target()
        val dismissed = snoozed.dismiss(server, listOf(target), now)
        assertEquals(Delivery.DISMISSED, dismissed.entries.single().phase)
        assertNull(dismissed.entries.single().snoozeUntil)
        assertFalse(dismissed.eligible(target, now.plusSeconds(600)))
        val cancelled = snoozed.apply(Snapshot(server, device, 2, listOf(reminder.copy(revision = 2, cancelled = true)), emptyList()))
        assertNull(cancelled.entries.single().snoozeUntil)
        assertFalse(cancelled.eligible(target, now.plusSeconds(600)))
        val moved = snoozed.apply(Snapshot(server, device, 2, listOf(reminder.copy(revision = 2, dueAt = now.plusSeconds(3600))), emptyList()))
        assertNull(moved.entries.single().snoozeUntil)
        assertFalse(moved.eligible(target, now.plusSeconds(600)))
        val backendDismissed = Observation(UUID.randomUUID(), reminder.id, 1, 20, Delivery.DISMISSED)
        assertEquals(Delivery.DISMISSED, snoozed.apply(Snapshot(server, device, 1, listOf(reminder), listOf(backendDismissed))).entries.single().phase)
    }
    @Test fun snoozeAllOnlyChangesCapturedActiveSetAndRejectsInvalidMetadata() {
        val second = reminder.copy(id = UUID.randomUUID())
        val both = initial().apply(Snapshot(server, device, 2, listOf(reminder, second), emptyList()))
            .transition(reminder.id, 1, Delivery.RINGING).transition(second.id, 1, Delivery.RINGING)
        val one = both.snooze(server, listOf(both.entries.first().target()), now)
        assertEquals(1, one.entries.count { it.snoozeUntil != null })
        assertEquals(1, one.entries.count { it.phase == Delivery.RINGING })
        val all = both.snooze(server, both.entries.map { it.target() }, now)
        assertTrue(all.entries.all { it.snoozeUntil == now.plusSeconds(600) })
        assertEquals(initial(), initial().snooze(server, listOf(AlarmTarget(reminder.id, 1, 0)), now))
        assertThrows(IllegalArgumentException::class.java) { Entry(reminder, generation = -1) }
        assertThrows(IllegalArgumentException::class.java) { Entry(reminder, snoozeUntil = now) }
    }
    @Test fun journalEntryMigrationPreservesLegacyStatesAndRoundTripsSnoozeMetadata() {
        for (phase in listOf(Delivery.ALARM_INTENT, Delivery.ARMED, Delivery.RINGING, Delivery.POSTED, Delivery.DISMISSED)) {
            val restored = Entry.restore(1, reminder, phase, phase, null, null)
            assertEquals(Entry(reminder, phase, phase), restored)
            assertEquals(0L, restored.generation)
            assertNull(restored.snoozeUntil)
        }
        val entry = Entry(reminder, Delivery.ARMED, Delivery.ARMED, 3, now.plusSeconds(600))
        assertEquals(entry, Entry.restore(2, entry.reminder, entry.phase, entry.reported, entry.generation, entry.snoozeUntil))
        assertThrows(IllegalArgumentException::class.java) { Entry.restore(3, reminder, Delivery.ARMED, null, null, null) }
        assertThrows(IllegalArgumentException::class.java) { Entry.restore(2, reminder, Delivery.ARMED, null, null, now) }
        assertThrows(IllegalArgumentException::class.java) { Entry.restore(2, reminder, Delivery.ARMED, null, -1, now) }
    }
}
