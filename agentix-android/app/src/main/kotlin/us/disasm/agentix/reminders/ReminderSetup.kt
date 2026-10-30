package us.disasm.agentix.reminders

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import androidx.core.content.ContextCompat
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

@Composable
fun ReminderSetup() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var message by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    val notifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { allowed ->
        message = if (allowed) "Notifications allowed. You can enable reminders." else "Reminders need notification permission."
    }
    val alarms = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        message = if (ReminderEngine.canAlarm(context)) "Alarm access allowed. You can enable reminders." else "Reminders need alarm access."
    }
    Text("Reminders use notifications and alarms on this phone.")
    Button(onClick = {
        if (Build.VERSION.SDK_INT >= 33 && ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) notifications.launch(Manifest.permission.POST_NOTIFICATIONS)
        else context.startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName))
    }) { Text("Allow reminder notifications") }
    if (Build.VERSION.SDK_INT >= 31) {
        Button(onClick = { alarms.launch(Intent(Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM, Uri.parse("package:${context.packageName}"))) }) { Text("Allow reminder alarms") }
    }
    Button(enabled = !busy, onClick = {
        busy = true
        scope.launch {
            message = withContext(Dispatchers.IO) {
                runCatching {
                    if (ReminderEngine.enabled(context)) ReminderEngine.sync(context.applicationContext)
                    else ReminderEngine.enable(context.applicationContext)
                    if (ReminderEngine.canNotify(context) && ReminderEngine.canAlarm(context)) "Reminders synchronized on this phone."
                    else "Reminders synchronized; delivery is blocked until notification and alarm access are restored."
                }
                    .getOrElse { it.message ?: "Reminders could not be enabled. Check permissions and connection." }
            }
            busy = false
        }
    }) { Text(if (ReminderEngine.enabled(context)) "Sync reminders" else "Enable reminders on this phone") }
    if (message.isNotBlank()) Text(message)
}
