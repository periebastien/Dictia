package app.dictia

import android.content.Context

/** Réglages de Dictia, enregistrés dans les préférences privées de l'application. */
data class Prefs(
    val provider: String,
    val openAiKey: String,
    val openAiModel: String,
    val mistralKey: String,
    val mistralModel: String,
    val language: String,
    val shortTextWords: Int,
) {
    val apiKey: String get() = if (provider == MISTRAL) mistralKey else openAiKey
    val model: String get() = if (provider == MISTRAL) mistralModel else openAiModel
    val baseUrl: String
        get() = if (provider == MISTRAL) "https://api.mistral.ai/v1" else "https://api.openai.com/v1"

    fun save(context: Context) {
        store(context).edit()
            .putString("provider", provider)
            .putString("openai_key", openAiKey.trim())
            .putString("openai_model", openAiModel.trim())
            .putString("mistral_key", mistralKey.trim())
            .putString("mistral_model", mistralModel.trim())
            .putString("language", language.trim())
            .putInt("short_text_words", shortTextWords)
            .apply()
    }

    companion object {
        const val OPENAI = "openai"
        const val MISTRAL = "mistral"
        const val OPENAI_MODEL = "gpt-4o-transcribe"
        const val MISTRAL_MODEL = "voxtral-mini-latest"

        private fun store(context: Context) =
            context.getSharedPreferences("dictia", Context.MODE_PRIVATE)

        fun load(context: Context): Prefs {
            val p = store(context)
            return Prefs(
                provider = p.getString("provider", OPENAI) ?: OPENAI,
                openAiKey = p.getString("openai_key", "") ?: "",
                openAiModel = p.getString("openai_model", "").orEmpty().ifBlank { OPENAI_MODEL },
                mistralKey = p.getString("mistral_key", "") ?: "",
                mistralModel = p.getString("mistral_model", "").orEmpty().ifBlank { MISTRAL_MODEL },
                language = p.getString("language", "fr") ?: "fr",
                shortTextWords = p.getInt("short_text_words", 3),
            )
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
