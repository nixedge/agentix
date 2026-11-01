package us.disasm.agentix.assistant

import android.Manifest
import android.app.KeyguardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.os.Build
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import us.disasm.agentix.R
import us.disasm.agentix.core.assistant.TurnPhase

class AssistantActivity : ComponentActivity() {
    private lateinit var controller: AssistantController
    private var voiceInvocation = false
    private var foreground = false
    private var locked by mutableStateOf(true)
    private val permission = registerForActivityResult(ActivityResultContracts.RequestPermission()) {
        controller.permissionResult(it)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        controller = AssistantController(this)
        voiceInvocation = savedInstanceState == null && intent?.action == ACTION_VOICE_TURN
        configureWakeWindow(intent)
        locked = getSystemService(KeyguardManager::class.java).isDeviceLocked
        setContent {
            MaterialTheme {
                if (locked) {
                    Scaffold { insets ->
                        Column(Modifier.fillMaxSize().padding(insets).padding(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                            Text("Agentix assistant", style = MaterialTheme.typography.headlineLarge)
                            Text("Unlock your phone to speak a command.")
                            Button(onClick = ::unlock) { Text("Unlock") }
                            TextButton(onClick = ::finish) { Text("Close") }
                        }
                    }
                } else AssistantConversation(controller, ::talk, ::finish) {
                    startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:$packageName")))
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        configureWakeWindow(intent)
        controller.cancel()
        locked = getSystemService(KeyguardManager::class.java).isDeviceLocked
        voiceInvocation = intent.action == ACTION_VOICE_TURN
        if (foreground && hasWindowFocus()) consumeVoiceInvocation()
    }

    override fun onResume() {
        super.onResume()
        foreground = true
        locked = getSystemService(KeyguardManager::class.java).isDeviceLocked
        if (hasWindowFocus() && !locked) {
            controller.resume()
            consumeVoiceInvocation()
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus && foreground) {
            locked = getSystemService(KeyguardManager::class.java).isDeviceLocked
            if (!locked) {
                controller.resume()
                consumeVoiceInvocation()
            }
        } else if (!hasFocus) {
            controller.pause()
        }
    }

    private fun consumeVoiceInvocation() {
        if (!voiceInvocation || locked) return
        voiceInvocation = false
        talk()
    }

    private fun talk() {
        if (getSystemService(KeyguardManager::class.java).isDeviceLocked) {
            controller.listen()
        } else if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            permission.launch(Manifest.permission.RECORD_AUDIO)
        } else {
            controller.listen()
        }
    }

    override fun onPause() {
        voiceInvocation = false
        foreground = false
        controller.pause()
        super.onPause()
    }

    override fun onDestroy() {
        controller.close()
        super.onDestroy()
    }

    companion object {
        // Only our protected voice session can launch this non-exported activity with this action.
        const val ACTION_VOICE_TURN = "us.disasm.agentix.VOICE_TURN"
    }

    private fun configureWakeWindow(intent: Intent?) {
        val wake = intent?.getBooleanExtra(AgentixVoiceService.WAKE, false) == true
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            setShowWhenLocked(wake)
            setTurnScreenOn(wake)
        }
    }

    private fun unlock() {
        getSystemService(KeyguardManager::class.java).requestDismissKeyguard(this, object : KeyguardManager.KeyguardDismissCallback() {
            override fun onDismissSucceeded() {
                locked = getSystemService(KeyguardManager::class.java).isDeviceLocked
                if (foreground && hasWindowFocus() && !locked) {
                    controller.resume()
                    consumeVoiceInvocation()
                }
            }
        })
    }
}

@Composable
private fun AssistantConversation(
    controller: AssistantController,
    talk: () -> Unit,
    close: () -> Unit,
    appSettings: () -> Unit,
) {
    val busy = controller.phase == TurnPhase.LISTENING || controller.phase == TurnPhase.PROCESSING
    Scaffold { insets ->
        Column(
            Modifier.fillMaxSize().padding(insets).padding(20.dp).verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text(stringResource(R.string.assistant_title), style = MaterialTheme.typography.headlineLarge)
            Text(stringResource(R.string.assistant_local_detail))
            Text(stringResource(controller.message))
            controller.pendingText?.let {
                Text(stringResource(R.string.assistant_pending_instruction, it))
                Button(onClick = controller::retryPending, enabled = !busy) {
                    Text(stringResource(R.string.assistant_retry_pending))
                }
            }
            OutlinedTextField(
                value = controller.draft,
                onValueChange = controller::updateDraft,
                readOnly = busy,
                modifier = Modifier.fillMaxWidth(),
                minLines = 3,
                label = { Text(stringResource(R.string.assistant_instruction)) },
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = talk, enabled = !busy && controller.pendingText == null) { Text(stringResource(R.string.assistant_talk)) }
                if (controller.phase == TurnPhase.LISTENING) {
                    Button(onClick = controller::stopListening) { Text(stringResource(R.string.assistant_finish)) }
                } else {
                    Button(onClick = controller::submitTyped, enabled = !busy && controller.pendingText == null && controller.draft.isNotBlank()) {
                        Text(stringResource(R.string.assistant_preview_instruction))
                    }
                }
            }
            if (busy) TextButton(onClick = controller::cancel) { Text(stringResource(R.string.assistant_cancel)) }
            if (controller.reply.isNotBlank()) {
                Text(controller.reply, style = MaterialTheme.typography.bodyLarge)
            }
            Row {
                Checkbox(checked = controller.speakReplies, onCheckedChange = controller::updateSpeakReplies)
                Text(stringResource(R.string.assistant_speak_replies), Modifier.padding(top = 12.dp))
            }
            controller.speechStatus?.let { Text(stringResource(it)) }
            TextButton(onClick = controller::stopSpeaking) { Text(stringResource(R.string.assistant_stop_speaking)) }
            VoiceControls(controller, busy)
            TextButton(onClick = appSettings) { Text(stringResource(R.string.assistant_app_permissions)) }
            TextButton(onClick = close) { Text(stringResource(R.string.assistant_close)) }
        }
    }
}
