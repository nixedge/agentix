package us.disasm.agentix.device

import android.content.Context
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import java.io.IOException
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import us.disasm.agentix.core.plaud.PlaudLibrary
import us.disasm.agentix.core.plaud.PlaudBinding
import us.disasm.agentix.core.plaud.PlaudProgress
import us.disasm.agentix.core.plaud.PlaudProtocol
import us.disasm.agentix.core.plaud.PlaudSync
import us.disasm.agentix.recording.PhoneCaptureState

class PlaudSyncController(private val context: Context, private val scope: CoroutineScope) {
    private val preferences = context.getSharedPreferences("plaud_binding", Context.MODE_PRIVATE)
    private val handler = Handler(Looper.getMainLooper())
    private var job: Job? = null
    private var transport: PlaudGatt? = null
    private var generation = 0L
    var busy by mutableStateOf(false)
        private set
    var progress by mutableStateOf(PlaudProgress("Select a recorder to sync. Recordings stay on the Plaud."))
        private set
    var failed by mutableStateOf(false)
        private set

    fun hasToken(address: String): Boolean = preferences.contains(address)

    fun saveToken(address: String, token: String): Boolean {
        if (busy || !PlaudProtocol.validToken(token)) return false
        return preferences.edit().putString(address, token).commit()
    }

    fun sync(address: String) {
        if (busy) return
        val token = preferences.getString(address, null)
        if (token == null) {
            progress = PlaudProgress("Add your existing Plaud binding token before syncing.")
            failed = true
            return
        }
        val current = ++generation
        failed = false
        busy = true
        progress = PlaudProgress("Connecting to Plaud…")
        val connection = PlaudGatt(context)
        transport = connection
        job = scope.launch {
            try {
                val result = withTimeout(2 * 60 * 60 * 1000L) {
                    withContext(Dispatchers.IO) {
                        connection.open(address)
                        var lastReport = 0L
                        var lastMessage = ""
                        PlaudSync(connection, PlaudLibrary(context.filesDir)).run(address, token) { update ->
                            val now = System.nanoTime()
                            if (update.message != lastMessage || now - lastReport >= 200_000_000L) {
                                lastReport = now
                                lastMessage = update.message
                                handler.post { if (generation == current) progress = update }
                            }
                        }
                    }
                }
                progress = PlaudProgress("Sync complete · ${result.imported} imported, ${result.existing} already in Inbox · protocol ${result.protocol}", result.imported)
                if (RecordingServerSync.enabled(context)) RecordingServerSync.start(context)
            } catch (_: TimeoutCancellationException) {
                failed = true
                progress = PlaudProgress("Recorder timed out. Keep it awake and nearby, then retry to resume.")
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: SecurityException) {
                failed = true
                progress = PlaudProgress("Bluetooth permission was removed. Grant Nearby devices and retry.")
            } catch (error: IOException) {
                failed = true
                progress = PlaudProgress(error.message ?: "Could not save Plaud recording. Partial data retained.")
            } catch (_: IllegalArgumentException) {
                failed = true
                progress = PlaudProgress("Invalid recorder address or binding token.")
            } catch (_: IllegalStateException) {
                failed = true
                progress = PlaudProgress("Recorder connection is unavailable. Turn on Bluetooth and retry.")
            } finally {
                connection.close()
                if (generation == current) {
                    generation++
                    busy = false
                    transport = null
                    job = null
                    PhoneCaptureState.refreshLibrary()
                }
            }
        }
    }

    fun switchBindingAfterConfirmation(address: String) {
        if (busy) return
        val token = preferences.getString(address, null) ?: UUID.randomUUID().toString().replace("-", "")
        if (!saveToken(address, token)) {
            failed = true
            progress = PlaudProgress("Could not persist the Agentix binding token. Recorder unchanged.")
            return
        }
        val current = ++generation
        busy = true
        failed = false
        progress = PlaudProgress("Checking recorder protocol before switching binding…")
        val connection = PlaudGatt(context)
        transport = connection
        job = scope.launch {
            var sent = false
            try {
                connection.open(address)
                connection.send(PlaudProtocol.handshake(token))
                val reply = PlaudProtocol.decode(connection.receive())
                if (reply.command != 1) throw IOException("Unexpected response; binding unchanged")
                PlaudProtocol.protocolVersion(reply.payload)
                val status = reply.payload[0].toInt() and 255
                if (status == 1) {
                    sent = true
                    connection.send(PlaudBinding.clearBindingPreservingRecordings())
                    progress = PlaudProgress("Binding switch sent. Wake the Plaud after it restarts, then select Sync recordings to bind to Agentix.")
                } else {
                    PlaudProtocol.authenticate(reply.payload)
                    progress = PlaudProgress("Recorder already accepts the Agentix token. Select Sync recordings.")
                }
            } catch (_: TimeoutCancellationException) {
                failed = true
                progress = PlaudProgress(if (sent) "Binding switch may have completed. Wake the Plaud and try Sync recordings before switching again." else "Recorder timed out; binding unchanged.")
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: SecurityException) {
                failed = true
                progress = PlaudProgress("Bluetooth permission is required. Recorder binding outcome is unconfirmed.")
            } catch (error: IOException) {
                failed = true
                progress = PlaudProgress(if (sent) "Plaud disconnected after the binding switch. Wake it and try Sync recordings. Recordings were not requested for deletion." else error.message ?: "Could not inspect recorder; binding unchanged.")
            } catch (_: IllegalArgumentException) {
                failed = true
                progress = PlaudProgress("Invalid recorder address or binding token.")
            } catch (_: IllegalStateException) {
                failed = true
                progress = PlaudProgress("Recorder unavailable; binding outcome is unconfirmed.")
            } finally {
                connection.close()
                if (generation == current) {
                    generation++
                    busy = false
                    transport = null
                    job = null
                }
            }
        }
    }

    fun cancel() {
        if (!busy) return
        generation++
        job?.cancel()
        transport?.close()
        transport = null
        // Keep the operation busy until its IO worker has released the partial file.
        val stopping = job
        scope.launch {
            stopping?.join()
            job = null
            busy = false
            failed = false
            progress = PlaudProgress("Sync stopped. Saved recordings and partial downloads are retained.")
            PhoneCaptureState.refreshLibrary()
        }
    }
}
