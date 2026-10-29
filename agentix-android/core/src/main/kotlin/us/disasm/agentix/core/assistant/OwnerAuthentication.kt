package us.disasm.agentix.core.assistant

object OwnerAuthentication {
    fun authorization(token: String): String? {
        if (token.isEmpty()) return null
        require(token.length in 32..512 && token.all { it.code in 33..126 })
        return "Bearer $token"
    }
}
