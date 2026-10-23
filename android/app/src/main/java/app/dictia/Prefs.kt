package app.dictia

import android.content.Context
import org.json.JSONObject

/** Réglages de Dictia, enregistrés dans les préférences privées de l'application. */
data class Prefs(
    val provider: String,
    /** "standard" : envoi à la fin ; "realtime" : envoi pendant la dictée. */
    val engine: String,
    val openAiKey: String,
    val openAiModel: String,
    val openAiRealtimeModel: String,
    val mistralKey: String,
    val mistralModel: String,
    val mistralRealtimeModel: String,
    val language: String,
    val shortTextWords: Int,
) {
    val isMistral: Boolean get() = provider == MISTRAL
    val realtime: Boolean get() = engine == REALTIME
    val apiKey: String get() = (if (isMistral) mistralKey else openAiKey).trim()
    val model: String get() = if (isMistral) mistralModel else openAiModel
    val realtimeModel: String get() = if (isMistral) mistralRealtimeModel else openAiRealtimeModel
    val baseUrl: String
        get() = if (isMistral) "https://api.mistral.ai/v1" else "https://api.openai.com/v1"

    /** Langue envoyée à l'API (vide : reconnue par le modèle). */
    val apiLanguage: String get() = language.trim().takeUnless { it == "auto" }.orEmpty()

    fun save(context: Context) {
        store(context).edit()
            .putString("provider", provider)
            .putString("engine", engine)
            .putString("openai_key", openAiKey.trim())
            .putString("openai_model", openAiModel.trim())
            .putString("openai_realtime_model", openAiRealtimeModel.trim())
            .putString("mistral_key", mistralKey.trim())
            .putString("mistral_model", mistralModel.trim())
            .putString("mistral_realtime_model", mistralRealtimeModel.trim())
            .putString("language", language.trim())
            .putInt("short_text_words", shortTextWords)
            .apply()
    }

    companion object {
        const val OPENAI = "openai"
        const val MISTRAL = "mistral"
        const val STANDARD = "standard"
        const val REALTIME = "realtime"

        /** Modèles proposés (comme sous Windows) ; « Autre… » permet d'en saisir un. */
        fun models(provider: String, engine: String): List<String> = when {
            provider == MISTRAL && engine == REALTIME ->
                listOf("voxtral-mini-transcribe-realtime-3")
            provider == MISTRAL -> listOf("voxtral-mini-latest", "voxtral-mini-transcribe-3")
            engine == REALTIME ->
                listOf("gpt-live-transcribe", "gpt-realtime-whisper", "gpt-4o-transcribe", "gpt-4o-mini-transcribe")
            else -> listOf("gpt-4o-transcribe", "gpt-4o-mini-transcribe", "gpt-transcribe", "whisper-1")
        }

        /** Langues proposées (`auto` : reconnue par le modèle). */
        val LANGUAGES = listOf(
            "auto" to "Automatique",
            "fr" to "Français",
            "en" to "Anglais",
            "es" to "Espagnol",
            "de" to "Allemand",
            "it" to "Italien",
            "pt" to "Portugais",
            "nl" to "Néerlandais",
            "ar" to "Arabe",
        )

        val SHORT_TEXT_WORDS = listOf(0, 1, 2, 3, 4, 5, 6)

        private fun store(context: Context) =
            context.getSharedPreferences("dictia", Context.MODE_PRIVATE)

        fun load(context: Context): Prefs {
            val p = store(context)
            fun text(key: String, default: String) = p.getString(key, "").orEmpty().ifBlank { default }
            return Prefs(
                provider = text("provider", OPENAI),
                engine = text("engine", STANDARD),
                openAiKey = p.getString("openai_key", "").orEmpty(),
                openAiModel = text("openai_model", models(OPENAI, STANDARD)[0]),
                openAiRealtimeModel = text("openai_realtime_model", models(OPENAI, REALTIME)[0]),
                mistralKey = p.getString("mistral_key", "").orEmpty(),
                mistralModel = text("mistral_model", models(MISTRAL, STANDARD)[0]),
                mistralRealtimeModel = text("mistral_realtime_model", models(MISTRAL, REALTIME)[0]),
                language = text("language", "fr"),
                shortTextWords = p.getInt("short_text_words", 3),
            )
        }

        fun loadVocab(context: Context): Vocab =
            store(context).getString("vocab", null)
                ?.let { runCatching { Vocab.fromJson(JSONObject(it)) }.getOrNull() }
                ?: Vocab()

        fun saveVocab(context: Context, vocab: Vocab) {
            store(context).edit().putString("vocab", vocab.toJson().toString()).apply()
        }

        /** Dernier problème rencontré, affiché dans l'écran de Dictia. */
        fun problem(context: Context): String? = store(context).getString("problem", null)

        fun setProblem(context: Context, message: String?) {
            store(context).edit().putString("problem", message).apply()
        }

        /** Position de la bulle à l'écran. */
        fun bubblePosition(context: Context): Pair<Int, Int> {
            val p = store(context)
            return p.getInt("bubble_x", 0) to p.getInt("bubble_y", 400)
        }

        fun saveBubblePosition(context: Context, x: Int, y: Int) {
            store(context).edit().putInt("bubble_x", x).putInt("bubble_y", y).apply()
        }
    }
}
