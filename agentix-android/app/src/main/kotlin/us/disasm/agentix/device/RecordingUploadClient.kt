package us.disasm.agentix.device

import android.os.Handler
import android.os.Looper
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.time.Instant
import java.util.UUID
import org.json.JSONObject
import us.disasm.agentix.core.assistant.OwnerAuthentication
import us.disasm.agentix.core.plaud.*

class RecordingUploadClient(private val token: String) : RecordingUploadApi {
    @Volatile private var active: HttpURLConnection? = null
    private val handler = Handler(Looper.getMainLooper())
    fun cancel() { active?.disconnect() }

    override suspend fun get(entry: RecordingUploadEntry): RecordingServerReceipt? = request(entry, null)
    override suspend fun put(entry: RecordingUploadEntry, file: File): RecordingServerReceipt =
        request(entry, file) ?: throw IOException("Recording server did not accept the upload")

    private fun request(entry: RecordingUploadEntry, file: File?): RecordingServerReceipt? {
        val url = URL(entry.endpoint.trimEnd('/') + "/v1/assistant/recordings/${entry.metadata.id}")
        if (url.protocol != "https") throw IOException("Recording uploads require HTTPS")
        val connection = url.openConnection() as HttpURLConnection
        val timeout = Runnable { connection.disconnect() }
        active = connection
        handler.postDelayed(timeout, 150_000)
        try {
            connection.instanceFollowRedirects = false
            connection.connectTimeout = 15_000; connection.readTimeout = 120_000
            OwnerAuthentication.authorization(token)?.let { connection.setRequestProperty("Authorization", it) }
            if (file != null) {
                connection.requestMethod = "PUT"; connection.doOutput = true
                connection.setRequestProperty("Content-Type", "audio/ogg")
                connection.setRequestProperty("X-Agentix-Recording-Metadata", metadata(entry.metadata).toString())
                connection.setFixedLengthStreamingMode(entry.metadata.audioBytes)
                file.inputStream().use { input -> connection.outputStream.use { output -> input.copyTo(output, 65_536) } }
            } else connection.requestMethod = "GET"
            val code = connection.responseCode
            if (code == 404 && file == null) return null
            if (code != 200) throw IOException("Server recording sync failed (HTTP $code). Local audio and retry identity are retained.")
            val bytes = connection.inputStream.use { input ->
                val output = ByteArrayOutputStream(); val buffer = ByteArray(4096)
                while (true) {
                    val count = input.read(buffer); if (count < 0) break
                    if (output.size() + count > 32_768) throw IOException("Recording receipt exceeds limit")
                    output.write(buffer, 0, count)
                }
                output.toByteArray()
            }
            return receipt(JSONObject(bytes.toString(Charsets.UTF_8)))
        } finally {
            handler.removeCallbacks(timeout)
            active = null
            connection.disconnect()
        }
    }

    private fun metadata(m: RecordingUploadMetadata) = JSONObject().put("id", m.id.toString()).put("source_device", m.sourceDevice)
        .put("source_session", m.sourceSession).put("source_bytes", m.sourceBytes).put("recorded_at", m.recordedAt)
        .put("duration_ms", m.durationMs).put("audio_bytes", m.audioBytes).put("sha256", m.sha256).put("media_type", m.mediaType)

    private fun receipt(root: JSONObject): RecordingServerReceipt {
        val stored = root.getJSONObject("recording"); val m = stored.getJSONObject("metadata")
        val metadata = RecordingUploadMetadata(UUID.fromString(m.getString("id")), m.getString("source_device"), m.getLong("source_session"), m.getLong("source_bytes"),
            Instant.parse(m.getString("recorded_at")).toString(), m.getLong("duration_ms"), m.getLong("audio_bytes"), m.getString("sha256"), m.getString("media_type"))
        Instant.parse(stored.getString("stored_at"))
        val transcribed = !stored.isNull("transcribed_at")
        if (transcribed) Instant.parse(stored.getString("transcribed_at"))
        val records = stored.getJSONArray("transcript_record_ids")
        require(records.length() <= 256)
        for (index in 0 until records.length()) require(UUID.fromString(records.getString(index)) != UUID(0, 0))
        return RecordingServerReceipt(UUID.fromString(root.getString("server_id")), metadata, transcribed, root.getBoolean("search_pending"), !stored.isNull("transcription_error"))
    }
}
