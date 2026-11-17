package us.disasm.agentix.core.plaud

/** Only the separately confirmed binding migration uses this command. Ordinary sync cannot unbind. */
object PlaudBinding {
    fun clearBindingPreservingRecordings(): ByteArray = byteArrayOf(1, 5, 0, 0)
}
