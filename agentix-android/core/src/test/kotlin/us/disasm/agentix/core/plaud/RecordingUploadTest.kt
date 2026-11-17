package us.disasm.agentix.core.plaud

import java.io.File
import java.io.IOException
import java.util.UUID
import kotlin.coroutines.Continuation
import kotlin.coroutines.EmptyCoroutineContext
import kotlin.coroutines.startCoroutine
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class RecordingUploadTest {
    @get:Rule val directory = TemporaryFolder()
    private val endpoint = "https://ai.example.test"
    private val server = UUID.randomUUID()
    private fun audio() = File(directory.root, "1700000000000-plaud-${"a".repeat(32)}-160.ogg").apply { writeText("immutable audio fixture") }
    private fun receipt(metadata: RecordingUploadMetadata) = RecordingServerReceipt(server, metadata, false, false, false)

    @Test fun metadataIsDeterministicAndHashesExactFile() {
        val audio = audio()
        val first = RecordingUploadMetadata.from(audio)
        assertEquals(first, RecordingUploadMetadata.from(audio))
        assertEquals(RecordingUploadMetadata.idFor(audio), first.id)
        assertEquals(40L, first.durationMs)
        assertEquals(1700000000L, first.sourceSession)
        assertEquals(64, first.sha256.length)
        assertEquals("2023-11-14T22:13:20Z", first.recordedAt)
        assertThrows(IOException::class.java) { RecordingUploadMetadata.from(File(directory.root, "wrong.wav")) }
    }

    @Test fun lostAcknowledgementSurvivesRestartAndPinsDestination() {
        val audio = audio()
        val journal = RecordingUploadJournal(directory.root)
        val metadata = RecordingUploadMetadata.from(audio)
        val api = object : RecordingUploadApi {
            override suspend fun get(entry: RecordingUploadEntry): RecordingServerReceipt? = null
            override suspend fun put(entry: RecordingUploadEntry, file: File): RecordingServerReceipt {
                assertEquals(entry, journal.load(metadata.id))
                throw IOException("lost response after remote commit")
            }
        }
        assertThrows(IOException::class.java) { run { RecordingUploadQueue(journal).sync(audio, endpoint, api) } }
        assertNull(journal.load(metadata.id)!!.receipt)
        assertThrows(IOException::class.java) { run { RecordingUploadQueue(RecordingUploadJournal(directory.root)).sync(audio, "https://different.example", api) } }
        val recovered = object : RecordingUploadApi {
            override suspend fun get(entry: RecordingUploadEntry): RecordingServerReceipt? = null
            override suspend fun put(entry: RecordingUploadEntry, file: File): RecordingServerReceipt {
                assertEquals(metadata, entry.metadata)
                return receipt(metadata)
            }
        }
        assertEquals(receipt(metadata), run { RecordingUploadQueue(RecordingUploadJournal(directory.root)).sync(audio, endpoint, recovered) })
        assertEquals(server, journal.load(metadata.id)!!.expectedServerId)
    }

    @Test fun repeatedSyncRefreshesStatusWithoutReuploadAndRejectsWrongIdentity() {
        val audio = audio()
        val journal = RecordingUploadJournal(directory.root)
        val metadata = RecordingUploadMetadata.from(audio)
        var puts = 0
        var gets = 0
        val api = object : RecordingUploadApi {
            override suspend fun get(entry: RecordingUploadEntry): RecordingServerReceipt { gets++; return receipt(metadata).copy(transcribed = true) }
            override suspend fun put(entry: RecordingUploadEntry, file: File): RecordingServerReceipt { puts++; return receipt(metadata) }
        }
        run { RecordingUploadQueue(journal).sync(audio, endpoint, api) }
        assertTrue(run { RecordingUploadQueue(journal).sync(audio, endpoint, api) }.transcribed)
        assertEquals(1, puts); assertEquals(1, gets)
        val wrong = object : RecordingUploadApi {
            override suspend fun get(entry: RecordingUploadEntry) = receipt(metadata).copy(serverId = UUID.randomUUID())
            override suspend fun put(entry: RecordingUploadEntry, file: File) = receipt(metadata).copy(metadata = metadata.copy(sha256 = "0".repeat(64)))
        }
        assertThrows(IOException::class.java) { run { RecordingUploadQueue(journal).sync(audio, endpoint, wrong) } }
        assertEquals(server, journal.load(metadata.id)!!.receipt!!.serverId)
        audio.appendText("changed")
        assertThrows(IOException::class.java) { run { RecordingUploadQueue(journal).sync(audio, endpoint, api) } }
    }

    private fun <T> run(block: suspend () -> T): T {
        var outcome: Result<T>? = null
        block.startCoroutine(object : Continuation<T> {
            override val context = EmptyCoroutineContext
            override fun resumeWith(result: Result<T>) { outcome = result }
        })
        return checkNotNull(outcome).getOrThrow()
    }
}
