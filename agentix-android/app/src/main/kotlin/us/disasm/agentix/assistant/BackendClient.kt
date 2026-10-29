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
import us.disasm.agentix.settings.AssistantSettings

internal class BackendClient private constructor(context: Context) : AssistantClient {
    private val settings = AssistantSettings(context)
    private val queue = TellQueue(settings)
    private var inFlight = false

    fun snapshot() = settings.load()

    override suspend fun tell(command: TellCommand): AssistantReply = withContext(Dispatchers.IO) {
        val (envelope, token) = synchronized(AssistantSettings.lock) {
            if (!settings.enabled()) return@withContext AssistantReply.NotConnected
            check(!inFlight) { "Assistant request is still running" }
            val token = settings.token()
            check(token.length in 32..512 && token.all { it.code in 33..126 })
            val pending = queue.begin(command.text, settings.endpoint(), ZoneId.systemDefault().id)
            inFlight = true
            pending to token
        }
        try {
            val result = post(envelope, token)
            synchronized(AssistantSettings.lock) { queue.complete(envelope, result) }
            AssistantReply.Message(result.reply)
        } finally {
            synchronized(AssistantSettings.lock) { inFlight = false }
        }
    }

    private fun post(envelope: TellEnvelope, token: String): TellResult {
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
            connection.setRequestProperty("Authorization", "Bearer $token")
            val body = JSONObject().put("request_id", envelope.requestId.toString())
                .put("conversation_id", envelope.conversationId.toString()).put("text", envelope.text)
                .put("timezone", envelope.timezone).toString().toByteArray(Charsets.UTF_8)
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
            val json = JSONObject(bytes.toString(Charsets.UTF_8))
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
            return TellResult(UUID.fromString(string("request_id")), UUID.fromString(string("conversation_id")),
                string("status"), string("reply"), outcomes.length(), records.length())
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
