package us.disasm.agentix.assistant

import android.content.ActivityNotFoundException
import android.content.Intent
import android.provider.Settings
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import us.disasm.agentix.core.assistant.SpeechProfile
import us.disasm.agentix.core.assistant.SpeechOutput

@Composable
internal fun VoiceControls(controller: AssistantController, busy: Boolean) {
    val context = LocalContext.current
    var expanded by remember { mutableStateOf(false) }
    Text("Voice", style = MaterialTheme.typography.titleMedium)
    Row(Modifier.horizontalScroll(rememberScrollState())) {
        SpeechOutput.entries.forEach { output ->
            FilterChip(selected = controller.speechOutput == output, enabled = !busy,
                onClick = { controller.chooseSpeechOutput(output) }, label = { Text(output.label) })
        }
    }
    if (controller.speechOutput == SpeechOutput.ANDROID) {
        Row(Modifier.horizontalScroll(rememberScrollState())) {
            FilterChip(selected = controller.voiceProfile == SpeechProfile.BRITISH_MEASURED, enabled = !busy,
                onClick = { controller.updateVoiceProfile(SpeechProfile.BRITISH_MEASURED) }, label = { Text("British, measured") })
            FilterChip(selected = controller.voiceProfile == SpeechProfile.SYSTEM, enabled = !busy,
                onClick = { controller.updateVoiceProfile(SpeechProfile.SYSTEM) }, label = { Text("Device language") })
        }
        Box {
            val selected = controller.voices.find { it.id == controller.selectedVoice }
            OutlinedButton(onClick = { expanded = true }, enabled = !busy && controller.voices.isNotEmpty()) {
                Text(selected?.let(controller::voiceLabel) ?: "Choose an offline voice")
            }
            DropdownMenu(expanded = expanded, onDismissRequest = { expanded = false }, modifier = Modifier.heightIn(max = 320.dp)) {
                controller.voices.forEach { voice ->
                    DropdownMenuItem(text = { Text(controller.voiceLabel(voice)) }, onClick = { controller.selectVoice(voice.id); expanded = false })
                }
            }
        }
    }
    controller.voiceNotice?.let { Text(it) }
    Button(onClick = controller::previewVoice, enabled = !busy && controller.canPreviewVoice) { Text("Preview voice") }
    if (controller.speechOutput == SpeechOutput.ANDROID) {
        TextButton(onClick = {
            try { context.startActivity(Intent("com.android.settings.TTS_SETTINGS")) }
            catch (_: ActivityNotFoundException) { context.startActivity(Intent(Settings.ACTION_SETTINGS)) }
        }) { Text("Android speech settings") }
    }
}
