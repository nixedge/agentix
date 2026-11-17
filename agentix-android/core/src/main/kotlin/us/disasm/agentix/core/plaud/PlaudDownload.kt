package us.disasm.agentix.core.plaud

import java.io.Closeable
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.RandomAccessFile
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest

class PlaudLibrary(private val filesDir: File) {
    fun open(address: String, recording: RemoteRecording): PlaudDownload {
        require(Regex("[0-9A-Fa-f]{2}(:[0-9A-Fa-f]{2}){5}").matches(address))
        val key = MessageDigest.getInstance("SHA-256").digest(address.uppercase(java.util.Locale.ROOT).toByteArray(Charsets.US_ASCII))
            .take(16).joinToString("") { "%02x".format(it.toInt() and 255) }
        val partials = File(filesDir, "plaud-partials")
        val recordings = File(filesDir, "recordings")
        listOf(partials, recordings).forEach {
            if (!it.isDirectory && !it.mkdirs()) throw IOException("Cannot create Plaud recording storage")
        }
        return PlaudDownload(
            recording,
            File(partials, "$key-${recording.sessionId}-${recording.size}.part"),
            File(recordings, "${recording.sessionId * 1000}-plaud-$key-${recording.size}.ogg"),
        )
    }
}

class PlaudDownload internal constructor(
    val recording: RemoteRecording,
    private val partial: File,
    val destination: File,
) : Closeable {
    private val file = RandomAccessFile(partial, "rw")
    var position: Long = 0
        private set
    var crc: Int = 0xffff
        private set
    val imported: Boolean get() = destination.isFile
    private var buffered = byteArrayOf()
    private var verified = false

    init {
        try {
            // A disposed screen's IO worker may still be closing when a new screen opens.
            try {
                if (file.channel.tryLock() == null) throw PlaudException("Another sync is still stopping; retry shortly")
            } catch (_: java.nio.channels.OverlappingFileLockException) {
                throw PlaudException("Another sync is still stopping; retry shortly")
            }
            val completeBytes = file.length() - file.length() % PlaudProtocol.RECORD_BYTES
            file.setLength(completeBytes)
            if (completeBytes / PlaudProtocol.RECORD_BYTES * 80 > recording.size) throw PlaudException("Partial download exceeds recording size")
            val record = ByteArray(PlaudProtocol.RECORD_BYTES)
            while (file.filePointer < completeBytes) {
                file.readFully(record)
                PlaudProtocol.validateRecord(record, recording, position)
                position += 80
                crc = PlaudProtocol.crc16(record, crc)
            }
        } catch (failure: Exception) {
            file.close()
            throw failure
        }
    }

    fun accept(payload: ByteArray) {
        verified = false
        if (payload.isEmpty() || payload.size > 512) throw PlaudException("Invalid audio notification size")
        buffered += payload
        var consumed = 0
        while (buffered.size - consumed >= PlaudProtocol.RECORD_BYTES) {
            val record = buffered.copyOfRange(consumed, consumed + PlaudProtocol.RECORD_BYTES)
            val offset = PlaudProtocol.u32(record, 4)
            if (offset < position) {
                PlaudProtocol.validateRecord(record, recording, offset)
                if (offset % 80 != 0L) throw PlaudException("Unaligned repeated audio offset")
                val previous = ByteArray(PlaudProtocol.RECORD_BYTES)
                file.seek(offset / 80 * PlaudProtocol.RECORD_BYTES)
                file.readFully(previous)
                if (!previous.contentEquals(record)) throw PlaudException("Conflicting repeated recording frame")
            } else {
                PlaudProtocol.validateRecord(record, recording, position)
                file.seek(position / 80 * PlaudProtocol.RECORD_BYTES)
                file.write(record)
                position += 80
                crc = PlaudProtocol.crc16(record, crc)
            }
            consumed += PlaudProtocol.RECORD_BYTES
        }
        buffered = buffered.copyOfRange(consumed, buffered.size)
    }

    fun checkComplete(deviceCrc: Int) {
        if (position != recording.size || buffered.isNotEmpty()) throw PlaudException("Recording transfer ended before all audio arrived; retry to resume")
        if (deviceCrc != 0xffff && deviceCrc != crc) throw PlaudException("Recording checksum mismatch; no audio imported")
        file.fd.sync()
        verified = true
    }

    fun publish(): File {
        if (!verified || position != recording.size || buffered.isNotEmpty()) throw PlaudException("Cannot import unverified or incomplete recording")
        if (destination.isFile) return destination
        val temporary = File(destination.parentFile, destination.name + ".part")
        FileOutputStream(temporary).use { output ->
            val ogg = OggOpus(output, recording.sessionId.toInt())
            ogg.headers()
            file.seek(0)
            val record = ByteArray(PlaudProtocol.RECORD_BYTES)
            var number = 0L
            while (number * 80 < recording.size) {
                file.readFully(record)
                PlaudProtocol.validateRecord(record, recording, number * 80)
                val length = record[8].toInt() and 255
                number++
                ogg.frame(record.copyOfRange(9, 9 + length), number, number * 80 == recording.size)
            }
            output.fd.sync()
        }
        Files.move(temporary.toPath(), destination.toPath(), StandardCopyOption.ATOMIC_MOVE)
        return destination
    }

    override fun close() {
        try { file.fd.sync() } finally { file.close() }
    }
}
