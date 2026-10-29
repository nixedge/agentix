package us.disasm.agentix

import android.content.Intent
import android.content.ComponentName
import android.os.Bundle
import android.service.voice.VoiceInteractionService
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import us.disasm.agentix.device.PlaudScanner
import us.disasm.agentix.settings.AssistantSettings
import us.disasm.agentix.assistant.AgentixVoiceService
import us.disasm.agentix.assistant.AssistantSetupScreen
import us.disasm.agentix.recording.PhoneCaptureState
import us.disasm.agentix.recording.PhonePlayback
import us.disasm.agentix.recording.PhoneRecordingStore
import us.disasm.agentix.recording.RecordScreen
import us.disasm.agentix.recording.RecordingInbox

class MainActivity : ComponentActivity() {
    private lateinit var scanner: PlaudScanner
    private val playback = PhonePlayback()
    private var recordRequests by mutableStateOf(0)
    private var assistantActive by mutableStateOf(false)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        scanner = PlaudScanner(applicationContext)
        val settings = AssistantSettings(applicationContext)
        val recordings = PhoneRecordingStore(filesDir)
        if (savedInstanceState == null && opensRecording(intent)) recordRequests++
        setContent { MaterialTheme { AgentixApp(scanner, settings, recordings, playback, recordRequests, assistantActive) } }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        if (opensRecording(intent)) recordRequests++
    }

    override fun onResume() {
        super.onResume()
        assistantActive = VoiceInteractionService.isActiveService(this, ComponentName(this, AgentixVoiceService::class.java))
        PhoneCaptureState.refreshLibrary()
    }

    override fun onPause() {
        playback.stop()
        super.onPause()
    }

    override fun onStop() {
        scanner.stop()
        playback.stop()
        super.onStop()
    }

    private fun opensRecording(intent: Intent?): Boolean =
        intent?.action == ACTION_OPEN_RECORD ||
            (intent?.action == Intent.ACTION_VIEW && intent.data?.scheme == "agentix" && intent.data?.host == "record") ||
            intent?.getStringExtra("feature") == "record"

    companion object {
        const val ACTION_OPEN_RECORD = "us.disasm.agentix.OPEN_RECORD"
    }
}

@Composable
private fun AgentixApp(
    scanner: PlaudScanner,
    settings: AssistantSettings,
    recordings: PhoneRecordingStore,
    playback: PhonePlayback,
    recordRequests: Int,
    assistantActive: Boolean,
) {
    var destination by rememberSaveable { mutableStateOf(0) }
    androidx.compose.runtime.LaunchedEffect(recordRequests) {
        if (recordRequests > 0) destination = 1
    }
    val destinations = listOf(R.string.inbox, R.string.record, R.string.devices, R.string.settings, R.string.assistant_title)

    Scaffold { insets ->
        Column(
            Modifier.fillMaxSize().padding(insets).padding(horizontal = 20.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text(stringResource(R.string.title), style = MaterialTheme.typography.headlineLarge)
            Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                destinations.forEachIndexed { index, title ->
                    FilterChip(
                        selected = destination == index,
                        onClick = { destination = index },
                        label = { Text(stringResource(title)) },
                    )
                }
            }
            Column(
                Modifier.weight(1f).verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                when (destination) {
                    0 -> RecordingInbox(recordings, playback)
                    1 -> RecordScreen(playback)
                    2 -> DevicesScreen(scanner)
                    3 -> SettingsScreen(settings)
                    4 -> AssistantSetupScreen(assistantActive)
                }
            }
        }
    }
}

@Composable
private fun DevicesScreen(scanner: PlaudScanner) {
    val permissions = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { _ ->
        // A result can arrive after navigation or backgrounding; require another explicit scan.
        if (!scanner.hasPermissions()) scanner.permissionDenied()
    }
    DisposableEffect(scanner) { onDispose { scanner.stop() } }

    Text(stringResource(R.string.devices_title), style = MaterialTheme.typography.headlineSmall)
    Text(stringResource(R.string.devices_detail))
    Button(onClick = {
        if (scanner.scanning) {
            scanner.stop()
        } else if (scanner.hasPermissions()) {
            scanner.start()
        } else {
            permissions.launch(scanner.permissions)
        }
    }) {
        Text(stringResource(if (scanner.scanning) R.string.stop_scan else R.string.scan))
    }
    Text(scanner.status)
    if (scanner.devices.isEmpty()) Text(stringResource(R.string.scan_none))
    scanner.devices.forEach { device ->
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(device.name ?: stringResource(R.string.device_unknown), style = MaterialTheme.typography.titleMedium)
                Text(stringResource(R.string.device_signal, device.address, device.rssi))
                device.projectCode?.let { Text(stringResource(R.string.device_project, it)) }
                Text(stringResource(R.string.device_unverified))
            }
        }
    }
}

@Composable
private fun SettingsScreen(settings: AssistantSettings) {
    var endpoint by rememberSaveable { mutableStateOf(settings.endpoint()) }
    var invalid by rememberSaveable { mutableStateOf(false) }
    var configured by rememberSaveable { mutableStateOf(settings.endpoint().isNotBlank()) }

    Text(stringResource(R.string.settings_title), style = MaterialTheme.typography.headlineSmall)
    Text(stringResource(R.string.settings_detail))
    OutlinedTextField(
        value = endpoint,
        onValueChange = { endpoint = it; invalid = false },
        modifier = Modifier.fillMaxWidth(),
        label = { Text(stringResource(R.string.endpoint_label)) },
        placeholder = { Text(stringResource(R.string.endpoint_example)) },
        singleLine = true,
        isError = invalid,
        supportingText = { if (invalid) Text(stringResource(R.string.endpoint_invalid)) },
    )
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Button(onClick = {
            invalid = !settings.saveEndpoint(endpoint)
            if (!invalid) {
                endpoint = settings.endpoint()
                configured = true
            }
        }) { Text(stringResource(R.string.save)) }
        TextButton(onClick = {
            settings.clearEndpoint()
            endpoint = ""
            configured = false
            invalid = false
        }) { Text(stringResource(R.string.clear)) }
    }
    Text(stringResource(if (configured) R.string.endpoint_saved else R.string.endpoint_unconfigured))
}
