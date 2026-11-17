package us.disasm.agentix.device

import android.text.format.Formatter
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.Button
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import us.disasm.agentix.core.plaud.PlaudProtocol

@Composable
fun PlaudSyncStatus(controller: PlaudSyncController) {
    val context = LocalContext.current
    val lifecycle = (context as? ComponentActivity)?.lifecycle
    DisposableEffect(controller, lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_STOP) controller.cancel()
        }
        lifecycle?.addObserver(observer)
        onDispose { lifecycle?.removeObserver(observer); controller.cancel() }
    }
    val progress = controller.progress
    Text(progress.message, color = if (controller.failed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface)
    if (progress.totalBytes > 0 && controller.busy) {
        LinearProgressIndicator(progress = { (progress.bytes.toFloat() / progress.totalBytes).coerceIn(0f, 1f) }, modifier = Modifier.fillMaxWidth())
        Text("${Formatter.formatFileSize(context, progress.bytes)} / ${Formatter.formatFileSize(context, progress.totalBytes)}")
    }
    if (controller.busy) TextButton(onClick = { controller.cancel() }) { Text("Stop sync") }
}

@Composable
fun PlaudSyncActions(device: RecorderAdvertisement, controller: PlaudSyncController, scanner: PlaudScanner) {
    var token by remember(device.address) { mutableStateOf("") }
    var editing by remember(device.address) { mutableStateOf(!controller.hasToken(device.address)) }
    var message by remember(device.address) { mutableStateOf<String?>(null) }
    var confirmSwitch by remember(device.address) { mutableStateOf(false) }
    if (editing) {
        Text("This recorder was paired with the Plaud app. Enter its existing 32-character binding token to preserve that binding. This is not your server token or Plaud password.")
        OutlinedTextField(
            value = token,
            onValueChange = { token = it; message = null },
            label = { Text("Plaud binding token") },
            visualTransformation = PasswordVisualTransformation(),
            singleLine = true,
            enabled = !controller.busy,
            isError = token.isNotEmpty() && !PlaudProtocol.validToken(token),
            modifier = Modifier.fillMaxWidth(),
        )
        Button(enabled = !controller.busy && PlaudProtocol.validToken(token), onClick = {
            if (controller.saveToken(device.address, token)) {
                token = ""
                editing = false
                message = "Binding token saved privately. Ready to sync."
            } else message = "Could not save the binding token."
        }) { Text("Save binding token") }
    } else {
        Button(enabled = !controller.busy, onClick = { scanner.stop(); controller.sync(device.address) }) { Text("Sync recordings") }
        TextButton(enabled = !controller.busy, onClick = { editing = true }) { Text("Change binding token") }
    }
    message?.let { Text(it) }
    TextButton(enabled = !controller.busy, onClick = { confirmSwitch = true }) { Text("Use this Plaud with Agentix") }
    if (confirmSwitch) AlertDialog(
        onDismissRequest = { confirmSwitch = false },
        title = { Text("Switch this Plaud to Agentix?") },
        text = { Text("This clears the existing app binding and recorder settings and restarts the device. Existing recordings are preserved. The official Plaud app will need to pair again if you switch back. Do this only after stopping recording. Agentix will save a private token for future syncs.") },
        confirmButton = { TextButton(onClick = {
            confirmSwitch = false
            token = ""
            scanner.stop()
            controller.switchBindingAfterConfirmation(device.address)
            editing = false
        }) { Text("Switch to Agentix") } },
        dismissButton = { TextButton(onClick = { confirmSwitch = false }) { Text("Cancel") } },
    )
    Text("Sync copies recordings into Inbox. Keep Devices open and your Plaud nearby. Leaving stops safely; retry resumes partial downloads.", style = MaterialTheme.typography.bodySmall)
}

@Composable
fun RecordingServerControls() {
    val context = LocalContext.current
    var enabled by remember { mutableStateOf(RecordingServerSync.enabled(context)) }
    val settings = remember { us.disasm.agentix.settings.AssistantSettings(context) }
    val lifecycle = (context as? ComponentActivity)?.lifecycle
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_STOP) RecordingServerSync.cancel() }
        lifecycle?.addObserver(observer)
        onDispose { lifecycle?.removeObserver(observer); RecordingServerSync.cancel() }
    }
    Text("Server recordings", style = MaterialTheme.typography.titleMedium)
    Text("Destination: ${settings.endpoint()}. Uploads use your existing backend connection and owner token. Originals stay on this phone and the Plaud.")
    androidx.compose.foundation.layout.Row {
        androidx.compose.material3.Checkbox(checked = enabled, onCheckedChange = {
            if (RecordingServerSync.enable(context, it)) enabled = it
        })
        Text("Enable recording uploads")
    }
    Text("With uploads enabled, Sync recordings also sends completed imports to this server. You can sync existing imports without connecting the Plaud.", style = MaterialTheme.typography.bodySmall)
    Button(enabled = enabled && !RecordingServerSync.busy, onClick = { RecordingServerSync.start(context) }) { Text("Sync to server") }
    if (RecordingServerSync.busy) TextButton(onClick = { RecordingServerSync.cancel() }) { Text("Stop server sync") }
    Text(RecordingServerSync.status)
}
