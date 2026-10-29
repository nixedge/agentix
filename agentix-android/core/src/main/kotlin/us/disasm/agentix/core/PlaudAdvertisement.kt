package us.disasm.agentix.core

object PlaudAdvertisement {
    const val MANUFACTURER_ID = 0x0059
    private val projectCodes = setOf(880, 881, 882, 888)

    fun projectCode(manufacturerPayload: ByteArray?): Int? {
        if (manufacturerPayload == null || manufacturerPayload.size < 3) return null
        return (manufacturerPayload[1].toInt() and 0xff) or
            ((manufacturerPayload[2].toInt() and 0xff) shl 8)
    }

    fun matches(name: String?, manufacturerPayload: ByteArray?): Boolean =
        name?.startsWith("PLAUD", ignoreCase = true) == true ||
            projectCode(manufacturerPayload) in projectCodes
}
