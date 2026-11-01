package us.disasm.agentix.wake

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import us.disasm.agentix.core.assistant.WakeWordGate

internal object WakeWordRuntime {
    private val gate = WakeWordGate()
    var status by mutableStateOf("Stopped. Start listening from Agentix assistant setup.")
    var running by mutableStateOf(false)
    var changed: (() -> Unit)? = null
    val held get() = gate.held

    fun hold(owner: Any, whenQuiet: () -> Unit = {}) {
        gate.hold(owner, whenQuiet)
        changed?.invoke()
    }
    fun release(owner: Any) { gate.release(owner); changed?.invoke() }
    fun reserve() = gate.reserve()
    fun quiet() = gate.quiet()
}
