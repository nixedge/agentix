package us.disasm.agentix.core.plaud

import java.io.IOException
import java.nio.ByteBuffer
import java.nio.ByteOrder

class PlaudException(message: String) : IOException(message)

data class RemoteRecording(val sessionId: Long, val size: Long, val scene: Int) {
    init {
        require(sessionId in 1..0xffff_ffffL)
        require(size in 80..MAX_BYTES && size % 80 == 0L)
        require(scene in 0..65535)
    }
    val durationMillis: Long get() = size / 80 * 20

    companion object { const val MAX_BYTES = 256L * 1024 * 1024 }
}

data class PlaudMessage(val type: Int, val command: Int?, val payload: ByteArray)

object PlaudProtocol {
    const val RECORD_BYTES = 89
    private fun buffer(size: Int) = ByteBuffer.allocate(size).order(ByteOrder.LITTLE_ENDIAN)
    private fun command(id: Int, payload: ByteArray = byteArrayOf()): ByteArray =
        buffer(3 + payload.size).put(1).putShort(id.toShort()).put(payload).array()

    fun validToken(token: String): Boolean = token.length == 32 && token.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' }

    fun handshake(token: String): ByteArray {
        require(validToken(token)) { "Binding token must contain 32 hexadecimal characters" }
        return command(1, byteArrayOf(2, 0, 0) + token.toByteArray(Charsets.US_ASCII))
    }

    fun list(): ByteArray = command(26, ByteArray(17))

    fun download(recording: RemoteRecording, position: Long): ByteArray {
        require(position in 0..recording.size && position % 80 == 0L)
        return command(28, buffer(12).putInt(recording.sessionId.toInt()).putInt(position.toInt()).putInt(recording.size.toInt()).array())
    }

    fun checksum(crc: Int): ByteArray = command(116, buffer(3).put(0).putShort(crc.toShort()).array())

    fun decode(bytes: ByteArray): PlaudMessage {
        if (bytes.isEmpty()) throw PlaudException("Empty recorder notification")
        return when (bytes[0].toInt() and 255) {
            1 -> {
                if (bytes.size < 3) throw PlaudException("Truncated recorder command")
                PlaudMessage(1, u16(bytes, 1), bytes.copyOfRange(3, bytes.size))
            }
            2 -> PlaudMessage(2, null, bytes.copyOfRange(1, bytes.size))
            else -> throw PlaudException("Unsupported recorder notification type")
        }
    }

    fun authenticate(payload: ByteArray): Int {
        val version = protocolVersion(payload)
        when (val status = payload[0].toInt() and 255) {
            0 -> return version
            1 -> throw PlaudException("Binding token mismatch (protocol $version). Supply the existing Plaud binding token or explicitly switch this recorder to Agentix. No binding or settings were cleared.")
            2 -> throw PlaudException("Plaud is recording. Stop recording on the device, then retry.")
            else -> throw PlaudException("Plaud authentication rejected (status $status, protocol $version)")
        }
    }

    fun protocolVersion(payload: ByteArray): Int {
        if (payload.size < 3) throw PlaudException("Truncated authentication response")
        val version = u16(payload, 1)
        if (version != 10) throw PlaudException("Recorder protocol $version is unsupported; this build supports protocol 10")
        return version
    }

    fun recordings(payload: ByteArray): List<RemoteRecording> {
        if (payload.size < 8) throw PlaudException("Truncated recording list")
        val count = u32(payload, 4)
        if (count > 10_000 || payload.size.toLong() != 8 + count * 10) {
            throw PlaudException("Incomplete or unsupported recording list; no recordings imported")
        }
        val result = (0 until count.toInt()).map { index ->
            val at = 8 + index * 10
            try {
                RemoteRecording(u32(payload, at), u32(payload, at + 4), u16(payload, at + 8))
            } catch (_: IllegalArgumentException) {
                throw PlaudException("Unsupported recording metadata or audio size")
            }
        }
        if (result.map { it.sessionId }.distinct().size != result.size) throw PlaudException("Duplicate recording sessions in device list")
        return result
    }

    fun checkHead(payload: ByteArray, recording: RemoteRecording) {
        if (payload.size != 5 || u32(payload, 0) != recording.sessionId) throw PlaudException("Unexpected file transfer header")
        val status = payload[4].toInt() and 255
        if (status != 0) throw PlaudException("Plaud rejected recording download (status $status)")
    }

    fun tailCrc(payload: ByteArray, recording: RemoteRecording): Int {
        if (payload.size != 6 || u32(payload, 0) != recording.sessionId) throw PlaudException("Unexpected file transfer completion")
        return u16(payload, 4)
    }

    fun validateRecord(bytes: ByteArray, recording: RemoteRecording, position: Long) {
        if (bytes.size != RECORD_BYTES || u32(bytes, 0) != recording.sessionId) throw PlaudException("Recording data belongs to a different session")
        if (u32(bytes, 4) != position) throw PlaudException("Recording has a gap or unsupported offset layout; partial retained")
        val length = bytes[8].toInt() and 255
        if (length !in 1..80 || position + 80 > recording.size) throw PlaudException("Invalid recording frame length")
        val toc = bytes[9].toInt() and 255
        val config = toc ushr 3
        val frameMicros = when {
            config < 12 -> intArrayOf(10_000, 20_000, 40_000, 60_000)[config and 3]
            config < 16 -> if (config and 1 == 0) 10_000 else 20_000
            else -> 2500 shl (config and 3)
        }
        val frames = when (toc and 3) {
            0 -> 1
            1, 2 -> 2
            else -> if (length > 1) bytes[10].toInt() and 63 else 0
        }
        if (toc and 4 != 0 || frameMicros * frames != 20_000) throw PlaudException("Unsupported Opus audio profile; expected mono 20 ms frames")
    }

    fun crc16(bytes: ByteArray, initial: Int = 0xffff): Int {
        var crc = initial
        bytes.forEach { byte ->
            crc = crc xor ((byte.toInt() and 255) shl 8)
            repeat(8) { crc = ((crc shl 1) xor (if (crc and 0x8000 != 0) 0x1021 else 0)) and 65535 }
        }
        return crc
    }

    internal fun u16(bytes: ByteArray, at: Int): Int = (bytes[at].toInt() and 255) or ((bytes[at + 1].toInt() and 255) shl 8)
    internal fun u32(bytes: ByteArray, at: Int): Long = ByteBuffer.wrap(bytes, at, 4).order(ByteOrder.LITTLE_ENDIAN).int.toLong() and 0xffff_ffffL
}
