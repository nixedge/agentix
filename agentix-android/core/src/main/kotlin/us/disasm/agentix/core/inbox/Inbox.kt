package us.disasm.agentix.core.inbox

import java.time.Instant
import java.util.UUID

enum class InboxKind { REMINDER, RECORDING }
data class InboxItem(val key: String, val kind: InboxKind, val time: Instant, val firing: Boolean = false, val resolved: Boolean = false) {
    init { require(key.isNotBlank() && key.length <= 256 && key.none { it.isISOControl() }); require(!firing || kind == InboxKind.REMINDER) }
    fun inArchive(archives: Archives) = !firing && (resolved || key in archives.keys)
}
data class Archives(val keys: Set<String> = emptySet()) {
    init { require(keys.size <= 10000 && keys.all { it.isNotBlank() && it.length <= 256 && it.none { c -> c.isISOControl() } }) }
    fun change(item: InboxItem, archived: Boolean): Archives {
        require(!item.firing) { "An active alarm cannot be archived" }
        require(!item.resolved) { "Resolved reminders remain in Archive" }
        return copy(keys = if (archived) keys + item.key else keys - item.key)
    }
}
fun recordingKey(filename: String): String {
    require(filename.isNotBlank() && filename.none { it == '/' || it == '\\' })
    return "recording/$filename"
}
fun reminderKey(server: UUID, id: UUID, revision: Long): String {
    require(server != UUID(0, 0) && id != UUID(0, 0) && revision > 0)
    return "reminder/$server/$id/$revision"
}
fun inboxItems(items: List<InboxItem>, archives: Archives?, archive: Boolean, selected: String? = null): List<InboxItem> {
    require(items.map { it.key }.toSet().size == items.size)
    return items.filter { (archives != null || it.firing || it.resolved) && it.inArchive(archives ?: Archives()) == archive }.sortedWith(
        compareBy<InboxItem> { !it.firing }.thenBy { it.key != selected }.thenByDescending { it.time }.thenBy { it.key },
    )
}
