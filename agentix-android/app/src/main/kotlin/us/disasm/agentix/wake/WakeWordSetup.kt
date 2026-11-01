package us.disasm.agentix.wake

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext

@Composable
internal fun WakeWordSetup(active: Boolean) {
    val context = LocalContext.current
    val permissions = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        WakeWordRuntime.status = "Permission request finished. Press Start listening when ready."
    }
    Text("Hey Agentix", style = MaterialTheme.typography.titleMedium)
    Text("Optional hands-free activation. Audio is checked locally in memory and is never saved or sent by the wake detector. Continuous software listening uses more battery than Google's built-in wake word and shows a microphone notification. Say Hey Agentix, wait for the assistant, then speak your command. Unlocking is required for commands on a locked phone.")
    Text(WakeWordRuntime.status)
    if (!WakeWordRuntime.running && WakeWordService.preferences(context).getBoolean("enabled", false)) {
        Text("Previously enabled, currently stopped. Start again here after a reboot, process stop or microphone error.")
    }
    Button(enabled = active || WakeWordRuntime.running, onClick = {
        if (WakeWordRuntime.running) {
            context.startService(Intent(context, WakeWordService::class.java).setAction(WakeWordService.STOP))
        } else {
            val required = mutableListOf(Manifest.permission.RECORD_AUDIO)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) required += Manifest.permission.POST_NOTIFICATIONS
            val missing = required.filter { context.checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
            if (missing.isNotEmpty()) permissions.launch(missing.toTypedArray())
            else try {
                context.startForegroundService(Intent(context, WakeWordService::class.java).setAction(WakeWordService.START))
            } catch (_: RuntimeException) { WakeWordRuntime.status = "Android refused the microphone service. Keep this screen open and check app permissions before trying again." }
        }
    }) { Text(if (WakeWordRuntime.running) "Stop listening" else "Start listening") }
}
