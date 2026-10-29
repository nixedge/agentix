package us.disasm.agentix.core.audio

import java.io.File
import java.io.IOException
import java.io.RandomAccessFile
import java.nio.ByteBuffer
import java.nio.ByteOrder

object PcmWave {
    const val SAMPLE_RATE = 16_000
    const val HEADER_BYTES = 44
    const val FRAME_BYTES = 2
    const val BYTES_PER_SECOND = SAMPLE_RATE * FRAME_BYTES
    const val MAX_DATA_BYTES = 0xffff_ffffL - 37L

    fun header(dataBytes: Long): ByteArray {
        require(dataBytes in 0..MAX_DATA_BYTES && dataBytes % FRAME_BYTES == 0L)
        return ByteBuffer.allocate(HEADER_BYTES).order(ByteOrder.LITTLE_ENDIAN).apply {
            put("RIFF".toByteArray(Charsets.US_ASCII))
            putInt((dataBytes + 36).toInt())
            put("WAVEfmt ".toByteArray(Charsets.US_ASCII))
            putInt(16)
            putShort(1)
            putShort(1)
            putInt(SAMPLE_RATE)
            putInt(BYTES_PER_SECOND)
            putShort(FRAME_BYTES.toShort())
            putShort(16)
            put("data".toByteArray(Charsets.US_ASCII))
            putInt(dataBytes.toInt())
        }.array()
    }

    fun finalizeFile(partial: File, destination: File): File? {
        require(partial != destination)
        if (!partial.isFile) throw IOException("Recording file is missing")
        val available = partial.length() - HEADER_BYTES
        if (available < FRAME_BYTES) {
            if (!partial.delete()) throw IOException("Cannot remove empty recording")
            return null
        }
        val dataBytes = available - available % FRAME_BYTES
        if (dataBytes > MAX_DATA_BYTES) throw IOException("Recording exceeds WAV size limit")
        if (destination.exists()) throw IOException("Recording destination already exists")
        RandomAccessFile(partial, "rw").use { file ->
            file.setLength(HEADER_BYTES + dataBytes)
            file.seek(0)
            file.write(header(dataBytes))
            file.fd.sync()
        }
        if (!partial.renameTo(destination)) throw IOException("Cannot finalize recording")
        return destination
    }
}
