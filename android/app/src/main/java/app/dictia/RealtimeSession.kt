package app.dictia

import android.util.Base64
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONArray
import org.json.JSONObject
import java.net.URLEncoder
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit

/**
 * Transcription en direct sur WebSocket : l'audio part pendant la dictée et
 * le texte est presque prêt quand on touche la bulle pour terminer.
 *
 * Deux protocoles (comme `src/transcription/realtime.rs` sous Windows) :
 * - OpenAI (`/v1/realtime?intent=transcription`, PCM 24 kHz) : à la fin,
 *   `input_audio_buffer.commit` puis attente de la transcription complète ;
 * - Mistral (`/v1/audio/transcriptions/realtime`, PCM 16 kHz) : attente de
 *   `session.created`, puis à la fin `input_audio.flush` et
 *   `input_audio.end`, le serveur répond `transcription.done`.
 */
class RealtimeSession(private val prefs: Prefs, private val vocab: Vocab) : WebSocketListener() {
    val rate: Int = if (prefs.isMistral) Audio.RATE else Audio.OPENAI_REALTIME_RATE

    private val lock = Object()
    private var socket: WebSocket? = null
    private var ready = false
    private val pending = ArrayList<ByteArray>()
    private var sentBytes = 0L
    private var commitSent = false
    private var committedId: String? = null
    private val deltas = StringBuilder()
    private val result = CompletableFuture<String>()

    fun start() {
        val request = Request.Builder()
            .url(endpoint())
            .header("Authorization", "Bearer ${prefs.apiKey}")
            .build()
        socket = client.newWebSocket(request, this)
    }

    private fun endpoint(): String {
        val ws = prefs.baseUrl.replaceFirst("https://", "wss://")
        return if (prefs.isMistral) {
            "$ws/audio/transcriptions/realtime?model=" + URLEncoder.encode(prefs.realtimeModel, "UTF-8")
        } else {
            "$ws/realtime?intent=transcription"
        }
    }

    /** Audio PCM 16 bits reçu du micro (thread de capture). */
    fun append(pcm: ByteArray) {
        synchronized(lock) {
            if (ready) send(pcm) else pending.add(pcm)
        }
    }

    /**
     * Fin de la dictée : valide l'audio et attend le texte complet.
     * Lève une exception en cas d'échec (l'appelant repasse en mode standard).
     */
    fun finish(timeoutSeconds: Long = 30): String {
        val ws = synchronized(lock) {
            // Mistral peut tarder à ouvrir la session : on lui laisse 5 s.
            val deadline = System.currentTimeMillis() + 5_000
            while (!ready && !result.isDone) {
                val left = deadline - System.currentTimeMillis()
                if (left <= 0) throw IllegalStateException("session temps réel pas encore ouverte")
                lock.wait(left)
            }
            if (result.isDone) return result.get()
            val ws = socket ?: throw IllegalStateException("connexion temps réel absente")
            // OpenAI refuse un tampon de moins de 100 ms ; rien à transcrire.
            if (sentBytes < rate / 10 * 2) {
                ws.close(1000, null)
                return ""
            }
            if (prefs.isMistral) {
                ws.send("""{"type":"input_audio.flush"}""")
                ws.send("""{"type":"input_audio.end"}""")
            } else {
                ws.send("""{"type":"input_audio_buffer.commit"}""")
            }
            commitSent = true
            ws
        }
        try {
            return result.get(timeoutSeconds, TimeUnit.SECONDS)
        } finally {
            ws.close(1000, null)
        }
    }

    fun cancel() {
        socket?.cancel()
    }

    // --- WebSocket ----------------------------------------------------------

    override fun onOpen(webSocket: WebSocket, response: Response) {
        // Mistral ouvre la session de lui-même (`session.created`).
        if (!prefs.isMistral) {
            webSocket.send(openAiSessionUpdate().toString())
            markReady()
        }
    }

