package us.disasm.agentix.recording

import android.media.MediaPlayer
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import java.io.File
import java.io.IOException
import us.disasm.agentix.R

class PhonePlayback {
    var playingFile by mutableStateOf<String?>(null)
        private set
    var error by mutableStateOf<Int?>(null)
        private set
    private var player: MediaPlayer? = null

    fun play(file: File) {
        stop()
        error = null
        val media = MediaPlayer()
        player = media
        try {
            media.setDataSource(file.absolutePath)
            media.setOnPreparedListener {
                if (player === it) {
                    try {
                        it.start()
                    } catch (_: IllegalStateException) {
                        stop()
                        error = R.string.recording_play_failed
                    }
                }
            }
            media.setOnCompletionListener { stop() }
            media.setOnErrorListener { _, _, _ ->
                stop()
                error = R.string.recording_play_failed
                true
            }
            playingFile = file.name
            media.prepareAsync()
        } catch (_: IOException) {
            stop()
            error = R.string.recording_play_failed
        } catch (_: RuntimeException) {
            stop()
            error = R.string.recording_play_failed
        }
    }

    fun stop() {
        player?.release()
        player = null
        playingFile = null
    }
}
