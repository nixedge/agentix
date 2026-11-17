package us.disasm.agentix.core.plaud

import java.io.OutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder

internal class OggOpus(private val output: OutputStream, private val serial: Int) {
    private var sequence = 0

    fun headers() {
        val head = ByteBuffer.allocate(19).order(ByteOrder.LITTLE_ENDIAN).apply {
            put("OpusHead".toByteArray(Charsets.US_ASCII))
            put(1); put(1); putShort(0); putInt(16_000); putShort(0); put(0)
        }.array()
        page(head, 0, 2)
        val vendor = "Agentix".toByteArray(Charsets.US_ASCII)
        val tags = ByteBuffer.allocate(16 + vendor.size).order(ByteOrder.LITTLE_ENDIAN).apply {
            put("OpusTags".toByteArray(Charsets.US_ASCII)); putInt(vendor.size); put(vendor); putInt(0)
        }.array()
        page(tags, 0, 0)
    }

    fun frame(packet: ByteArray, frameNumber: Long, last: Boolean) = page(packet, frameNumber * 960, if (last) 4 else 0)

    private fun page(packet: ByteArray, granule: Long, flags: Int) {
        require(packet.size < 255)
        val bytes = ByteBuffer.allocate(28 + packet.size).order(ByteOrder.LITTLE_ENDIAN).apply {
            put("OggS".toByteArray(Charsets.US_ASCII)); put(0); put(flags.toByte()); putLong(granule)
            putInt(serial); putInt(sequence++); putInt(0); put(1); put(packet.size.toByte()); put(packet)
        }.array()
        var crc = 0
        bytes.forEach { byte ->
            crc = crc xor ((byte.toInt() and 255) shl 24)
            repeat(8) { crc = (crc shl 1) xor (if (crc < 0) 0x04c11db7 else 0) }
        }
        ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN).putInt(22, crc)
        output.write(bytes)
    }
}
