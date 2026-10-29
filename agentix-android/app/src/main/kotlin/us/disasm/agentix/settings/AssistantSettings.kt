package us.disasm.agentix.settings

import android.content.Context
import us.disasm.agentix.core.AssistantEndpoint

class AssistantSettings(context: Context) {
    private val preferences = context.getSharedPreferences("assistant", Context.MODE_PRIVATE)

    fun endpoint(): String = preferences.getString("endpoint", "").orEmpty()

    fun saveEndpoint(input: String): Boolean {
        val normalized = AssistantEndpoint.normalize(input) ?: return false
        preferences.edit().putString("endpoint", normalized).apply()
        return true
    }

    fun clearEndpoint() {
        preferences.edit().remove("endpoint").apply()
    }
}