    override fun onMessage(webSocket: WebSocket, text: String) {
        val event = runCatching { JSONObject(text) }.getOrNull() ?: return
        val type = event.optString("type")
        synchronized(lock) {
            when {
                type == "error" -> fail("erreur de l'API : ${errorMessage(event.opt("error"))}")
                prefs.isMistral -> when (type) {
                    "session.created" -> {
                        webSocket.send(mistralSessionUpdate().toString())
                        markReady()
                    }
                    "transcription.text.delta" -> deltas.append(event.optString("text"))
                    "transcription.done" -> result.complete(event.optString("text", deltas.toString()))
                }
                else -> when (type) {
                    "conversation.item.input_audio_transcription.delta" ->
                        deltas.append(event.optString("delta"))
                    "input_audio_buffer.committed" -> committedId = event.optString("item_id")
                    "conversation.item.input_audio_transcription.completed" -> {
                        val ours = committedId?.let { it == event.optString("item_id") } ?: commitSent
                        if (ours) result.complete(event.optString("transcript"))
                    }
                    "conversation.item.input_audio_transcription.failed" ->
                        fail("transcription échouée : ${errorMessage(event.opt("error"))}")
                }
            }
        }
    }

    override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
        val detail = response?.let { r ->
            val body = runCatching { r.body?.string() }.getOrNull().orEmpty()
            " (HTTP ${r.code}${body.take(200).let { if (it.isBlank()) "" else " : $it" }})"
        }.orEmpty()
        synchronized(lock) { fail("connexion temps réel impossible : ${t.message}$detail") }
    }

    override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
        synchronized(lock) { fail("connexion temps réel fermée ($code${if (reason.isBlank()) "" else " : $reason"})") }
        webSocket.close(1000, null)
    }

    // --- Messages -----------------------------------------------------------

    private fun markReady() {
        synchronized(lock) {
            ready = true
            pending.forEach { send(it) }
            pending.clear()
            lock.notifyAll()
        }
    }

    private fun fail(message: String) {
        if (result.completeExceptionally(IllegalStateException(message))) lock.notifyAll()
    }

    private fun send(pcm: ByteArray) {
        val type = if (prefs.isMistral) "input_audio.append" else "input_audio_buffer.append"
        val audio = Base64.encodeToString(pcm, Base64.NO_WRAP)
        socket?.send("""{"type":"$type","audio":"$audio"}""")
        sentBytes += pcm.size
    }

    /** Session de transcription seule, PCM 24 kHz, sans détection de tour. */
    private fun openAiSessionUpdate(): JSONObject {
        val model = prefs.realtimeModel
        val transcription = JSONObject().put("model", model)
        if (prefs.apiLanguage.isNotEmpty()) transcription.put("language", prefs.apiLanguage)
        // Chaque famille de modèles a sa façon d'être guidée.
        when (model) {
            "gpt-live-transcribe", "gpt-transcribe" ->
                if (vocab.cleanTerms.isNotEmpty()) transcription.put("keywords", JSONArray(vocab.cleanTerms))
            "gpt-realtime-whisper" -> transcription.put("delay", "minimal")
            else -> if (vocab.prompt.isNotEmpty()) transcription.put("prompt", vocab.prompt)
        }
        val input = JSONObject()
            .put("format", JSONObject().put("type", "audio/pcm").put("rate", Audio.OPENAI_REALTIME_RATE))
            .put("transcription", transcription)
            .put("turn_detection", JSONObject.NULL)
            .put("noise_reduction", JSONObject().put("type", "near_field"))
        return JSONObject()
            .put("type", "session.update")
            .put(
                "session",
                JSONObject().put("type", "transcription")
                    .put("audio", JSONObject().put("input", input)),
            )
    }

    /** Session Voxtral Realtime : PCM 16 kHz, langue et termes du vocabulaire. */
    private fun mistralSessionUpdate(): JSONObject {
        val session = JSONObject().put(
            "audio_format",
            JSONObject().put("encoding", "pcm_s16le").put("sample_rate", Audio.RATE),
        )
        if (prefs.apiLanguage.isNotEmpty()) session.put("language", prefs.apiLanguage)
        if (vocab.cleanTerms.isNotEmpty()) session.put("context_bias", JSONArray(vocab.cleanTerms))
        return JSONObject().put("type", "session.update").put("session", session)
    }

    /** `{"message": "…"}` ou, chez Mistral, `{"message": {"detail": "…"}}`. */
    private fun errorMessage(error: Any?): String = when (error) {
        is JSONObject -> when (val m = error.opt("message")) {
            is String -> m
            is JSONObject -> m.optString("detail", m.toString())
            else -> error.toString()
        }
        null -> "inconnue"
        else -> error.toString()
    }.take(200)

    private companion object {
        val client: OkHttpClient = OkHttpClient.Builder()
            .connectTimeout(10, TimeUnit.SECONDS)
            .pingInterval(15, TimeUnit.SECONDS)
            .build()
    }
}
