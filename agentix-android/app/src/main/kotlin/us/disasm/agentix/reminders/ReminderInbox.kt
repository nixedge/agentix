package us.disasm.agentix.reminders

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
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
import us.disasm.agentix.core.reminders.Entry
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.UUID

@Composable
fun ReminderCard(entry: Entry, server: UUID, actions: @Composable () -> Unit) {
    val context = LocalContext.current.applicationContext
    val scope = rememberCoroutineScope()
    var actionError by remember { mutableStateOf(false) }
    val reminder = entry.reminder
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("Reminder", style = MaterialTheme.typography.labelMedium)
            Text(reminder.text, style = MaterialTheme.typography.titleMedium)
            Text(reminder.dueAt.atZone(ZoneId.of(reminder.timezone)).format(DateTimeFormatter.ofPattern("EEE, MMM d, yyyy · h:mm a z")))
            entry.snoozeUntil?.let {
                Text("Snoozed until ${it.atZone(ZoneId.systemDefault()).format(DateTimeFormatter.ofPattern("EEE, MMM d · h:mm a z"))}")
            }
            Text(when {
                reminder.cancelled -> if (entry.phase == Delivery.CANCEL_APPLIED) "Cancelled on this phone" else "Cancellation pending"
                entry.phase == Delivery.RINGING -> "Alarm active — dismiss or snooze to stop"
                entry.phase == Delivery.POST_PENDING -> "Starting alarm"
                entry.phase == Delivery.DISMISSED -> "Dismissed"
                entry.phase == Delivery.POSTED -> "Notification posted (history)"
                entry.phase == Delivery.BLOCKED -> "Blocked — check notification and alarm access"
                entry.phase == Delivery.UNCERTAIN -> "Alarm playback needs recovery"
                entry.phase == Delivery.ARMED -> if (entry.snoozeUntil != null) "Snooze scheduled on this phone" else "Upcoming — scheduled on this phone"
                else -> "Awaiting phone scheduling"
            })
            if (actionError) Text("Could not apply the action. Check this reminder's current status and try again.", color = MaterialTheme.colorScheme.error)
            if (entry.firing()) {
                Button(onClick = {
                    scope.launch {
                        actionError = withContext(Dispatchers.IO) { runCatching { ReminderEngine.snooze(context, server, listOf(entry.target())) }.isFailure }
                    }
                }) { Text("Snooze 10 minutes") }
            }
            if (!reminder.cancelled && (entry.snoozeUntil != null || entry.phase in setOf(Delivery.RINGING, Delivery.POST_PENDING, Delivery.POSTED) || (reminder.dueAt <= Instant.now() && entry.phase in setOf(Delivery.BLOCKED, Delivery.UNCERTAIN)))) {
                Button(onClick = {
                    scope.launch {
                        actionError = withContext(Dispatchers.IO) { runCatching { ReminderEngine.dismiss(context, server, listOf(entry.target())) }.isFailure }
                    }
                }) { Text("Dismiss") }
            }
            actions()
        }
    }
}
