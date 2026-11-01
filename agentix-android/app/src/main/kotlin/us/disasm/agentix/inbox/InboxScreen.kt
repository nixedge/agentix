package us.disasm.agentix.inbox

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import us.disasm.agentix.R
import us.disasm.agentix.core.inbox.*
import us.disasm.agentix.core.reminders.*
import us.disasm.agentix.recording.*
import us.disasm.agentix.reminders.ReminderCard
import us.disasm.agentix.reminders.ReminderEngine
import java.time.Instant

private data class InboxData(val journal: Journal?, val recordings: RecordingLibrary, val archives: Archives?, val errors: List<String>)

@Composable
fun InboxScreen(store: PhoneRecordingStore, playback: PhonePlayback, archive: Boolean, selected: String?) {
    val context = LocalContext.current.applicationContext
    val reminderChange by ReminderEngine.changes.collectAsState()
    val archiveChange by ArchiveStore.changes.collectAsState()
    val scope = rememberCoroutineScope()
    var limit by remember(archive) { mutableStateOf(20) }
    var actionError by remember(archive) { mutableStateOf(false) }
    val data by produceState<InboxData?>(null, reminderChange, archiveChange, PhoneCaptureState.revision) {
        value = withContext(Dispatchers.IO) {
            val journal = runCatching { ReminderEngine.inbox(context) }
            val recordings = runCatching { store.recoverAndList { PhoneCaptureState.activeFileName } }
            val archives = runCatching { ArchiveStore.load(context) }
            InboxData(journal.getOrNull(), recordings.getOrDefault(RecordingLibrary(emptyList(), true)), archives.getOrNull(), buildList {
                if (journal.isFailure) add("Reminder storage needs recovery.")
                if (recordings.isFailure || recordings.getOrNull()?.hasReadErrors == true) add(context.getString(R.string.recording_load_failed))
                if (archives.isFailure) add("Could not load archive organization. Active alarms and resolved reminders remain available.")
            })
        }
    }
    DisposableEffect(playback, archive) { onDispose { playback.stop() } }
    Text(stringResource(if (archive) R.string.archive else R.string.inbox), style = MaterialTheme.typography.headlineSmall)
    if (actionError) Text("Could not move this item. It may have changed or its alarm may be firing. Try again.", color = MaterialTheme.colorScheme.error)
    playback.error?.let { Text(stringResource(it), color = MaterialTheme.colorScheme.error) }
    val loaded = data
    if (loaded == null) { Text("Loading…"); return }
    loaded.errors.forEach { Text(it, color = MaterialTheme.colorScheme.error) }
    val archives = loaded.archives
    val journal = loaded.journal
    val server = journal?.serverId
    val reminders = if (server == null) emptyMap() else checkNotNull(journal).entries.associateBy { reminderKey(server, it.reminder.id, it.reminder.revision) }
    val recordings = loaded.recordings.recordings.associateBy { recordingKey(it.file.name) }
    val items = recordings.map { (id, recording) -> InboxItem(id, InboxKind.RECORDING, Instant.ofEpochMilli(recording.startedAtMillis)) } +
        reminders.map { (id, entry) -> InboxItem(id, InboxKind.REMINDER, entry.deadline(), entry.firing(), entry.reminder.cancelled || entry.phase.completed()) }
    val selectedKey = reminders.entries.find { it.value.reminder.id.toString() == selected }?.key
    val shown = inboxItems(items, archives, archive, selectedKey)
    if (shown.isEmpty() && loaded.errors.isEmpty()) Text(if (archive) "Nothing archived yet." else "Inbox is empty.")
    shown.take(limit).forEach { item ->
        key(item.key) {
            val entry = reminders[item.key]
            val recording = recordings[item.key]
            val actions: @Composable () -> Unit = {
                if (!item.resolved && archives != null) {
                    TextButton(enabled = !item.firing, onClick = {
                        scope.launch {
                            val success = withContext(Dispatchers.IO) {
                                runCatching {
                                    if (entry != null) {
                                        ReminderEngine.organize(context, checkNotNull(server), entry.target()) { current -> ArchiveStore.change(context, current, !archive) }
                                    } else {
                                        check(recording?.file?.isFile == true) { "Recording changed" }
                                        ArchiveStore.change(context, item, !archive)
                                    }
                                }.isSuccess
                            }
                            actionError = !success
                            if (success && recording?.file?.name == playback.playingFile) playback.stop()
                        }
                    }) { Text(if (archive) "Restore to Inbox" else "Archive") }
                    if (item.firing) Text("Dismiss or snooze the alarm before archiving.", style = MaterialTheme.typography.bodySmall)
                }
            }
            if (entry != null && server != null) ReminderCard(entry, server, actions)
            else if (recording != null) RecordingCard(recording, playback, actions)
        }
    }
    if (shown.size > limit) TextButton(onClick = { limit += 20 }) { Text("Show more (${shown.size - limit})") }
}
