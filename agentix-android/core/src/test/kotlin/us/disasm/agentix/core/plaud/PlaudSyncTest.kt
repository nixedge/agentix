package us.disasm.agentix.core.plaud

import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.ArrayDeque
import kotlin.coroutines.Continuation
import kotlin.coroutines.EmptyCoroutineContext
import kotlin.coroutines.startCoroutine
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class PlaudSyncTest {
    @get:Rule val temporary = TemporaryFolder()
    private val address = "AA:BB:CC:DD:EE:FF"
    private val token = "0123456789abcdef0123456789abcdef"
    private val recording = RemoteRecording(1_700_000_000, 160, 1)

    @Test fun commandBytesUseDocumentedWidthsAndExcludeDeletion() {
        val handshake = PlaudProtocol.handshake(token)
        assertEquals(38, handshake.size)
        assertArrayEquals(byteArrayOf(1, 1, 0, 2, 0, 0), handshake.take(6).toByteArray())
        assertEquals(token, handshake.drop(6).toByteArray().toString(Charsets.US_ASCII))
        assertArrayEquals(byteArrayOf(1, 26, 0) + ByteArray(17), PlaudProtocol.list())
        val download = PlaudProtocol.download(recording, 80)
        assertEquals(15, download.size)
        assertEquals(recording.sessionId, u32(download, 3))
        assertEquals(80L, u32(download, 7))
        assertEquals(160L, u32(download, 11))
        assertArrayEquals(byteArrayOf(1, 116, 0, 0, 0x34, 0x12), PlaudProtocol.checksum(0x1234))
        assertArrayEquals(byteArrayOf(1, 5, 0, 0), PlaudBinding.clearBindingPreservingRecordings())
        assertThrows(IllegalArgumentException::class.java) { PlaudProtocol.handshake("not a token") }
        assertThrows(IllegalArgumentException::class.java) { PlaudProtocol.download(recording, 1) }
    }

    @Test fun authenticationRequiresSupportedDeviceEvidence() {
        assertEquals(10, PlaudProtocol.authenticate(byteArrayOf(0, 10, 0)))
        assertThrows(PlaudException::class.java) { PlaudProtocol.authenticate(byteArrayOf(0, 20, 0)) }
        assertTrue(assertThrows(PlaudException::class.java) { PlaudProtocol.authenticate(byteArrayOf(1, 10, 0)) }.message!!.contains("Binding token mismatch"))
        assertTrue(assertThrows(PlaudException::class.java) { PlaudProtocol.authenticate(byteArrayOf(2, 10, 0)) }.message!!.contains("recording"))
        assertThrows(PlaudException::class.java) { PlaudProtocol.authenticate(byteArrayOf(0)) }
    }

    @Test fun listingNeverAcceptsTruncatedOrDuplicateEntries() {
        assertEquals(listOf(recording), PlaudProtocol.recordings(listing(recording)))
        assertEquals(emptyList<RemoteRecording>(), PlaudProtocol.recordings(ByteArray(8)))
        assertThrows(PlaudException::class.java) { PlaudProtocol.recordings(listing(recording).dropLast(1).toByteArray()) }
        assertThrows(PlaudException::class.java) { PlaudProtocol.recordings(listing(recording, recording)) }
        assertThrows(PlaudException::class.java) { PlaudProtocol.recordings(ByteArray(8) { -1 }) }
        val invalid = listing(recording)
        invalid[12] = 1
        assertThrows(PlaudException::class.java) { PlaudProtocol.recordings(invalid) }
    }

    @Test fun invalidPacketTypesAndTransferIdentitiesFail() {
        assertThrows(PlaudException::class.java) { PlaudProtocol.decode(byteArrayOf()) }
        assertThrows(PlaudException::class.java) { PlaudProtocol.decode(byteArrayOf(1)) }
        assertThrows(PlaudException::class.java) { PlaudProtocol.decode(byteArrayOf(3, 0, 0)) }
        assertThrows(PlaudException::class.java) { PlaudProtocol.checkHead(ByteArray(5), recording) }
        assertThrows(PlaudException::class.java) { PlaudProtocol.tailCrc(ByteArray(6), recording) }
        assertEquals(0x29b1, PlaudProtocol.crc16("123456789".toByteArray()))
    }

    @Test fun partialReopenResumesOnlyCompleteRecordsAndValidatesRepeatedFrames() {
        val library = PlaudLibrary(temporary.root)
        library.open(address, recording).use { download ->
            download.accept(record(0).copyOfRange(0, 40))
            assertEquals(0L, download.position)
            download.accept(record(0).copyOfRange(40, 89))
            assertEquals(80L, download.position)
            download.accept(record(0))
            assertEquals(80L, download.position)
        }
        val partial = File(temporary.root, "plaud-partials").listFiles()!!.single()
        partial.appendBytes(ByteArray(17))
        library.open(address, recording).use { download ->
            assertEquals(80L, download.position)
            assertEquals(89L, partial.length())
            download.accept(record(80))
            download.checkComplete(0xffff)
            val output = download.publish()
            assertTrue(output.isFile)
            assertEquals(40L, recording.durationMillis)
        }
    }

    @Test fun gapsWrongSessionConflictsAndUnsupportedAudioCannotPublish() {
        val library = PlaudLibrary(temporary.root)
        library.open(address, recording).use { download ->
            assertThrows(PlaudException::class.java) { download.accept(record(80)) }
            assertThrows(PlaudException::class.java) { download.publish() }
        }
        library.open(address, recording).use { download ->
            val wrong = record(0).also { it[0] = (it[0].toInt() xor 1).toByte() }
            assertThrows(PlaudException::class.java) { download.accept(wrong) }
        }
        library.open(address, recording).use { download ->
            val unsupported = record(0).also { it[9] = 0xfc.toByte() }
            assertThrows(PlaudException::class.java) { download.accept(unsupported) }
        }
        library.open(address, recording).use { download ->
            download.accept(record(0))
            val conflict = record(0).also { it[10] = 0 }
            assertThrows(PlaudException::class.java) { download.accept(conflict) }
            assertFalse(download.destination.exists())
        }
    }

    @Test fun checksumFailureAndMissingTailNeverCreateInboxAudio() {
        val library = PlaudLibrary(temporary.root)
        library.open(address, recording).use { download ->
            download.accept(record(0))
            assertThrows(PlaudException::class.java) { download.checkComplete(0xffff) }
            download.accept(record(80))
            assertThrows(PlaudException::class.java) { download.checkComplete(download.crc xor 1) }
            assertThrows(PlaudException::class.java) { download.publish() }
            assertFalse(download.destination.exists())
            download.checkComplete(download.crc)
            assertTrue(download.publish().isFile)
        }
    }

    @Test fun endToEndSyncIsIdempotentAndUsesOnlyReadDownloadCommands() {
        val library = PlaudLibrary(temporary.root)
        val first = FakeTransport(listOf(
            cmd(1, byteArrayOf(0, 10, 0)), cmd(26, listing(recording)),
            cmd(28, head()), voice(record(0)), voice(record(80)), cmd(29, tail()), cmd(117, byteArrayOf(0)),
        ))
        val result = runSuspend { PlaudSync(first, library).run(address, token) {} }
        assertEquals(PlaudSyncResult(10, 1, 0), result)
        assertEquals(listOf(1, 26, 28, 116), first.sent.map { PlaudProtocol.decode(it).command })
        val second = FakeTransport(listOf(cmd(1, byteArrayOf(0, 10, 0)), cmd(26, listing(recording))))
        assertEquals(PlaudSyncResult(10, 0, 1), runSuspend { PlaudSync(second, library).run(address, token) {} })
        assertEquals(listOf(1, 26), second.sent.map { PlaudProtocol.decode(it).command })
        assertEquals(1, File(temporary.root, "recordings").listFiles()!!.count { it.extension == "ogg" })
    }

    @Test fun disconnectRetryUsesDurableOffsetAndChecksumsAllRecords() {
        val library = PlaudLibrary(temporary.root)
        val interrupted = FakeTransport(listOf(cmd(1, byteArrayOf(0, 10, 0)), cmd(26, listing(recording)), cmd(28, head()), voice(record(0))))
        assertThrows(PlaudException::class.java) { runSuspend { PlaudSync(interrupted, library).run(address, token) {} } }
        val crc = PlaudProtocol.crc16(record(0) + record(80))
        val retry = FakeTransport(listOf(cmd(1, byteArrayOf(0, 10, 0)), cmd(26, listing(recording)), cmd(28, head()), voice(record(80)), cmd(29, tail(crc)), cmd(117, byteArrayOf(0))))
        assertEquals(1, runSuspend { PlaudSync(retry, library).run(address, token) {} }.imported)
        assertEquals(80L, u32(retry.sent[2], 7))
    }

    @Test fun missingChecksumAckCannotPublishAndOtherDevicesDoNotDeduplicate() {
        val library = PlaudLibrary(temporary.root)
        val missingAck = FakeTransport(listOf(cmd(1, byteArrayOf(0, 10, 0)), cmd(26, listing(recording)), cmd(28, head()), voice(record(0)), voice(record(80)), cmd(29, tail())))
        assertThrows(PlaudException::class.java) { runSuspend { PlaudSync(missingAck, library).run(address, token) {} } }
        library.open(address, recording).use { assertFalse(it.imported); assertEquals(160L, it.position) }
        library.open("11:22:33:44:55:66", recording).use { assertFalse(it.imported); assertEquals(0L, it.position) }
    }

    @Test fun publicationRecoveryBuildsStandardOggWithIndependentPageChecksums() {
        val library = PlaudLibrary(temporary.root)
        val output = library.open(address, recording).use { download ->
            download.accept(record(0)); download.accept(record(80)); download.checkComplete(0xffff)
            File(download.destination.parentFile, download.destination.name + ".part").writeText("interrupted container")
            download.publish()
        }
        val bytes = output.readBytes()
        var at = 0
        var sequence = 0
        var finalGranule = 0L
        while (at < bytes.size) {
            assertEquals("OggS", bytes.copyOfRange(at, at + 4).toString(Charsets.US_ASCII))
            assertEquals(sequence++, u32(bytes, at + 18).toInt())
            val segments = bytes[at + 26].toInt() and 255
            val pageSize = 27 + segments + (0 until segments).sumOf { bytes[at + 27 + it].toInt() and 255 }
            val page = bytes.copyOfRange(at, at + pageSize)
            val expected = u32(page, 22)
            (22..25).forEach { page[it] = 0 }
            assertEquals(expected, independentOggCrc(page))
            finalGranule = ByteBuffer.wrap(bytes, at + 6, 8).order(ByteOrder.LITTLE_ENDIAN).long
            if (at + pageSize == bytes.size) assertEquals(4, bytes[at + 5].toInt())
            at += pageSize
        }
        assertEquals(4, sequence)
        assertEquals(1920L, finalGranule)
        assertTrue(bytes.toString(Charsets.ISO_8859_1).contains("OpusHead"))
        // Canonical Nix postCheck independently decodes this standard Opus silence fixture.
        val fixture = File("build/plaud-fixtures/silence.ogg")
        fixture.parentFile.mkdirs()
        output.copyTo(fixture, overwrite = true)
    }

    private fun independentOggCrc(bytes: ByteArray): Long {
        var value = 0L
        for (byte in bytes) {
            for (bit in 7 downTo 0) {
                val feedback = ((value ushr 31) xor ((byte.toInt() ushr bit) and 1).toLong()) and 1
                value = (value shl 1) and 0xffff_ffffL
                if (feedback == 1L) value = value xor 0x04c11db7
            }
        }
        return value
    }

    private fun record(position: Long): ByteArray = buffer(89).putInt(recording.sessionId.toInt()).putInt(position.toInt()).put(3)
        .put(byteArrayOf(0xf8.toByte(), 0xff.toByte(), 0xfe.toByte())).put(ByteArray(77)).array()
    private fun listing(vararg recordings: RemoteRecording): ByteArray = buffer(8 + recordings.size * 10).apply {
        putInt(0); putInt(recordings.size)
        recordings.forEach { putInt(it.sessionId.toInt()); putInt(it.size.toInt()); putShort(it.scene.toShort()) }
    }.array()
    private fun head() = buffer(5).putInt(recording.sessionId.toInt()).put(0).array()
    private fun tail(crc: Int = 0xffff) = buffer(6).putInt(recording.sessionId.toInt()).putShort(crc.toShort()).array()
    private fun cmd(id: Int, payload: ByteArray) = buffer(3 + payload.size).put(1).putShort(id.toShort()).put(payload).array()
    private fun voice(record: ByteArray) = byteArrayOf(2) + record
    private fun buffer(size: Int) = ByteBuffer.allocate(size).order(ByteOrder.LITTLE_ENDIAN)
    private fun u32(bytes: ByteArray, at: Int) = ByteBuffer.wrap(bytes, at, 4).order(ByteOrder.LITTLE_ENDIAN).int.toLong() and 0xffff_ffffL

    private class FakeTransport(packets: List<ByteArray>) : PlaudTransport {
        val sent = mutableListOf<ByteArray>()
        private val incoming = ArrayDeque(packets)
        override suspend fun send(packet: ByteArray) { sent += packet }
        override suspend fun receive(): ByteArray = incoming.pollFirst() ?: throw PlaudException("Disconnected fixture transport")
    }

    private fun <T> runSuspend(block: suspend () -> T): T {
        var outcome: Result<T>? = null
        block.startCoroutine(object : Continuation<T> {
            override val context = EmptyCoroutineContext
            override fun resumeWith(result: Result<T>) { outcome = result }
        })
        return checkNotNull(outcome) { "Fixture unexpectedly suspended" }.getOrThrow()
    }
}
