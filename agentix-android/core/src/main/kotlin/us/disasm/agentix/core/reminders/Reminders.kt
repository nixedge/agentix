package us.disasm.agentix.core.reminders

import java.time.Instant
import java.time.ZoneId
import java.util.UUID

private fun identity(id: UUID) { require(id != UUID(0, 0)) }
data class Reminder(val id: UUID, val revision: Long, val text: String, val dueAt: Instant, val timezone: String, val cancelled: Boolean) {
    init { identity(id); require(revision > 0 && text.isNotBlank() && text.toByteArray(Charsets.UTF_8).size <= 1024); ZoneId.of(timezone) }
}
enum class Delivery { ALARM_INTENT, ARMED, BLOCKED, POST_PENDING, POSTED, RINGING, DISMISSED, UNCERTAIN, CANCEL_APPLIED }
fun Delivery.completed() = this in setOf(Delivery.POSTED, Delivery.DISMISSED, Delivery.CANCEL_APPLIED)
data class Observation(val reportId: UUID, val reminderId: UUID, val revision: Long, val sequence: Long, val state: Delivery) {
    init { identity(reportId); identity(reminderId); require(revision > 0 && sequence > 0 && state != Delivery.ALARM_INTENT) }
}
data class Snapshot(val serverId: UUID, val deviceId: UUID, val sequence: Long, val reminders: List<Reminder>, val observations: List<Observation>) {
    init {
        identity(serverId); identity(deviceId); require(sequence >= 0 && reminders.size <= 1000 && observations.size <= 1000)
        require(reminders.map { it.id }.toSet().size == reminders.size)
        require(observations.map { it.reminderId }.toSet().size == observations.size)
        require(observations.all { o -> reminders.any { it.id == o.reminderId && it.revision == o.revision } })
        require(observations.all { o -> reminders.single { it.id == o.reminderId }.cancelled == (o.state == Delivery.CANCEL_APPLIED) })
    }
}
data class AlarmTarget(val id: UUID, val revision: Long, val generation: Long) {
    init { identity(id); require(revision > 0 && generation >= 0) }
}
data class Entry(val reminder: Reminder, val phase: Delivery = Delivery.ALARM_INTENT, val reported: Delivery? = null, val generation: Long = 0, val snoozeUntil: Instant? = null) {
    init {
        require(generation >= 0)
        require(snoozeUntil == null || (generation > 0 && !reminder.cancelled && !phase.completed() && phase !in setOf(Delivery.RINGING, Delivery.POST_PENDING)))
    }
    fun target() = AlarmTarget(reminder.id, reminder.revision, generation)
    fun deadline() = snoozeUntil ?: reminder.dueAt
    companion object {
        fun restore(version: Int, reminder: Reminder, phase: Delivery, reported: Delivery?, generation: Long?, snoozeUntil: Instant?): Entry {
            require(version in 1..2) { "Unknown reminder journal format" }
            return if (version == 1) Entry(reminder, phase, reported)
                else Entry(reminder, phase, reported, requireNotNull(generation), snoozeUntil)
        }
    }
}
data class Journal(
    val endpoint: String,
    val deviceId: UUID,
    val serverId: UUID? = null,
    val snapshotSequence: Long = -1,
    val nextReport: Long = 1,
    val entries: List<Entry> = emptyList(),
    val reports: List<Observation> = emptyList(),
) {
    init {
        identity(deviceId); serverId?.let(::identity)
        require(nextReport > 0 && snapshotSequence >= -1 && entries.size <= 1000 && reports.size <= 1000)
        require(entries.map { it.reminder.id }.toSet().size == entries.size)
        require(reports.map { it.reportId }.toSet().size == reports.size)
        require(reports.all { it.sequence < nextReport })
    }
    fun apply(snapshot: Snapshot): Journal {
        require(deviceId == snapshot.deviceId && (serverId == null || serverId == snapshot.serverId)) { "Reminder backend identity changed" }
        require(snapshot.sequence >= snapshotSequence) { "Stale reminder snapshot" }
        if (snapshot.sequence == snapshotSequence) require(entries.map { it.reminder }.toSet() == snapshot.reminders.toSet()) { "Reminder snapshot changed without a new sequence" }
        val updated = snapshot.reminders.map { item ->
            val previous = entries.find { it.reminder.id == item.id }
            require(previous == null || item.revision >= previous.reminder.revision)
            if (previous != null && item.revision == previous.reminder.revision) require(item == previous.reminder)
            val observed = snapshot.observations.find { it.reminderId == item.id }
            if (previous?.reminder == item) {
                when {
                    previous.phase == Delivery.DISMISSED -> previous
                    observed?.state == Delivery.DISMISSED || observed?.state == Delivery.POSTED -> previous.copy(phase = observed.state, reported = observed.state, snoozeUntil = null)
                    observed?.state == Delivery.RINGING && previous.generation == 0L && previous.phase in setOf(Delivery.ALARM_INTENT, Delivery.ARMED, Delivery.POST_PENDING) -> previous.copy(phase = Delivery.RINGING, reported = Delivery.RINGING)
                    else -> previous
                }
            } else {
                val phase = when {
                    item.cancelled -> Delivery.ALARM_INTENT
                    observed?.state == Delivery.POSTED -> Delivery.POSTED
                    observed?.state == Delivery.DISMISSED -> Delivery.DISMISSED
                    observed?.state == Delivery.RINGING -> Delivery.RINGING
                    else -> Delivery.ALARM_INTENT
                }
                Entry(item, phase, if (phase.completed() || phase == Delivery.RINGING) phase else null)
            }
        }
        require(entries.all { old -> updated.any { it.reminder.id == old.reminder.id } || old.phase.completed() }) { "Snapshot omitted a live reminder" }
        val observedNext = snapshot.observations.maxOfOrNull { Math.addExact(it.sequence, 1) } ?: 1
        return copy(serverId = snapshot.serverId, snapshotSequence = snapshot.sequence, nextReport = maxOf(nextReport, observedNext), entries = updated,
            reports = reports.filter { o -> updated.any { it.reminder.id == o.reminderId && it.reminder.revision == o.revision && (!it.phase.completed() || o.state == it.phase) } })
    }
    fun transition(id: UUID, revision: Long, phase: Delivery): Journal {
        val entry = entries.single { it.reminder.id == id }
        require(entry.reminder.revision == revision) { "Stale reminder revision" }
        require((phase == Delivery.CANCEL_APPLIED) == entry.reminder.cancelled || phase == Delivery.ALARM_INTENT)
        require(!entry.phase.completed() || phase == entry.phase || (entry.phase == Delivery.POSTED && phase == Delivery.DISMISSED)) { "Completed reminder cannot be rearmed" }
        require(phase != Delivery.DISMISSED || entry.snoozeUntil != null || entry.phase in setOf(Delivery.POST_PENDING, Delivery.RINGING, Delivery.POSTED, Delivery.DISMISSED, Delivery.BLOCKED, Delivery.UNCERTAIN)) { "Upcoming reminder cannot be dismissed" }
        val emit = phase != Delivery.ALARM_INTENT && entry.reported != phase
        val next = if (emit) Math.addExact(nextReport, 1) else nextReport
        val event = if (emit) Observation(UUID.randomUUID(), id, revision, nextReport, phase) else null
        return copy(nextReport = next, entries = entries.map { if (it.reminder.id == id) it.copy(phase = phase, reported = if (emit) phase else it.reported, snoozeUntil = if (phase.completed() || phase in setOf(Delivery.POST_PENDING, Delivery.RINGING)) null else it.snoozeUntil) else it },
            reports = if (event == null) reports else reports.filter { it.reminderId != id } + event)
    }
    fun acknowledge(ids: Set<UUID>) = copy(reports = reports.filter { it.reportId !in ids })
    fun snooze(server: UUID, targets: List<AlarmTarget>, now: Instant): Journal {
        if (serverId != server) return this
        val until = now.plusSeconds(600)
        return copy(entries = entries.map { entry ->
            if (entry.target() in targets && !entry.reminder.cancelled && entry.phase in setOf(Delivery.RINGING, Delivery.POST_PENDING)) {
                entry.copy(phase = Delivery.ALARM_INTENT, generation = Math.addExact(entry.generation, 1), snoozeUntil = until)
            } else entry
        })
    }
    fun dismiss(server: UUID, targets: List<AlarmTarget>, now: Instant): Journal {
        if (serverId != server) return this
        var next = this
        targets.forEach { target ->
            val entry = next.entries.find { it.target() == target && !it.reminder.cancelled }
            if (entry != null && (entry.snoozeUntil != null || entry.phase in setOf(Delivery.POST_PENDING, Delivery.RINGING, Delivery.POSTED) || (entry.reminder.dueAt <= now && entry.phase in setOf(Delivery.BLOCKED, Delivery.UNCERTAIN)))) {
                next = next.transition(target.id, target.revision, Delivery.DISMISSED)
            }
        }
        return next
    }
    fun eligible(id: UUID, revision: Long, now: Instant): Boolean = entries.any {
        it.reminder.id == id && it.reminder.revision == revision && !it.reminder.cancelled && (it.deadline() <= now || it.phase == Delivery.RINGING) && !it.phase.completed()
    }
    fun eligible(target: AlarmTarget, now: Instant): Boolean = entries.any { it.target() == target } && eligible(target.id, target.revision, now)
}
