package us.disasm.agentix.device

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import us.disasm.agentix.core.plaud.RecordingUploadJournal
import us.disasm.agentix.core.plaud.RecordingUploadQueue
import us.disasm.agentix.recording.PhoneCaptureState
import us.disasm.agentix.recording.PhoneRecordingStore
import us.disasm.agentix.settings.AssistantSettings

object RecordingServerSync {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var job: Job? = null
    private var client: RecordingUploadClient? = null
    var busy by mutableStateOf(false)
        private set
    var status by mutableStateOf("Server recording sync is opt-in. Local recordings are retained after upload.")
        private set

    fun enabled(context: Context): Boolean = context.getSharedPreferences("recording_server", Context.MODE_PRIVATE).getBoolean("enabled", false)
    fun enable(context: Context, enabled: Boolean): Boolean {
        val saved = context.getSharedPreferences("recording_server", Context.MODE_PRIVATE).edit().putBoolean("enabled", enabled).commit()
        if (saved && !enabled) cancel()
        return saved
    }

    fun start(context: Context) {
        if (busy) return
        val app = context.applicationContext
        val settings = AssistantSettings(app)
        if (!enabled(app) || !settings.enabled()) {
            status = "Enable the assistant backend and recording uploads before syncing to the server."
            return
        }
        val endpoint = settings.endpoint()
        val connection = RecordingUploadClient(settings.token())
        client = connection
        busy = true
        status = "Checking local recordings for $endpoint…"
        job = scope.launch {
            try {
                val recordings = withContext(Dispatchers.IO) {
                    val library = PhoneRecordingStore(app.filesDir).recoverAndList { PhoneCaptureState.activeFileName }
                    check(!library.hasReadErrors) { "Recording storage needs recovery" }
                    library.recordings.filter { it.fromPlaud }
                }
                val queue = RecordingUploadQueue(RecordingUploadJournal(app.filesDir))
                var confirmed = 0
                for (recording in recordings) {
                    status = "Syncing recording ${confirmed + 1} of ${recordings.size} to $endpoint…"
                    withContext(Dispatchers.IO) { queue.sync(recording.file, endpoint, connection) }
                    confirmed++
                    PhoneCaptureState.refreshLibrary()
                }
                status = "Server sync complete · $confirmed recordings confirmed. Transcription and search status appear in Inbox."
            } catch (cancelled: CancellationException) {
                status = "Server sync stopped. Local recordings and unresolved retry identities are retained."
                throw cancelled
            } catch (error: java.io.IOException) {
                status = error.message ?: "Server sync is unresolved. Local audio and retry state are retained."
            } catch (_: Exception) {
                status = "Server response or local upload state could not be validated. Retry with the same endpoint."
            } finally {
                busy = false
                client = null
                job = null
                PhoneCaptureState.refreshLibrary()
            }
        }
    }

    fun cancel() {
        if (!busy) return
        job?.cancel()
        client?.cancel()
    }
}
