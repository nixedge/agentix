package us.disasm.agentix.inbox

import android.content.Context
import android.util.AtomicFile
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONArray
import org.json.JSONObject
import us.disasm.agentix.core.inbox.Archives
import us.disasm.agentix.core.inbox.InboxItem
import java.io.File

object ArchiveStore {
    private const val MAX_BYTES = 4194304
    private val gate = Any()
    private var faulted = false
    private val updates = MutableStateFlow(0L)
    val changes = updates.asStateFlow()
    private fun file(context: Context) = AtomicFile(File(context.filesDir, "inbox-archives-v1.json"))
    fun load(context: Context): Archives = synchronized(gate) {
        check(!faulted) { "Archive storage needs recovery" }
        val source = file(context)
        if (!source.baseFile.exists() && !File(source.baseFile.path + ".bak").exists()) return@synchronized Archives()
        source.openRead().use { input ->
            val bytes = input.readBytesLimited()
            val json = JSONObject(bytes.toString(Charsets.UTF_8))
            require(json.getInt("version") == 1) { "Unknown archive format" }
            val keys = json.getJSONArray("keys")
            require(keys.length() <= 10000)
            val values = (0 until keys.length()).map { index -> keys.get(index).also { require(it is String) } as String }
            require(values.toSet().size == values.size)
            Archives(values.toSet())
        }
    }
    fun change(context: Context, item: InboxItem, archived: Boolean) = synchronized(gate) {
        val current = load(context)
        val next = current.change(item, archived)
        if (next == current) return@synchronized
        val bytes = JSONObject().put("version", 1).put("keys", JSONArray(next.keys.sorted())).toString().toByteArray(Charsets.UTF_8)
        check(bytes.size <= MAX_BYTES) { "Archive registry exceeds limit" }
        val target = file(context)
        val stream = target.startWrite()
        try {
            stream.write(bytes)
            target.finishWrite(stream)
            updates.value += 1
        } catch (error: Exception) { target.failWrite(stream); faulted = true; updates.value += 1; throw error }
    }
    private fun java.io.InputStream.readBytesLimited(): ByteArray {
        val out = java.io.ByteArrayOutputStream()
        val buffer = ByteArray(4096)
        while (true) {
            val count = read(buffer)
            if (count < 0) return out.toByteArray()
            check(out.size() + count <= MAX_BYTES) { "Archive registry exceeds limit" }
            out.write(buffer, 0, count)
        }
    }
}
