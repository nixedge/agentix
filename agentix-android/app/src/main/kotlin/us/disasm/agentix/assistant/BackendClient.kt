package us.disasm.agentix.assistant

import android.content.Context
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.time.ZoneId
import java.util.UUID
import us.disasm.agentix.core.assistant.AssistantClient
import us.disasm.agentix.core.assistant.AssistantReply
import us.disasm.agentix.core.assistant.TellCommand
import us.disasm.agentix.core.assistant.TellEnvelope
import us.disasm.agentix.core.assistant.TellQueue
import us.disasm.agentix.core.assistant.TellResult
import us.disasm.agentix.core.assistant.OwnerAuthentication
import us.disasm.agentix.settings.AssistantSettings
import us.disasm.agentix.reminders.ReminderEngine
import us.disasm.agentix.reminders.ReminderWire

internal class BackendClient private constructor(context: Context) : AssistantClient {
    private val context = context.applicationContext
    private val settings = AssistantSettings(context)
    private val queue = TellQueue(settings)
    private var inFlight = false

    fun snapshot() = settings.load()

    override suspend fun tell(command: TellCommand): AssistantReply = withContext(Dispatchers.IO) {
        val (endpoint, token) = synchronized(AssistantSettings.lock) {
            if (!settings.enabled()) return@withContext AssistantReply.NotConnected
            check(!inFlight) { "Assistant request is still running" }
            val token = OwnerAuthentication.authorization(settings.token())
            inFlight = true
            settings.endpoint() to token
        }
        try {
            val deviceId = if (ReminderEngine.enabled(context)) ReminderEngine.deviceId(context) else null
            val envelope = synchronized(AssistantSettings.lock) {
                check(settings.endpoint() == endpoint) { "Assistant endpoint changed before request admission" }
                queue.begin(command.text, endpoint, ZoneId.systemDefault().id, deviceId)
            }
            val result = post(envelope, token)
            synchronized(AssistantSettings.lock) { queue.complete(envelope, result) }
            AssistantReply.Message(result.reply)
        } finally {
            synchronized(AssistantSettings.lock) { inFlight = false }
        }
    }

    private fun post(envelope: TellEnvelope, token: String?): TellResult {
        val url = URL(envelope.endpoint.trimEnd('/') + "/v1/assistant/tell")
        check(url.protocol == "https")
        val connection = url.openConnection() as HttpURLConnection
        val deadline = System.nanoTime() + 150_000_000_000L
        try {
            connection.instanceFollowRedirects = false
            connection.connectTimeout = 15_000
            connection.readTimeout = 135_000
            connection.requestMethod = "POST"
            connection.doOutput = true
            connection.setRequestProperty("Content-Type", "application/json; charset=utf-8")
            token?.let { connection.setRequestProperty("Authorization", it) }
            val request = JSONObject().put("request_id", envelope.requestId.toString())
                .put("conversation_id", envelope.conversationId.toString()).put("text", envelope.text)
                .put("timezone", envelope.timezone)
            val body = JSONObject().put("device_id", envelope.deviceId?.toString() ?: JSONObject.NULL)
                .put("request", request).toString().toByteArray(Charsets.UTF_8)
            connection.setFixedLengthStreamingMode(body.size)
            connection.outputStream.use { it.write(body) }
            check(connection.responseCode == 200) { "Assistant response is unresolved" }
            val bytes = connection.inputStream.use { input ->
                val output = java.io.ByteArrayOutputStream()
                val buffer = ByteArray(4096)
                while (true) {
                    val remaining = (deadline - System.nanoTime()) / 1_000_000L
                    check(remaining > 0) { "Assistant response timed out" }
                    connection.readTimeout = remaining.coerceAtMost(135_000L).toInt().coerceAtLeast(1)
                    val count = input.read(buffer)
                    if (count == -1) break
                    check(output.size() + count <= 131_072) { "Assistant response exceeds limit" }
                    output.write(buffer, 0, count)
                }
                output.toByteArray()
            }
            val root = JSONObject(bytes.toString(Charsets.UTF_8))
            val deviceId = if (root.isNull("device_id")) null else UUID.fromString(root.getString("device_id"))
            require(deviceId == envelope.deviceId)
            val receipts = root.getJSONArray("reminder_receipts")
            require(receipts.length() <= 1 && (receipts.length() == 0 || deviceId != null))
            val reminderReceipts = (0 until receipts.length()).map { ReminderWire.reminder(receipts.getJSONObject(it)) }
            val json = root.getJSONObject("response")
            fun string(name: String) = json.get(name) as? String ?: error("Invalid response field")
            val outcomes = json.getJSONArray("outcomes")
            val records = json.getJSONArray("records")
            for (index in 0 until outcomes.length()) {
                val receipt = outcomes.getJSONObject(index)
                UUID.fromString(receipt.getString("request_id"))
                validateRecord(receipt.getJSONObject("record"))
                check(receipt.getJSONObject("command").getString("id") == receipt.getJSONObject("record").getString("id"))
            }
            for (index in 0 until records.length()) validateRecord(records.getJSONObject(index))
            val result = TellResult(UUID.fromString(string("request_id")), UUID.fromString(string("conversation_id")),
                string("status"), string("reply"), outcomes.length(), records.length())
            require(result.requestId == envelope.requestId && result.conversationId == envelope.conversationId)
            if (reminderReceipts.isNotEmpty()) {
                require(result.status == "answer" && outcomes.length() == 0 && records.length() == 0)
                val receipt = reminderReceipts.single()
                ReminderEngine.synchronizeReceipt(context, receipt.id, receipt.revision)
                val localTime = receipt.dueAt.atZone(ZoneId.of(receipt.timezone)).format(java.time.format.DateTimeFormatter.ofPattern("EEE MMM d 'at' h:mm a z"))
                return result.copy(reply = "${receipt.text.take(256)} ($localTime): ${ReminderEngine.outcome(context, receipt.id, receipt.revision)}")
            }
            if (envelope.deviceId != null) {
                runCatching { ReminderEngine.sync(context) }
                ReminderEngine.requestSync(context)
            }
            return result
        } finally {
            connection.disconnect()
        }
    }

    private fun validateRecord(record: JSONObject) {
        check(UUID.fromString(record.getString("id")) != UUID(0, 0))
        check(record.getInt("schema_version") == 1 && record.getLong("revision") > 0)
        record.getBoolean("deleted")
        check(record.getJSONObject("body").getString("kind") in setOf("note", "task", "list", "list_item"))
    }

    companion object {
        @Volatile private var instance: BackendClient? = null
        fun get(context: Context): BackendClient = synchronized(AssistantSettings.lock) {
            instance ?: BackendClient(context.applicationContext).also { instance = it }
        }
    }
}
