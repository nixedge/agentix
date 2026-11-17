package us.disasm.agentix.core

object PlaudAdvertisement {
    const val MANUFACTURER_ID = 0x0059
    const val ALTERNATE_MANUFACTURER_ID = 0x0046
    private val projectCodes = setOf(880, 881, 882, 888)

    fun identity(raw: ByteArray?, manufacturerId: Int = MANUFACTURER_ID): String? {
        if (raw == null || raw.size !in 6..512 || raw[0].toInt() != 2) return null
        if (manufacturerId != MANUFACTURER_ID && manufacturerId != ALTERNATE_MANUFACTURER_ID) return null
        if (projectCode(raw) !in projectCodes) return null
        val width = (raw[3].toInt() and 255) - 1
        if (width !in 1..4 || raw.size <= 5 + width || raw[4].toInt() !in 'A'.code..'Z'.code) return null
        val lengthAt = 5 + width
        val length = raw[lengthAt].toInt() and 255
        if (length !in 1..32 || lengthAt + 1 + length > raw.size) return null
        val serial = raw.copyOfRange(lengthAt + 1, lengthAt + 1 + length)
        if (serial.all { it == 0.toByte() } || serial.all { it == (-1).toByte() }) return null
        val digest = java.security.MessageDigest.getInstance("SHA-256")
        digest.update("plaud-serial-v1\u0000".toByteArray(Charsets.US_ASCII))
        digest.update(raw.copyOfRange(1, 3))
        val key = digest.digest(serial).take(16).joinToString("") { "%02x".format(it.toInt() and 255) }
        return "serial:$key"
    }

    fun diagnostic(manufacturerPayload: ByteArray?, manufacturerId: Int = MANUFACTURER_ID): String {
        val manufacturer = "Manufacturer 0x%04x".format(manufacturerId)
        val raw = manufacturerPayload ?: return "$manufacturer: not present\nAdvertised firmware: unavailable"
        fun unavailable(reason: String): String = "$manufacturer: ${raw.size} bytes\nAdvertised firmware: unavailable ($reason)\nPayload omitted: layout could contain serial data"
        if (manufacturerId != MANUFACTURER_ID && manufacturerId != ALTERNATE_MANUFACTURER_ID) return unavailable("unsupported manufacturer")
        if (raw.size !in 6..512 || raw[0].toInt() != 2) return unavailable("unsupported project layout")
        val project = projectCode(raw)
        if (project !in projectCodes) return unavailable("unsupported project code")
        val fieldSize = raw[3].toInt() and 255
        val width = fieldSize - 1
        if (width !in 1..4 || raw.size <= 4 + fieldSize) return unavailable("truncated or unsupported version field")
        val type = raw[4].toInt() and 255
        if (type !in 'A'.code..'Z'.code) return unavailable("unsupported version type")
        var version = 0L
        repeat(width) { index -> version = version or ((raw[5 + index].toLong() and 255) shl (8 * index)) }
        val serialLengthAt = 4 + fieldSize
        val serialLength = raw[serialLengthAt].toInt() and 255
        val serialEnd = serialLengthAt + 1 + serialLength
        if (serialLength == 0 || serialEnd > raw.size) return unavailable("truncated serial layout")
        val protocolAt = if (project == 888 || project == 881) 18 else 15
        val protocol = if (serialLength < 10 && protocolAt >= serialEnd && protocolAt + 2 <= raw.size) {
            ((raw[protocolAt].toInt() and 255) or ((raw[protocolAt + 1].toInt() and 255) shl 8)).toString()
        } else "unavailable (unsupported or truncated protocol layout)"
        val prefix = raw.take(serialLengthAt).joinToString(" ") { "%02x".format(it.toInt() and 255) }
        return "$manufacturer: ${raw.size} bytes\nAdvertised project code: $project (not model verification)\nAdvertised firmware: ${type.toChar()}${version.toString().padStart(4, '0')} (numeric $version)\nAdvertised protocol: $protocol (not authenticated)\nProject/version prefix: $prefix\nSerial number and remaining payload omitted"
    }

    fun projectCode(manufacturerPayload: ByteArray?): Int? {
        if (manufacturerPayload == null || manufacturerPayload.size < 3) return null
        return (manufacturerPayload[1].toInt() and 0xff) or
            ((manufacturerPayload[2].toInt() and 0xff) shl 8)
    }

    fun matches(name: String?, manufacturerPayload: ByteArray?): Boolean =
        name?.startsWith("PLAUD", ignoreCase = true) == true ||
            projectCode(manufacturerPayload) in projectCodes
}
