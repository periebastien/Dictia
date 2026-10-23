package app.dictia

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** Capture micro en mono 16 kHz, PCM 16 bits. */
class Recorder {
    private var record: AudioRecord? = null
    private var thread: Thread? = null
    private val pcm = ByteArrayOutputStream()

    @Volatile
    private var running = false

    @SuppressLint("MissingPermission") // vérifiée avant le démarrage de la dictée
    fun start() {
        val min = AudioRecord.getMinBufferSize(
            Audio.RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT,
        )
        val r = AudioRecord(
            MediaRecorder.AudioSource.VOICE_RECOGNITION,
            Audio.RATE,
            AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT,
            maxOf(min, Audio.RATE), // 0,5 s de tampon
        )
        if (r.state != AudioRecord.STATE_INITIALIZED) {
            r.release()
            throw IllegalStateException("micro indisponible")
        }
        r.startRecording()
        record = r
        running = true
        thread = Thread({
            val chunk = ByteArray(3200) // 100 ms
            while (running) {
                val n = r.read(chunk, 0, chunk.size)
                if (n > 0) synchronized(pcm) { pcm.write(chunk, 0, n) }
            }
        }, "dictia-micro").apply { start() }
    }

    /** Arrête la capture et renvoie l'audio enregistré. */
    fun stop(): ByteArray {
        running = false
        thread?.join(500)
        record?.run {
            runCatching { stop() }
            release()
        }
        record = null
        return synchronized(pcm) { pcm.toByteArray() }
    }
}

object Audio {
    const val RATE = 16_000
    private const val FRAME = RATE / 50 // 20 ms
    private const val THRESHOLD = 500.0 // RMS, environ -36 dBFS
    private const val MARGIN_FRAMES = 15 // 300 ms gardés autour de la parole

    /**
     * Supprime les silences de début et de fin (envoi plus court, réponse plus
     * rapide). Renvoie null si aucune parole n'est détectée.
     */
    fun trimSilence(pcm: ByteArray): ByteArray? {
        val samples = ByteBuffer.wrap(pcm).order(ByteOrder.LITTLE_ENDIAN).asShortBuffer()
        val frames = samples.remaining() / FRAME
        var first = -1
        var last = -1
        for (f in 0 until frames) {
            var sum = 0.0
            for (i in f * FRAME until (f + 1) * FRAME) {
                val s = samples.get(i).toDouble()
                sum += s * s
            }
            if (Math.sqrt(sum / FRAME) >= THRESHOLD) {
                if (first < 0) first = f
                last = f
            }
        }
        if (first < 0) return null
        val from = maxOf(0, first - MARGIN_FRAMES) * FRAME * 2
        val to = minOf(frames, last + 1 + MARGIN_FRAMES) * FRAME * 2
        return pcm.copyOfRange(from, to)
    }

    /** Fichier WAV en mémoire à partir de PCM 16 bits mono 16 kHz. */
    fun wav(pcm: ByteArray): ByteArray {
        val header = ByteBuffer.allocate(44).order(ByteOrder.LITTLE_ENDIAN).apply {
            put("RIFF".toByteArray()); putInt(36 + pcm.size); put("WAVE".toByteArray())
            put("fmt ".toByteArray()); putInt(16); putShort(1); putShort(1)
            putInt(RATE); putInt(RATE * 2); putShort(2); putShort(16)
            put("data".toByteArray()); putInt(pcm.size)
        }
        return header.array() + pcm
    }
}
