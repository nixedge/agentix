package us.disasm.agentix.core.audio

import java.io.File
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.file.Files
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class PcmWaveTest {
    @Test
    fun createsStandardSpeechWaveHeader() {
        val header = PcmWave.header(32_000)
        val fields = ByteBuffer.wrap(header).order(ByteOrder.LITTLE_ENDIAN)
        assertEquals(44, header.size)
        assertEquals("RIFF", header.copyOfRange(0, 4).toString(Charsets.US_ASCII))
        assertEquals(32_036, fields.getInt(4))
        assertEquals("WAVEfmt ", header.copyOfRange(8, 16).toString(Charsets.US_ASCII))
        assertEquals(1, fields.getShort(20).toInt())
        assertEquals(1, fields.getShort(22).toInt())
        assertEquals(16_000, fields.getInt(24))
        assertEquals(32_000, fields.getInt(28))
        assertEquals(16, fields.getShort(34).toInt())
        assertEquals("data", header.copyOfRange(36, 40).toString(Charsets.US_ASCII))
        assertEquals(32_000, fields.getInt(40))
    }

    @Test
    fun rejectsUnalignedOrOutOfRangeDataSizes() {
        for (size in listOf(-2L, 1L, PcmWave.MAX_DATA_BYTES + 2)) {
            assertThrows(IllegalArgumentException::class.java) { PcmWave.header(size) }
        }
        assertEquals(44, PcmWave.header(PcmWave.MAX_DATA_BYTES).size)
    }

    @Test
    fun recoversCompleteFramesAfterInterruptionAndTrimsIncompleteFrame() {
        val directory = Files.createTempDirectory("agentix-wave-test").toFile()
        try {
            val partial = File(directory, "note.wav.part")
            val output = File(directory, "note.wav")
            partial.writeBytes(ByteArray(44) + byteArrayOf(1, 2, 3, 4, 5))
            assertEquals(output, PcmWave.finalizeFile(partial, output))
            assertFalse(partial.exists())
            assertArrayEquals(PcmWave.header(4) + byteArrayOf(1, 2, 3, 4), output.readBytes())
        } finally {
            directory.deleteRecursively()
        }
    }

    @Test
    fun discardsEmptyCaptureWithoutReplacingExistingAudio() {
        val directory = Files.createTempDirectory("agentix-wave-test").toFile()
        try {
            val partial = File(directory, "empty.wav.part")
            val output = File(directory, "existing.wav")
            output.writeBytes(byteArrayOf(9, 8))
            partial.writeBytes(ByteArray(44))
            assertEquals(null, PcmWave.finalizeFile(partial, output))
            assertFalse(partial.exists())
            partial.writeBytes(ByteArray(44) + byteArrayOf(1, 2))
            assertThrows(IOException::class.java) { PcmWave.finalizeFile(partial, output) }
            assertTrue(partial.exists())
            assertArrayEquals(byteArrayOf(9, 8), output.readBytes())
        } finally {
            directory.deleteRecursively()
        }
    }
}
