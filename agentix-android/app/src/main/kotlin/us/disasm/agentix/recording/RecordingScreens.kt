package us.disasm.agentix.recording

import android.Manifest
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.SystemClock
import android.text.format.DateUtils
import android.text.format.Formatter
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import java.text.DateFormat
import java.util.Date
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import us.disasm.agentix.R

@Composable
fun RecordScreen(playback: PhonePlayback) {
    val context = LocalContext.current
    val permissions = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        PhoneCaptureState.message = if (hasMicrophone(context)) R.string.record_permission_ready else R.string.record_permission
    }
    val phase = PhoneCaptureState.phase
    var elapsed by remember { mutableStateOf(0L) }
    LaunchedEffect(phase, PhoneCaptureState.startedElapsedMillis) {
        while (PhoneCaptureState.phase == CapturePhase.RECORDING) {
            elapsed = (SystemClock.elapsedRealtime() - PhoneCaptureState.startedElapsedMillis).coerceAtLeast(0)
            delay(250)
        }
    }

    Text(stringResource(R.string.record_title), style = MaterialTheme.typography.headlineSmall)
    Text(stringResource(R.string.record_detail))
    Text(when (phase) {
        CapturePhase.IDLE -> stringResource(PhoneCaptureState.message ?: R.string.record_idle)
        CapturePhase.STARTING -> stringResource(R.string.record_starting)
        CapturePhase.RECORDING -> stringResource(R.string.record_active, DateUtils.formatElapsedTime(elapsed / 1000))
        CapturePhase.STOPPING -> stringResource(R.string.record_stopping)
    })
    Button(
        enabled = phase == CapturePhase.IDLE || phase == CapturePhase.RECORDING,
        onClick = {
            if (phase == CapturePhase.RECORDING) {
                context.startService(Intent(context, PhoneRecordingService::class.java).setAction(PhoneRecordingService.ACTION_STOP))
            } else if (!hasMicrophone(context)) {
                val requested = mutableListOf(Manifest.permission.RECORD_AUDIO)
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) requested += Manifest.permission.POST_NOTIFICATIONS
                permissions.launch(requested.toTypedArray())
            } else {
                playback.stop()
                try {
                    context.startForegroundService(Intent(context, PhoneRecordingService::class.java).setAction(PhoneRecordingService.ACTION_START))
                } catch (_: RuntimeException) {
                    PhoneCaptureState.message = R.string.record_failed
                }
            }
        },
    ) {
        Text(stringResource(if (phase == CapturePhase.RECORDING) R.string.record_stop else R.string.record_start))
    }
}

private fun hasMicrophone(context: Context): Boolean =
    context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

@Composable
fun RecordingInbox(store: PhoneRecordingStore, playback: PhonePlayback) {
    val context = LocalContext.current
    var library by remember { mutableStateOf(RecordingLibrary(emptyList(), false)) }
    var shareError by remember { mutableStateOf(false) }
    LaunchedEffect(PhoneCaptureState.revision) {
        library = withContext(Dispatchers.IO) { store.recoverAndList { PhoneCaptureState.activeFileName } }
    }
    DisposableEffect(playback) { onDispose { playback.stop() } }

    Text(stringResource(R.string.inbox_title), style = MaterialTheme.typography.headlineSmall)
    if (library.hasReadErrors) Text(stringResource(R.string.recording_load_failed), color = MaterialTheme.colorScheme.error)
    playback.error?.let { Text(stringResource(it), color = MaterialTheme.colorScheme.error) }
    if (shareError) Text(stringResource(R.string.recording_share_failed), color = MaterialTheme.colorScheme.error)
    if (library.recordings.isEmpty()) {
        Text(stringResource(R.string.inbox_empty))
        Text(stringResource(R.string.inbox_detail))
    }
    library.recordings.forEach { recording ->
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(stringResource(R.string.recording_title, DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(Date(recording.startedAtMillis))))
                Text(stringResource(R.string.recording_details,
                    DateUtils.formatElapsedTime(recording.durationMillis / 1000),
                    Formatter.formatFileSize(context, recording.file.length())))
                if (recording.recovered) Text(stringResource(R.string.recording_recovered))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    TextButton(
                        enabled = PhoneCaptureState.phase == CapturePhase.IDLE,
                        onClick = {
                            if (playback.playingFile == recording.file.name) playback.stop() else playback.play(recording.file)
                        },
                    ) {
                        Text(stringResource(if (playback.playingFile == recording.file.name) R.string.recording_stop_playback else R.string.recording_play))
                    }
                    TextButton(onClick = {
                        try {
                            val uri = FileProvider.getUriForFile(context, "${context.packageName}.recordings", recording.file)
                            val share = Intent(Intent.ACTION_SEND).apply {
                                type = "audio/wav"
                                putExtra(Intent.EXTRA_STREAM, uri)
                                clipData = android.content.ClipData.newRawUri("Voice note", uri)
                                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                            }
                            context.startActivity(Intent.createChooser(share, context.getString(R.string.recording_share)))
                            shareError = false
                        } catch (_: ActivityNotFoundException) {
                            shareError = true
                        } catch (_: IllegalArgumentException) {
                            shareError = true
                        }
                    }) { Text(stringResource(R.string.recording_share)) }
                }
            }
        }
    }
}
