package us.disasm.agentix.core

import java.net.URI
import java.net.URISyntaxException

object AssistantEndpoint {
    fun normalize(input: String): String? {
        val uri = try {
            URI(input.trim())
        } catch (_: URISyntaxException) {
            return null
        }
        if (!uri.scheme.equals("https", ignoreCase = true) || uri.host.isNullOrBlank()) return null
        if (uri.rawUserInfo != null || uri.rawQuery != null || uri.rawFragment != null) return null
        if (uri.port != -1 && uri.port !in 1..65535) return null
        return uri.normalize().toASCIIString()
    }
}
