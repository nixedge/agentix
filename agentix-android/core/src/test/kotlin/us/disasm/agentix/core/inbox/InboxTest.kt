package us.disasm.agentix.core.inbox

import org.junit.Assert.*
import org.junit.Test
import us.disasm.agentix.core.reminders.*
import java.time.Instant
import java.util.UUID

class InboxTest {
    private val now = Instant.parse("2026-11-01T12:00:00Z")
    private val server = UUID.randomUUID()
    private val reminder = Reminder(UUID.randomUUID(), 1, "laundry", now, "UTC", false)
    private val voice = InboxItem(recordingKey("123-voice.wav"), InboxKind.RECORDING, now.minusSeconds(10))
    private fun item(entry: Entry) = InboxItem(reminderKey(server, entry.reminder.id, entry.reminder.revision), InboxKind.REMINDER, entry.deadline(), entry.firing(), entry.reminder.cancelled || entry.phase.completed())
    @Test fun unifiedFeedContainsBothKindsAndArchivesWithoutDeletingItems() {
        val reminderItem = item(Entry(reminder, Delivery.ARMED))
        val original = listOf(voice, reminderItem)
        assertEquals(listOf(reminderItem, voice), inboxItems(original, Archives(), false))
        val archives = Archives().change(voice, true).change(reminderItem, true)
        assertTrue(inboxItems(original, archives, false).isEmpty())
        assertEquals(original.toSet(), inboxItems(original, archives, true).toSet())
        val restored = archives.change(voice, false)
        assertEquals(listOf(voice), inboxItems(original, restored, false))
        assertEquals(2, original.size)
        assertEquals(archives, Archives(archives.keys.toSet()))
    }
    @Test fun dismissedCancelledAndPostedHistoryNeverReturnToInbox() {
        for (entry in listOf(Entry(reminder, Delivery.DISMISSED), Entry(reminder, Delivery.POSTED), Entry(reminder.copy(cancelled = true), Delivery.ALARM_INTENT))) {
            val row = item(entry)
            assertTrue(inboxItems(listOf(row), Archives(), false).isEmpty())
            assertEquals(listOf(row), inboxItems(listOf(row), Archives(), true))
            assertThrows(IllegalArgumentException::class.java) { Archives().change(row, false) }
        }
    }
    @Test fun firingAlarmsOverrideArchiveAndRejectStaleArchiveActions() {
        val pending = item(Entry(reminder, Delivery.ARMED))
        val archives = Archives().change(pending, true)
        for (phase in listOf(Delivery.POST_PENDING, Delivery.RINGING)) {
            val active = item(Entry(reminder, phase))
            assertEquals(listOf(active), inboxItems(listOf(voice, active), archives, false).filter { it.kind == InboxKind.REMINDER })
            assertTrue(inboxItems(listOf(active), archives, true).isEmpty())
            assertThrows(IllegalArgumentException::class.java) { archives.change(active, true) }
        }
    }
    @Test fun snoozedAndBlockedEntriesRemainArchivableWithoutChangingDeadlines() {
        val snoozed = Entry(reminder, Delivery.ARMED, generation = 1, snoozeUntil = now.plusSeconds(600))
        for (entry in listOf(snoozed, snoozed.copy(phase = Delivery.BLOCKED), Entry(reminder, Delivery.UNCERTAIN))) {
            val row = item(entry)
            val archived = Archives().change(row, true)
            assertTrue(row.inArchive(archived))
            assertEquals(entry.deadline(), row.time)
            assertFalse(row.inArchive(archived.change(row, false)))
        }
        assertEquals(now.plusSeconds(600), snoozed.snoozeUntil)
    }
    @Test fun namespaceRevisionAndRecordingKeysCannotCrossArchiveBoundaries() {
        val row = item(Entry(reminder))
        val archives = Archives().change(row, true)
        assertFalse(row.copy(key = reminderKey(UUID.randomUUID(), reminder.id, 1)).inArchive(archives))
        assertFalse(item(Entry(reminder.copy(revision = 2))).inArchive(archives))
        assertFalse(voice.inArchive(archives))
        assertThrows(IllegalArgumentException::class.java) { recordingKey("../recording.wav") }
        assertThrows(IllegalArgumentException::class.java) { Archives(setOf("bad\nkey")) }
        assertThrows(IllegalArgumentException::class.java) { inboxItems(listOf(row, row), Archives(), false) }
    }
    @Test fun activeAlarmsPrecedeSelectionAndOtherEntriesHaveStableTimeOrder() {
        val active = item(Entry(reminder, Delivery.RINGING))
        val selected = voice.copy(time = now.plusSeconds(3600))
        val other = voice.copy(key = recordingKey("456-other.wav"), time = now.plusSeconds(1200))
        assertEquals(listOf(active, selected, other), inboxItems(listOf(other, active, selected), Archives(), false, selected.key))
    }
    @Test fun unavailableRegistryPreservesActiveControlsWithoutSilentlyUnarchivingOtherItems() {
        val active = item(Entry(reminder, Delivery.RINGING))
        val dismissed = item(Entry(reminder.copy(id = UUID.randomUUID()), Delivery.DISMISSED))
        val pending = item(Entry(reminder.copy(id = UUID.randomUUID()), Delivery.ARMED))
        val all = listOf(voice, pending, active, dismissed)
        assertEquals(listOf(active), inboxItems(all, null, false))
        assertEquals(listOf(dismissed), inboxItems(all, null, true))
    }
}
