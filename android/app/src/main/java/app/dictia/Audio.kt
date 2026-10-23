package app.dictia

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Capture micro en mono PCM 16 bits à `rate` Hz. `onChunk` reçoit chaque bloc
 * de 50 ms au fil de l'eau (mode temps réel), depuis le thread de capture.
 */
class Recorder(private val rate: Int, private val onChunk: ((ByteArray) -> Unit)? = null) {
    private var record: AudioRecord? = null
    private var thread: Thread? = null
    private val pcm = ByteArrayOutputStream()

    @Volatile
    private var running = false

    /** Volume du micro entre 0 et 1, pour l'animation de la bulle. */
    @Volatile
    var level = 0f
        private set

    @SuppressLint("MissingPermission") // vérifiée avant le démarrage de la dictée
    fun start() {
        val min = AudioRecord.getMinBufferSize(
            rate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT,
        )
        val r = AudioRecord(
            MediaRecorder.AudioSource.VOICE_RECOGNITION,
            rate,
            AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT,
            maxOf(min, rate), // 0,5 s de tampon
        )
        if (r.state != AudioRecord.STATE_INITIALIZED) {
            r.release()
            throw IllegalStateException("micro indisponible")
        }
        r.startRecording()
        record = r
        running = true
        thread = Thread({
            val chunk = ByteArray(rate / 10) // 50 ms
            while (running) {
                val n = r.read(chunk, 0, chunk.size)
                if (n > 0) {
                    synchronized(pcm) { pcm.write(chunk, 0, n) }
                    level = Audio.level(chunk, n)
                    onChunk?.invoke(chunk.copyOf(n))
                }
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
    /** Fréquence par défaut (envoi en fin de dictée, Voxtral Realtime). */
    const val RATE = 16_000

    /** Fréquence imposée par l'API Realtime d'OpenAI. */
    const val OPENAI_REALTIME_RATE = 24_000
    private const val THRESHOLD = 500.0 // RMS, environ -36 dBFS
    private const val MARGIN_FRAMES = 15 // 300 ms gardés autour de la parole

    /**
     * Volume RMS d'un bloc PCM ramené entre 0 et 1 sur une échelle en dB :
     * -60 dBFS (silence) → 0, -15 dBFS (voix forte) → 1 (comme sous Windows).
     */
    fun level(pcm: ByteArray, length: Int): Float {
        val samples = ByteBuffer.wrap(pcm, 0, length).order(ByteOrder.LITTLE_ENDIAN).asShortBuffer()
        val count = samples.remaining()
        if (count == 0) return 0f
        var sum = 0.0
        for (i in 0 until count) {
            val v = samples.get(i) / 32768.0
            sum += v * v
        }
        val db = 20 * Math.log10(Math.sqrt(sum / count).coerceAtLeast(1e-9))
        return ((db + 60) / 45).toFloat().coerceIn(0f, 1f)
    }

    /**
     * Supprime les silences de début et de fin (envoi plus court, réponse plus
     * rapide). Renvoie null si aucune parole n'est détectée.
     */
    fun trimSilence(pcm: ByteArray, rate: Int): ByteArray? {
        val frameSize = rate / 50 // 20 ms
        val samples = ByteBuffer.wrap(pcm).order(ByteOrder.LITTLE_ENDIAN).asShortBuffer()
        val frames = samples.remaining() / frameSize
        var first = -1
        var last = -1
        for (f in 0 until frames) {
            var sum = 0.0
            for (i in f * frameSize until (f + 1) * frameSize) {
                val s = samples.get(i).toDouble()
                sum += s * s
            }
            if (Math.sqrt(sum / frameSize) >= THRESHOLD) {
                if (first < 0) first = f
                last = f
            }
        }
        if (first < 0) return null
        val from = maxOf(0, first - MARGIN_FRAMES) * frameSize * 2
        val to = minOf(frames, last + 1 + MARGIN_FRAMES) * frameSize * 2
        return pcm.copyOfRange(from, to)
    }

    /** Fichier WAV en mémoire à partir de PCM 16 bits mono. */
    fun wav(pcm: ByteArray, rate: Int): ByteArray {
        val header = ByteBuffer.allocate(44).order(ByteOrder.LITTLE_ENDIAN).apply {
            put("RIFF".toByteArray()); putInt(36 + pcm.size); put("WAVE".toByteArray())
            put("fmt ".toByteArray()); putInt(16); putShort(1); putShort(1)
            putInt(rate); putInt(rate * 2); putShort(2); putShort(16)
            put("data".toByteArray()); putInt(pcm.size)
        }
        return header.array() + pcm
    }
}
