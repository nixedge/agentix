package us.disasm.agentix.reminders

import org.json.JSONArray
import org.json.JSONObject
import us.disasm.agentix.core.reminders.*
import java.net.HttpURLConnection
import java.net.URL
import java.time.Instant
import java.util.Locale
import java.util.UUID

internal object ReminderWire {
    fun reminder(json: JSONObject) = Reminder(UUID.fromString(json.getString("id")), json.getLong("revision"), json.getString("text"), Instant.parse(json.getString("due_at")), json.getString("timezone"), json.getBoolean("cancelled"))
    fun reminder(value: Reminder) = JSONObject().put("id", value.id).put("revision", value.revision).put("text", value.text).put("due_at", value.dueAt).put("timezone", value.timezone).put("cancelled", value.cancelled)
    fun observation(json: JSONObject) = Observation(UUID.fromString(json.getString("report_id")), UUID.fromString(json.getString("reminder_id")), json.getLong("revision"), json.getLong("sequence"), Delivery.valueOf(json.getString("state").uppercase(Locale.ROOT)))
    fun observation(value: Observation) = JSONObject().put("report_id", value.reportId).put("reminder_id", value.reminderId).put("revision", value.revision).put("sequence", value.sequence).put("state", value.state.name.lowercase(Locale.ROOT))
    fun snapshot(json: JSONObject): Snapshot {
        return Snapshot(UUID.fromString(json.getString("server_id")), UUID.fromString(json.getString("device_id")), json.getLong("sequence"),
            json.getJSONArray("reminders").objects().map(::reminder), json.getJSONArray("observations").objects().map(::observation))
    }
    fun journal(json: JSONObject): Journal {
        require(json.getInt("version") == 1)
        return Journal(json.getString("endpoint"), UUID.fromString(json.getString("device_id")), if (json.isNull("server_id")) null else UUID.fromString(json.getString("server_id")),
            json.getLong("snapshot_sequence"), json.getLong("next_report"), json.getJSONArray("entries").objects().map {
                Entry(reminder(it.getJSONObject("reminder")), Delivery.valueOf(it.getString("phase")), if (it.isNull("reported")) null else Delivery.valueOf(it.getString("reported")))
            }, json.getJSONArray("reports").objects().map(::observation))
    }
    fun journal(value: Journal) = JSONObject().put("version", 1).put("endpoint", value.endpoint).put("device_id", value.deviceId).put("server_id", value.serverId ?: JSONObject.NULL)
        .put("snapshot_sequence", value.snapshotSequence).put("next_report", value.nextReport)
        .put("entries", JSONArray(value.entries.map { JSONObject().put("reminder", reminder(it.reminder)).put("phase", it.phase.name).put("reported", it.reported?.name ?: JSONObject.NULL) }))
        .put("reports", JSONArray(value.reports.map(::observation)))
    fun request(endpoint: String, path: String, authorization: String?, body: JSONObject? = null, timeout: Int = 30000): JSONObject {
        val url = URL(endpoint.trimEnd('/') + path)
        require(url.protocol == "https" && url.userInfo == null)
        val connection = url.openConnection() as HttpURLConnection
        val deadline = System.nanoTime() + timeout.toLong() * 1000000
        try {
            connection.instanceFollowRedirects = false
            connection.connectTimeout = minOf(timeout, 15000)
            connection.readTimeout = timeout
            connection.requestMethod = if (body == null) "GET" else "POST"
            authorization?.let { connection.setRequestProperty("Authorization", it) }
            if (body != null) {
                val bytes = body.toString().toByteArray(Charsets.UTF_8)
                require(bytes.size <= 16384) { "Reminder request exceeds limit" }
                connection.doOutput = true
                connection.setRequestProperty("Content-Type", "application/json; charset=utf-8")
                connection.setFixedLengthStreamingMode(bytes.size)
                connection.outputStream.use { it.write(bytes) }
            }
            val status = connection.responseCode
            check(status == 200) { "Reminder request failed (HTTP $status)" }
            val bytes = connection.inputStream.use { input ->
                val out = java.io.ByteArrayOutputStream()
                val buffer = ByteArray(4096)
                while (true) {
                    val remaining = (deadline - System.nanoTime()) / 1000000
                    check(remaining > 0) { "Reminder response timed out" }
                    connection.readTimeout = remaining.coerceAtMost(timeout.toLong()).toInt().coerceAtLeast(1)
                    val count = input.read(buffer)
                    if (count == -1) break
                    check(out.size() + count <= 2097152) { "Reminder response exceeds limit" }
                    out.write(buffer, 0, count)
                }
                out.toByteArray()
            }
            return JSONObject(bytes.toString(Charsets.UTF_8))
        } finally { connection.disconnect() }
    }
    private fun JSONArray.objects(): List<JSONObject> { require(length() <= 1000); return (0 until length()).map { getJSONObject(it) } }
}
