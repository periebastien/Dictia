package app.dictia

import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.net.HttpURLConnection
import java.net.URL

/** Transcription par `POST {base}/audio/transcriptions` (OpenAI ou Mistral). */
class Transcriber(private val prefs: Prefs, private val vocab: Vocab) {

    /**
     * Ouvre la connexion TLS pendant que l'utilisateur parle : la requête
     * finale réutilise la connexion déjà établie.
     */
    fun warmUp() {
        runCatching {
            val c = URL(prefs.baseUrl + "/models").openConnection() as HttpURLConnection
            c.connectTimeout = 5_000
            c.readTimeout = 5_000
            c.setRequestProperty("Authorization", "Bearer ${prefs.apiKey.trim()}")
            c.responseCode
            (c.errorStream ?: c.inputStream)?.use { it.readBytes() }
        }
    }

    fun transcribe(wav: ByteArray): String {
        val boundary = "dictia${System.nanoTime()}"
        val body = ByteArrayOutputStream()
        fun field(name: String, value: String) {
            body.write(
                "--$boundary\r\nContent-Disposition: form-data; name=\"$name\"\r\n\r\n$value\r\n"
                    .toByteArray(Charsets.UTF_8),
            )
        }
        field("model", prefs.model)
        if (!prefs.isMistral) field("response_format", "json")
        if (prefs.apiLanguage.isNotEmpty()) field("language", prefs.apiLanguage)
        if (prefs.isMistral) {
            // Voxtral : une entrée `context_bias` par terme à reconnaître.
            vocab.cleanTerms.forEach { field("context_bias", it) }
        } else if (vocab.prompt.isNotEmpty()) {
            field("prompt", vocab.prompt)
        }
        body.write(
            ("--$boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n" +
                "Content-Type: audio/wav\r\n\r\n").toByteArray(),
        )
        body.write(wav)
        body.write("\r\n--$boundary--\r\n".toByteArray())
        val bytes = body.toByteArray()

        val c = URL(prefs.baseUrl + "/audio/transcriptions").openConnection() as HttpURLConnection
        c.connectTimeout = 10_000
        c.readTimeout = 30_000
        c.requestMethod = "POST"
        c.doOutput = true
        c.setFixedLengthStreamingMode(bytes.size)
        c.setRequestProperty("Authorization", "Bearer ${prefs.apiKey.trim()}")
        c.setRequestProperty("Content-Type", "multipart/form-data; boundary=$boundary")
        c.outputStream.use { it.write(bytes) }

        val code = c.responseCode
        val text = (if (code in 200..299) c.inputStream else c.errorStream)
            ?.use { it.readBytes().toString(Charsets.UTF_8) }.orEmpty()
        if (code !in 200..299) throw IllegalStateException("API $code : ${errorMessage(text)}")
        return JSONObject(text).optString("text")
    }

    private fun errorMessage(body: String): String = runCatching {
        val json = JSONObject(body)
        val error = json.opt("error")
        when {
            error is JSONObject -> error.optString("message")
            error is String -> error
            json.has("message") -> json.optString("message")
            json.has("detail") -> json.opt("detail").toString()
            else -> body
        }
    }.getOrDefault(body).take(200)
}
