package us.disasm.agentix.core.plaud

interface PlaudTransport {
    suspend fun send(packet: ByteArray)
    suspend fun receive(): ByteArray
}

data class PlaudProgress(val message: String, val imported: Int = 0, val bytes: Long = 0, val totalBytes: Long = 0)

data class PlaudSyncResult(val protocol: Int, val imported: Int, val existing: Int)

class PlaudSync(private val transport: PlaudTransport, private val library: PlaudLibrary) {
    suspend fun run(identity: String, token: String, progress: (PlaudProgress) -> Unit): PlaudSyncResult {
        progress(PlaudProgress("Authenticating Plaud…"))
        transport.send(PlaudProtocol.handshake(token))
        val version = PlaudProtocol.authenticate(response(1))
        progress(PlaudProgress("Connected · protocol $version. Listing recordings…"))
        transport.send(PlaudProtocol.list())
        val recordings = PlaudProtocol.recordings(response(26))
        var imported = 0
        var existing = 0
        recordings.forEachIndexed { index, recording ->
            library.open(identity, recording).use { download ->
                if (download.imported) {
                    existing++
                    return@forEachIndexed
                }
                progress(PlaudProgress("Downloading ${index + 1} of ${recordings.size} · protocol $version", imported, download.position, recording.size))
                transport.send(PlaudProtocol.download(recording, download.position))
                var head = false
                var done = false
                var ignored = 0
                while (!done) {
                    val message = PlaudProtocol.decode(transport.receive())
                    when {
                        message.type == 2 -> {
                            download.accept(message.payload)
                            progress(PlaudProgress("Downloading ${index + 1} of ${recordings.size} · protocol $version", imported, download.position, recording.size))
                        }
                        message.command == 28 && !head -> {
                            PlaudProtocol.checkHead(message.payload, recording)
                            head = true
                        }
                        message.command == 29 && head -> {
                            download.checkComplete(PlaudProtocol.tailCrc(message.payload, recording))
                            transport.send(PlaudProtocol.checksum(download.crc))
                            response(117)
                            progress(PlaudProgress("Saving recording ${index + 1}…", imported, download.position, recording.size))
                            download.publish()
                            imported++
                            done = true
                        }
                        message.command == 9 && ignored++ < 64 -> Unit
                        else -> throw PlaudException("Unexpected response during recording download")
                    }
                }
            }
        }
        progress(PlaudProgress("Sync complete · $imported imported, $existing already in Inbox · protocol $version", imported))
        return PlaudSyncResult(version, imported, existing)
    }

    private suspend fun response(command: Int): ByteArray {
        repeat(64) {
            val message = PlaudProtocol.decode(transport.receive())
            if (message.command == command) return message.payload
            if (message.command != 9) throw PlaudException("Unexpected recorder response while waiting for $command")
        }
        throw PlaudException("Too many unrelated recorder notifications")
    }
}
