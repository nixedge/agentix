package us.disasm.agentix.reminders

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.launch
import us.disasm.agentix.core.reminders.Delivery
import us.disasm.agentix.core.reminders.Journal
import us.disasm.agentix.core.reminders.completed
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter

@Composable
fun ReminderInbox(selected: String?) {
    val context = LocalContext.current.applicationContext
    val changed by ReminderEngine.changes.collectAsState()
    var limit by remember { mutableStateOf(20) }
    var error by remember { mutableStateOf(false) }
    var dismissError by remember { mutableStateOf(false) }
    var snoozeError by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val journal by produceState<Journal?>(null, changed) {
        val result = withContext(Dispatchers.IO) { runCatching { ReminderEngine.inbox(context) } }
        error = result.isFailure
        value = result.getOrNull()
    }
    Text("Reminders", style = MaterialTheme.typography.headlineSmall)
    Text("Alarms use your phone's alarm tone and alarm volume.", style = MaterialTheme.typography.bodySmall)
    if (error) Text("Reminder storage needs recovery. Existing recordings are still available.")
    if (dismissError) Text("Could not save dismissal. The alarm may still be active; try again.")
    if (snoozeError) Text("Snooze needs recovery. Check the reminder's scheduling status and alarm access.")
    val state = journal
    if (state == null || state.entries.isEmpty()) {
        if (!error) Text("No reminders synced to this phone.")
        return
    }
    val entries = state.entries.sortedWith(compareBy(
        { it.reminder.id.toString() != selected },
        { it.phase.completed() || it.reminder.cancelled },
        { it.phase !in setOf(Delivery.RINGING, Delivery.POST_PENDING) },
        { it.deadline() },
    ))
    entries.take(limit).forEach { entry ->
        val reminder = entry.reminder
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(reminder.text, style = MaterialTheme.typography.titleMedium)
                Text(reminder.dueAt.atZone(ZoneId.of(reminder.timezone)).format(DateTimeFormatter.ofPattern("EEE, MMM d, yyyy · h:mm a z")))
                entry.snoozeUntil?.let {
                    Text("Snoozed until ${it.atZone(ZoneId.systemDefault()).format(DateTimeFormatter.ofPattern("EEE, MMM d · h:mm a z"))}")
                }
                Text(when {
                    reminder.cancelled -> if (entry.phase == Delivery.CANCEL_APPLIED) "Cancelled on this phone" else "Cancellation pending"
                    entry.phase == Delivery.RINGING -> "Alarm active — dismiss to stop"
                    entry.phase == Delivery.POST_PENDING -> "Starting alarm"
                    entry.phase == Delivery.DISMISSED -> "Dismissed"
                    entry.phase == Delivery.POSTED -> "Notification posted (history)"
                    entry.phase == Delivery.BLOCKED -> "Blocked — check notification and alarm access"
                    entry.phase == Delivery.UNCERTAIN -> "Alarm playback needs recovery"
                    entry.phase == Delivery.ARMED -> if (entry.snoozeUntil != null) "Snooze scheduled on this phone" else "Upcoming — scheduled on this phone"
                    else -> "Awaiting phone scheduling"
                })
                if (!reminder.cancelled && entry.phase in setOf(Delivery.RINGING, Delivery.POST_PENDING)) {
                    Button(onClick = {
                        scope.launch {
                            snoozeError = withContext(Dispatchers.IO) {
                                runCatching { ReminderEngine.snooze(context, checkNotNull(state.serverId), listOf(entry.target())) }.isFailure
                            }
                        }
                    }) { Text("Snooze 10 minutes") }
                }
                if (!reminder.cancelled && (entry.snoozeUntil != null || entry.phase in setOf(Delivery.RINGING, Delivery.POST_PENDING, Delivery.POSTED) || (reminder.dueAt <= Instant.now() && entry.phase in setOf(Delivery.BLOCKED, Delivery.UNCERTAIN)))) {
                    Button(onClick = {
                        scope.launch {
                            dismissError = withContext(Dispatchers.IO) {
                                runCatching { ReminderEngine.dismiss(context, checkNotNull(state.serverId), listOf(entry.target())) }.isFailure
                            }
                        }
                    }) { Text("Dismiss") }
                }
            }
        }
    }
    if (entries.size > limit) TextButton(onClick = { limit += 20 }) { Text("Show more (${entries.size - limit})") }
    Text("Cached on this phone. Changes made elsewhere appear after synchronization.", style = MaterialTheme.typography.bodySmall)
}
