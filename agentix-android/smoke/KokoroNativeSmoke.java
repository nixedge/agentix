package us.disasm.agentix.smoke;

import com.google.mediapipe.tasks.audio.texttospeech.AudioResult;
import com.google.mediapipe.tasks.audio.texttospeech.MediaPipeTtsJni;
import com.google.mediapipe.tasks.audio.texttospeech.VoiceInfo;
import java.io.File;
import java.io.FileOutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.HashSet;
import java.util.Set;

/** Fixed-sentence model/JNI test for adb app_process; never opens audio input/output. */
public final class KokoroNativeSmoke {
    public static void main(String[] args) throws Exception {
        File directory = new File(args[0]);
        File cache = new File(directory, "cache");
        if (!cache.isDirectory() && !cache.mkdirs()) throw new IllegalStateException("Cache unavailable");
        long started = System.nanoTime();
        MediaPipeTtsJni jni = MediaPipeTtsJni.INSTANCE;
        long handle = jni.nativeCreate(new File(directory, "kokoro_int8_fp16.litertlm").getAbsolutePath(),
                0, 0.95f, 128, "bm_george", "en-GB", cache.getAbsolutePath());
        if (handle == 0) throw new IllegalStateException("Model initialization failed");
        try {
            Set<String> voices = new HashSet<>();
            for (VoiceInfo voice : jni.nativeListVoices(handle)) voices.add(voice.getVoiceId());
            if (!voices.contains("bm_george") || !voices.contains("bm_daniel")) throw new IllegalStateException("Requested voice missing");
            System.out.println("Model ready in " + (System.nanoTime() - started) / 1_000_000 + " ms; voices=" + voices.size());
            for (String voice : new String[] {"bm_george", "bm_daniel"}) {
                long synthesisStarted = System.nanoTime();
                AudioResult audio = jni.nativeSynthesize(handle,
                        "At your service. Your next reminder is at four thirty this afternoon.", voice, "en-GB");
                float[] samples = audio.getSamples();
                if (audio.getSampleRateHz() != 24000 || samples.length < 2400 || samples.length > 24000 * 60) throw new IllegalStateException("Invalid speech output");
                double energy = 0;
                for (float sample : samples) {
                    if (!Float.isFinite(sample)) throw new IllegalStateException("Non-finite speech output");
                    energy += sample * sample;
                }
                if (energy < 0.001) throw new IllegalStateException("Silent speech output");
                writeWav(new File(directory, voice + ".wav"), samples);
                System.out.println(voice + ": " + samples.length + " samples, 24000 Hz, "
                        + (System.nanoTime() - synthesisStarted) / 1_000_000 + " ms synthesis");
            }
        } finally { jni.nativeDestroy(handle); }
    }

    private static void writeWav(File output, float[] samples) throws Exception {
        ByteBuffer buffer = ByteBuffer.allocate(44 + samples.length * 2).order(ByteOrder.LITTLE_ENDIAN);
        buffer.put(new byte[] {'R','I','F','F'}).putInt(36 + samples.length * 2);
        buffer.put(new byte[] {'W','A','V','E','f','m','t',' '}).putInt(16).putShort((short) 1).putShort((short) 1);
        buffer.putInt(24000).putInt(48000).putShort((short) 2).putShort((short) 16);
        buffer.put(new byte[] {'d','a','t','a'}).putInt(samples.length * 2);
        for (float sample : samples) buffer.putShort((short) (Math.max(-1f, Math.min(1f, sample)) * 32767));
        try (FileOutputStream stream = new FileOutputStream(output)) { stream.write(buffer.array()); }
    }
}
