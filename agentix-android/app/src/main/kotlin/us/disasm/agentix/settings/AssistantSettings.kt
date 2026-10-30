package us.disasm.agentix.settings

import android.annotation.SuppressLint
import android.content.Context
import org.json.JSONObject
import java.util.UUID
import us.disasm.agentix.core.AssistantEndpoint
import us.disasm.agentix.core.assistant.TellEnvelope
import us.disasm.agentix.core.assistant.TellJournal
import us.disasm.agentix.core.assistant.TellQueueState
import us.disasm.agentix.core.assistant.TellResult
import us.disasm.agentix.core.assistant.OwnerAuthentication

class AssistantSettings(context: Context) : TellJournal {
    private val preferences = context.getSharedPreferences("assistant", Context.MODE_PRIVATE)

    fun endpoint(): String = preferences.getString("endpoint", "https://ai.lan.disasm.us").orEmpty()
    fun enabled(): Boolean = preferences.getBoolean("enabled", false)
    fun token(): String = preferences.getString("owner_token", "").orEmpty()

    fun configure(input: String, token: String, enabled: Boolean): Boolean = synchronized(lock) {
        val normalized = AssistantEndpoint.normalize(input) ?: return false
        if (runCatching { OwnerAuthentication.authorization(token) }.isFailure) return false
        val pending = load().pending
        if (pending != null && pending.endpoint != normalized) return false
        persist(preferences.edit().putString("endpoint", normalized)
            .putString("owner_token", token).putBoolean("enabled", enabled))
        true
    }

    fun clearEndpoint(): Boolean = synchronized(lock) {
        if (load().pending != null) return false
        persist(preferences.edit().putString("endpoint", "").remove("owner_token").putBoolean("enabled", false))
        true
    }

    override fun load(): TellQueueState = synchronized(lock) {
        check(!faulted) { "Assistant storage needs recovery" }
        val raw = preferences.getString("tell_queue", null)
            ?: return TellQueueState(UUID.randomUUID())
        val json = JSONObject(raw)
        require(json.getInt("version") == 1)
        val conversation = UUID.fromString(json.getString("conversation_id"))
        val pending = if (json.isNull("pending")) null else json.getJSONObject("pending").let {
            TellEnvelope(UUID.fromString(it.getString("request_id")), conversation,
                it.getString("text"), it.getString("timezone"), it.getString("endpoint"),
                if (it.isNull("device_id")) null else UUID.fromString(it.getString("device_id")))
        }
        val result = if (json.isNull("last_result")) null else json.getJSONObject("last_result").let {
            TellResult(UUID.fromString(it.getString("request_id")), conversation,
                it.getString("status"), it.getString("reply"), it.getInt("outcomes"), it.getInt("records"))
        }
        TellQueueState(conversation, pending, result)
    }

    override fun store(state: TellQueueState) = synchronized(lock) {
        val pending = state.pending?.let {
            JSONObject().put("request_id", it.requestId.toString()).put("text", it.text)
                .put("timezone", it.timezone).put("endpoint", it.endpoint)
                .put("device_id", it.deviceId ?: JSONObject.NULL)
        }
        val result = state.lastResult?.let {
            JSONObject().put("request_id", it.requestId.toString()).put("status", it.status)
                .put("reply", it.reply).put("outcomes", it.outcomes).put("records", it.records)
        }
        val json = JSONObject().put("version", 1).put("conversation_id", state.conversationId.toString())
            .put("pending", pending ?: JSONObject.NULL).put("last_result", result ?: JSONObject.NULL)
        persist(preferences.edit().putString("tell_queue", json.toString()))
    }

    @SuppressLint("ApplySharedPref")
    private fun persist(edit: android.content.SharedPreferences.Editor) {
        check(!faulted) { "Assistant storage needs recovery" }
        // A failed commit can update preferences in memory: freeze this process to prevent a new ID.
        if (!edit.commit()) {
            faulted = true
            error("Assistant state could not be persisted")
        }
    }

    companion object {
        val lock = Any()
        private var faulted = false
    }
}
