package us.disasm.agentix.core.assistant

/** All operations belong to the caller's serialized control thread, never the audio worker. */
class WakeWordGate {
    private val holds = mutableSetOf<Any>()
    private val waiting = mutableMapOf<Any, () -> Unit>()
    var capturing = false
        private set
    val held: Boolean get() = holds.isNotEmpty()

    fun hold(owner: Any, whenQuiet: () -> Unit) {
        holds.add(owner)
        if (capturing) waiting[owner] = whenQuiet else whenQuiet()
    }

    fun release(owner: Any) {
        holds.remove(owner)
        waiting.remove(owner)
    }

    fun reserve(): Boolean {
        if (capturing || held) return false
        capturing = true
        return true
    }

    fun quiet() {
        capturing = false
        val callbacks = waiting.toList()
        waiting.clear()
        callbacks.forEach { (owner, callback) -> if (owner in holds) callback() }
    }
}

class WakeAdmission(private val cooldownMillis: Long = 3000) {
    private var generation = 0L
    private var lastAccepted: Long? = null
    fun begin(): Long { generation++; return generation }
    fun invalidate() { generation++ }
    fun accept(token: Long, elapsedMillis: Long, allowed: Boolean): Boolean {
        if (!allowed || token != generation) return false
        val last = lastAccepted
        if (last != null && elapsedMillis - last < cooldownMillis) return false
        lastAccepted = elapsedMillis
        generation++
        return true
    }
}
