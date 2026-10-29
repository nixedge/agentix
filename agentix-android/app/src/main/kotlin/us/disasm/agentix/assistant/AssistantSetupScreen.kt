package us.disasm.agentix.assistant

import android.content.ActivityNotFoundException
import android.content.Intent
import android.provider.Settings
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import us.disasm.agentix.R

@Composable
fun AssistantSetupScreen(active: Boolean) {
    val context = LocalContext.current
    Text(stringResource(R.string.assistant_title), style = MaterialTheme.typography.headlineSmall)
    Text(stringResource(if (active) R.string.assistant_selected else R.string.assistant_not_selected))
    Text(stringResource(R.string.assistant_setup_detail))
    Button(onClick = {
        try {
            context.startActivity(Intent(Settings.ACTION_VOICE_INPUT_SETTINGS))
        } catch (_: ActivityNotFoundException) {
            context.startActivity(Intent(Settings.ACTION_MANAGE_DEFAULT_APPS_SETTINGS))
        }
    }) { Text(stringResource(R.string.assistant_choose)) }
    Button(onClick = { context.startActivity(Intent(context, AssistantActivity::class.java)) }) {
        Text(stringResource(R.string.assistant_open))
    }
    Text(stringResource(R.string.assistant_no_wakeword))
}
