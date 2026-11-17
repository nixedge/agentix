package us.disasm.agentix.core.plaud

import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest
import java.time.Instant
import java.util.UUID
import us.disasm.agentix.core.AssistantEndpoint

data class RecordingUploadMetadata(
    val id: UUID, val sourceDevice: String, val sourceSession: Long, val sourceBytes: Long,
    val recordedAt: String, val durationMs: Long, val audioBytes: Long, val sha256: String,
    val mediaType: String = "audio/ogg",
) {
    companion object {
        fun idFor(file: File): UUID = UUID.nameUUIDFromBytes(("plaud-recording-v1:" + file.name).toByteArray(Charsets.US_ASCII))
        fun from(file: File): RecordingUploadMetadata {
            val match = Regex("([0-9]+)-plaud-([0-9a-f]{32})-([0-9]+)\\.ogg").matchEntire(file.name)
                ?: throw IOException("Unsupported recording filename")
            val timestamp = match.groupValues[1].toLong()
            val sourceBytes = match.groupValues[3].toLong()
            val duration = sourceBytes / 80 * 20
            if (timestamp % 1000 != 0L || timestamp / 1000 !in 1..0xffff_ffffL || sourceBytes <= 0 || sourceBytes % 80 != 0L || duration > 14_400_000 || file.length() !in 1..67_108_864L) {
                throw IOException("Server upload supports recordings up to 64 MiB and four hours")
            }
            val digest = MessageDigest.getInstance("SHA-256")
            file.inputStream().use { input ->
                val bytes = ByteArray(65_536)
                while (true) { val count = input.read(bytes); if (count < 0) break; digest.update(bytes, 0, count) }
            }
            return RecordingUploadMetadata(
                idFor(file),
                match.groupValues[2], timestamp / 1000, sourceBytes, Instant.ofEpochMilli(timestamp).toString(),
                duration, file.length(), digest.digest().joinToString("") { "%02x".format(it.toInt() and 255) },
            )
        }
    }
}

data class RecordingServerReceipt(val serverId: UUID, val metadata: RecordingUploadMetadata, val transcribed: Boolean, val searchPending: Boolean, val retrying: Boolean)
data class RecordingUploadEntry(val filename: String, val endpoint: String, val metadata: RecordingUploadMetadata, val receipt: RecordingServerReceipt?, val expectedServerId: UUID? = receipt?.serverId)

interface RecordingUploadApi {
    suspend fun get(entry: RecordingUploadEntry): RecordingServerReceipt?
    suspend fun put(entry: RecordingUploadEntry, file: File): RecordingServerReceipt
}

class RecordingUploadJournal(filesDir: File) {
    private val directory = File(filesDir, "recording-uploads")
    private fun path(id: UUID) = File(directory, "$id.bin")

    fun load(id: UUID): RecordingUploadEntry? {
        val file = path(id)
        if (!file.exists()) return null
        if (file.length() > 16_384) throw IOException("Recording upload journal exceeds limit")
        return DataInputStream(file.inputStream()).use { input ->
            if (input.readInt() != 2) throw IOException("Unsupported recording upload journal")
            val filename = input.readUTF(); val endpoint = input.readUTF()
            val metadata = RecordingUploadMetadata(UUID.fromString(input.readUTF()), input.readUTF(), input.readLong(), input.readLong(), input.readUTF(), input.readLong(), input.readLong(), input.readUTF(), input.readUTF())
            val receipt = if (input.readBoolean()) RecordingServerReceipt(UUID.fromString(input.readUTF()), metadata, input.readBoolean(), input.readBoolean(), input.readBoolean()) else null
            val expected = if (input.readBoolean()) UUID.fromString(input.readUTF()) else null
            if (metadata.id != id || input.read() != -1) throw IOException("Recording upload journal identity mismatch")
            RecordingUploadEntry(filename, endpoint, metadata, receipt, expected)
        }
    }

    fun store(entry: RecordingUploadEntry) {
        if (!directory.isDirectory && !directory.mkdirs()) throw IOException("Cannot create upload journal")
        val temporary = File(directory, "${entry.metadata.id}-${UUID.randomUUID()}.part")
        FileOutputStream(temporary).use { file ->
            val output = DataOutputStream(file)
            output.writeInt(2); output.writeUTF(entry.filename); output.writeUTF(entry.endpoint)
            val m = entry.metadata
            output.writeUTF(m.id.toString()); output.writeUTF(m.sourceDevice); output.writeLong(m.sourceSession); output.writeLong(m.sourceBytes)
            output.writeUTF(m.recordedAt); output.writeLong(m.durationMs); output.writeLong(m.audioBytes); output.writeUTF(m.sha256); output.writeUTF(m.mediaType)
            output.writeBoolean(entry.receipt != null)
            entry.receipt?.let { output.writeUTF(it.serverId.toString()); output.writeBoolean(it.transcribed); output.writeBoolean(it.searchPending); output.writeBoolean(it.retrying) }
            output.writeBoolean(entry.expectedServerId != null)
            entry.expectedServerId?.let { output.writeUTF(it.toString()) }
            output.flush(); file.fd.sync()
        }
        Files.move(temporary.toPath(), path(entry.metadata.id).toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
    }
}

class RecordingUploadQueue(private val journal: RecordingUploadJournal) {
    suspend fun sync(file: File, endpoint: String, api: RecordingUploadApi): RecordingServerReceipt {
        val normalized = AssistantEndpoint.normalize(endpoint) ?: throw IOException("Configure an HTTPS backend before uploading recordings")
        val metadata = RecordingUploadMetadata.from(file)
        val previous = journal.load(metadata.id)
        if (previous != null && previous.metadata != metadata) throw IOException("Local recording differs from its retained upload identity")
        if (previous != null && previous.receipt == null && previous.endpoint != normalized) throw IOException("This recording has an unresolved upload to its original server. Restore that endpoint to retry.")
        val entry = if (previous?.endpoint == normalized) previous else RecordingUploadEntry(file.name, normalized, metadata, null)
        if (entry.receipt != null) {
            val current = api.get(entry)
            if (current != null) {
                validate(entry, current)
                journal.store(entry.copy(receipt = current, expectedServerId = current.serverId))
                return current
            }
        }
        // Persist the exact destination/identity before any upload can reach the server.
        journal.store(entry.copy(receipt = null))
        val receipt = api.put(entry, file)
        validate(entry, receipt)
        journal.store(entry.copy(receipt = receipt, expectedServerId = receipt.serverId))
        return receipt
    }

    private fun validate(entry: RecordingUploadEntry, receipt: RecordingServerReceipt) {
        if (receipt.serverId == UUID(0, 0) || receipt.metadata != entry.metadata ||
            (entry.expectedServerId != null && entry.expectedServerId != receipt.serverId)) throw IOException("Server recording receipt did not match the retained upload identity")
    }
}
