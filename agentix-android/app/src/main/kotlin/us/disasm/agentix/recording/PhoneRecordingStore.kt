package us.disasm.agentix.recording

import java.io.File
import java.io.IOException
import java.util.UUID
import us.disasm.agentix.core.audio.PcmWave

data class PhoneRecording(val file: File, val startedAtMillis: Long, val recovered: Boolean) {
    val durationMillis: Long
        get() = (file.length() - PcmWave.HEADER_BYTES).coerceAtLeast(0) * 1000 / PcmWave.BYTES_PER_SECOND
}

data class RecordingLibrary(val recordings: List<PhoneRecording>, val hasReadErrors: Boolean)

class PhoneRecordingStore(filesDir: File) {
    private val directory = File(filesDir, "recordings")

    fun newPartial(): File = File(directory, "${System.currentTimeMillis()}-${UUID.randomUUID()}.wav.part")

    fun prepareDirectory() {
        if (!directory.isDirectory && !directory.mkdirs()) throw IOException("Cannot create recording directory")
    }

    fun finish(partial: File): File? =
        PcmWave.finalizeFile(partial, File(directory, partial.name.removeSuffix(".part")))

    fun recoverAndList(activeFileName: () -> String?): RecordingLibrary {
        if (!directory.exists()) return RecordingLibrary(emptyList(), false)
        var hasErrors = false
        val files = directory.listFiles() ?: return RecordingLibrary(emptyList(), true)
        files.filter { it.name.endsWith(".wav.part") }.forEach { partial ->
            if (partial.name != activeFileName()) {
                try {
                    val recovered = File(directory, partial.name.removeSuffix(".wav.part") + ".recovered.wav")
                    PcmWave.finalizeFile(partial, recovered)
                } catch (_: IOException) {
                    hasErrors = true
                }
            }
        }
        val completed = directory.listFiles() ?: return RecordingLibrary(emptyList(), true)
        val recordings = completed.filter { it.isFile && it.name.endsWith(".wav") }.mapNotNull { file ->
            val timestamp = file.name.substringBefore('-').toLongOrNull()
            if (timestamp == null || file.length() < PcmWave.HEADER_BYTES) {
                hasErrors = true
                null
            } else {
                PhoneRecording(file, timestamp, file.name.endsWith(".recovered.wav"))
            }
        }.sortedByDescending { it.startedAtMillis }
        return RecordingLibrary(recordings, hasErrors)
    }
}
