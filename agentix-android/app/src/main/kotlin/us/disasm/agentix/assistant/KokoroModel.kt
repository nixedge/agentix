package us.disasm.agentix.assistant

import android.content.Context
import android.util.AtomicFile
import org.json.JSONObject
import java.io.File
import java.io.InputStream
import java.security.MessageDigest
import java.util.concurrent.CancellationException
import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock

internal object KokoroModel {
    private val installLock = ReentrantLock()

    fun install(context: Context, current: () -> Boolean, progress: (String) -> Unit): File {
        while (!installLock.tryLock(100, TimeUnit.MILLISECONDS)) checkCurrent(current)
        try {
            checkCurrent(current)
            val manifest = context.assets.open("kokoro/model.json").bufferedReader().use { JSONObject(it.readText()) }
            val filename = manifest.getString("filename")
            val revision = manifest.getString("revision")
            val hash = manifest.getString("sha256")
            val size = manifest.getLong("bytes")
            require(filename == "kokoro_int8_fp16.litertlm" && revision.matches(Regex("[a-f0-9]{40}")) && hash.matches(Regex("[a-f0-9]{64}")) && size in 1..400_000_000)
            val directory = File(context.noBackupFilesDir, "kokoro/$revision")
            check(directory.isDirectory || directory.mkdirs())
            val file = AtomicFile(File(directory, filename))
            progress("Checking local Kokoro model…")
            if (file.baseFile.exists()) {
                val valid = file.openRead().use { digest(it, current) == hash } && file.baseFile.length() == size
                if (valid) return file.baseFile
                file.delete()
            }
            progress("Preparing bundled Kokoro model for first use…")
            val output = file.startWrite()
            try {
                val sha = MessageDigest.getInstance("SHA-256")
                var copied = 0L
                context.assets.open("kokoro/$filename").use { input ->
                    val buffer = ByteArray(64 * 1024)
                    while (true) {
                        checkCurrent(current)
                        val count = input.read(buffer)
                        if (count < 0) break
                        copied += count
                        check(copied <= size)
                        sha.update(buffer, 0, count)
                        output.write(buffer, 0, count)
                    }
                }
                check(copied == size && hex(sha.digest()) == hash)
                checkCurrent(current)
                file.finishWrite(output)
            } catch (failure: Exception) {
                file.failWrite(output)
                throw failure
            }
            return file.baseFile
        } finally { installLock.unlock() }
    }

    private fun digest(input: InputStream, current: () -> Boolean): String {
        val sha = MessageDigest.getInstance("SHA-256")
        val buffer = ByteArray(64 * 1024)
        while (true) {
            checkCurrent(current)
            val count = input.read(buffer)
            if (count < 0) break
            sha.update(buffer, 0, count)
        }
        return hex(sha.digest())
    }
    private fun hex(bytes: ByteArray) = bytes.joinToString("") { "%02x".format(it.toInt() and 255) }
    private fun checkCurrent(current: () -> Boolean) { if (!current()) throw CancellationException() }
}
