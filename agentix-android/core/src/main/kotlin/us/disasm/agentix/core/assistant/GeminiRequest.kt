package us.disasm.agentix.core.assistant

sealed interface GeminiRequest {
    data object NotRequested : GeminiRequest
    data object MissingQuestion : GeminiRequest
    data object TooLong : GeminiRequest
    data class Question(val text: String) : GeminiRequest
}
private val geminiPrefix = Regex("^(?:agentix[,:]?\\s+)?ask\\s+gemini(?=\\s|:|,|$)[,:]?\\s*", RegexOption.IGNORE_CASE)
fun geminiRequest(text: String, hasPendingBackendTurn: Boolean = false): GeminiRequest {
    if (hasPendingBackendTurn) return GeminiRequest.NotRequested
    val input = text.trim()
    val prefix = geminiPrefix.find(input) ?: return GeminiRequest.NotRequested
    val question = input.substring(prefix.range.last + 1).trim()
    return when {
        question.isBlank() -> GeminiRequest.MissingQuestion
        question.toByteArray(Charsets.UTF_8).size > 8192 -> GeminiRequest.TooLong
        else -> GeminiRequest.Question(question)
    }
}
