package app.dictia

import org.json.JSONArray
import org.json.JSONObject

/** Règle de remplacement appliquée au texte transcrit. */
data class Replacement(
    val from: String,
    val to: String,
    val wholeWord: Boolean = true,
    val caseSensitive: Boolean = false,
)

/**
 * Vocabulaire, au même format que `vocab.toml` de la version Windows :
 * contexte libre et termes envoyés au modèle, puis remplacements appliqués au
 * texte. Le fichier s'échange tel quel entre le PC et le téléphone.
 */
data class Vocab(
    val context: String = DEFAULT_CONTEXT,
    val terms: List<String> = listOf("Dictia"),
    val replacements: List<Replacement> = emptyList(),
) {
    val cleanTerms: List<String> get() = terms.map { it.trim() }.filter { it.isNotEmpty() }

    /** Contexte suivi de la liste des termes (prompt des modèles OpenAI). */
    val prompt: String
        get() = listOfNotNull(
            context.trim().ifEmpty { null },
            cleanTerms.takeIf { it.isNotEmpty() }?.let { "Vocabulaire : ${it.joinToString(", ")}." },
        ).joinToString(" ")

    /** Applique les remplacements, dans l'ordre. */
    fun apply(text: String): String = replacements.fold(text) { out, rule ->
        val from = rule.from.trim()
        if (from.isEmpty()) return@fold out
        val pattern = buildString {
            if (!rule.caseSensitive) append("(?iu)")
            // Limite de mot seulement à côté d'un caractère de mot : « ; » ou
            // « ... » en bordure restent remplaçables.
            if (rule.wholeWord && isWordChar(from.first())) append("(?<![\\p{L}\\p{N}_])")
            append(Regex.escape(from))
            if (rule.wholeWord && isWordChar(from.last())) append("(?![\\p{L}\\p{N}_])")
        }
        Regex(pattern).replace(out, Regex.escapeReplacement(rule.to))
    }

    /**
     * Détecte une « transcription » qui recopie le prompt (audio presque
     * muet) : au moins trois mots entièrement contenus dans le prompt.
     */
    fun isPromptEcho(text: String): Boolean {
        val words = normalizeWords(text)
        return words.split(' ').count { it.isNotEmpty() } >= 3 && normalizeWords(prompt).contains(words)
    }

    fun toJson(): JSONObject = JSONObject()
        .put("context", context)
        .put("terms", JSONArray(terms))
        .put(
            "replacements",
            JSONArray(
                replacements.map {
                    JSONObject().put("from", it.from).put("to", it.to)
                        .put("whole_word", it.wholeWord).put("case_sensitive", it.caseSensitive)
                },
            ),
        )

    /** Fichier `vocab.toml` lisible par la version Windows. */
    fun toToml(): String = buildString {
        append("# Vocabulaire de Dictia (exporté depuis Android)\n")
        append("# Compatible avec %APPDATA%\\Dictia\\vocab.toml sur Windows.\n\n")
        append("# Contexte libre transmis au modèle (prompt).\n")
        append("context = ").append(tomlString(context)).append("\n\n")
        append("# Noms propres, sigles et termes techniques à bien orthographier.\n")
        append("terms = [")
        if (terms.isNotEmpty()) {
            append("\n")
            terms.forEach { append("    ").append(tomlString(it)).append(",\n") }
        }
        append("]\n")
        if (replacements.isNotEmpty()) {
            append("\n# Remplacements appliqués au texte transcrit, dans l'ordre.\n")
        }
        replacements.forEach {
            append("\n[[replacements]]\n")
            append("from = ").append(tomlString(it.from)).append("\n")
            append("to = ").append(tomlString(it.to)).append("\n")
            if (!it.wholeWord) append("whole_word = false\n")
            if (it.caseSensitive) append("case_sensitive = true\n")
        }
    }

    companion object {
        const val DEFAULT_CONTEXT = "Dictée en français, avec une ponctuation soignée."

        private fun isWordChar(c: Char) = c.isLetterOrDigit() || c == '_'

        private fun normalizeWords(text: String) = text.lowercase()
            .split(Regex("[^\\p{L}\\p{N}]+")).filter { it.isNotEmpty() }.joinToString(" ")

        fun fromJson(json: JSONObject): Vocab {
            val terms = json.optJSONArray("terms") ?: JSONArray()
            val reps = json.optJSONArray("replacements") ?: JSONArray()
            return Vocab(
                context = json.optString("context", DEFAULT_CONTEXT),
                terms = (0 until terms.length()).map { terms.getString(it) },
                replacements = (0 until reps.length()).map {
                    val r = reps.getJSONObject(it)
                    Replacement(
                        from = r.optString("from"),
                        to = r.optString("to"),
                        wholeWord = r.optBoolean("whole_word", true),
                        caseSensitive = r.optBoolean("case_sensitive", false),
                    )
                }.filter { it.from.isNotBlank() },
            )
        }

        /** Lit un fichier `vocab.toml` (celui de Windows ou un export Android). */
        fun fromToml(text: String): Vocab {
            val root = Toml.parse(text)
            @Suppress("UNCHECKED_CAST")
            val reps = (root["replacements"] as? List<Any?>).orEmpty().mapNotNull { it as? Map<String, Any?> }
            return Vocab(
                context = root["context"] as? String ?: "",
                terms = (root["terms"] as? List<*>).orEmpty().mapNotNull { it as? String },
                replacements = reps.map {
                    Replacement(
                        from = it["from"] as? String ?: "",
                        to = it["to"] as? String ?: "",
                        wholeWord = it["whole_word"] as? Boolean ?: true,
                        caseSensitive = it["case_sensitive"] as? Boolean ?: false,
                    )
                }.filter { it.from.isNotBlank() },
            )
        }

        private fun tomlString(value: String) = buildString {
            append('"')
            for (c in value) {
                when {
                    c == '"' -> append("\\\"")
                    c == '\\' -> append("\\\\")
                    c == '\n' -> append("\\n")
                    c == '\t' -> append("\\t")
                    c == '\r' -> append("\\r")
                    c < ' ' || c == '\u007F' -> append("\\u%04X".format(c.code))
                    else -> append(c)
                }
            }
            append('"')
        }
    }
}

/**
 * Lecteur TOML minimal, suffisant pour `vocab.toml` : chaînes (simples,
 * littérales, multilignes), booléens, nombres, tableaux, tables et tableaux
 * de tables. Les tables ordinaires sont lues mais rangées sous leur nom.
 */
object Toml {
    class Error(message: String) : IllegalArgumentException(message)

    fun parse(text: String): Map<String, Any?> = Parser(text).document()

    private class Parser(val s: String) {
        var i = 0
        val root = LinkedHashMap<String, Any?>()

        fun document(): Map<String, Any?> {
            var table: MutableMap<String, Any?> = root
            while (true) {
                skipBlank()
                if (i >= s.length) return root
                when {
                    s.startsWith("[[", i) -> {
                        i += 2
                        val name = key()
                        expect("]]")
                        @Suppress("UNCHECKED_CAST")
                        val list = root.getOrPut(name) { ArrayList<Any?>() } as? MutableList<Any?>
                            ?: throw Error("« $name » est déjà une valeur")
                        table = LinkedHashMap<String, Any?>().also { list.add(it) }
                    }
                    s[i] == '[' -> {
                        i++
                        val name = key()
                        expect("]")
                        table = LinkedHashMap<String, Any?>().also { root[name] = it }
                    }
                    else -> {
                        val k = key()
                        skipSpaces()
                        expect("=")
                        skipSpaces()
                        table[k] = value()
                    }
                }
                skipSpaces()
                if (i < s.length && s[i] == '#') skipComment()
                if (i < s.length && s[i] != '\n' && s[i] != '\r') throw Error("ligne ${line()} : fin de ligne attendue")
            }
        }

        fun key(): String {
            skipSpaces()
            val k = when {
                i < s.length && s[i] == '"' -> basicString()
                i < s.length && s[i] == '\'' -> literalString()
                else -> {
                    val start = i
                    while (i < s.length && (s[i].isLetterOrDigit() || s[i] == '_' || s[i] == '-' || s[i] == '.')) i++
                    if (start == i) throw Error("ligne ${line()} : nom attendu")
                    s.substring(start, i)
                }
            }
            skipSpaces()
            return k
        }

        fun value(): Any? = when {
            s.startsWith("\"\"\"", i) -> multiline("\"\"\"", escapes = true)
            s.startsWith("'''", i) -> multiline("'''", escapes = false)
            i < s.length && s[i] == '"' -> basicString()
            i < s.length && s[i] == '\'' -> literalString()
            i < s.length && s[i] == '[' -> array()
            i < s.length && s[i] == '{' -> inlineTable()
            s.startsWith("true", i) -> true.also { i += 4 }
            s.startsWith("false", i) -> false.also { i += 5 }
            else -> {
                val start = i
                while (i < s.length && s[i] !in ",]}\n\r#" && !s[i].isWhitespace()) i++
                if (start == i) throw Error("ligne ${line()} : valeur attendue")
                s.substring(start, i).replace("_", "").let { it.toLongOrNull() ?: it.toDoubleOrNull() ?: it }
            }
        }

        fun array(): List<Any?> {
            i++
            val out = ArrayList<Any?>()
            while (true) {
                skipBlank()
                if (i >= s.length) throw Error("tableau non fermé")
                if (s[i] == ']') { i++; return out }
                out.add(value())
                skipBlank()
                if (i < s.length && s[i] == ',') i++
            }
        }

        fun inlineTable(): Map<String, Any?> {
            i++
            val out = LinkedHashMap<String, Any?>()
            while (true) {
                skipSpaces()
                if (i >= s.length) throw Error("table non fermée")
                if (s[i] == '}') { i++; return out }
                val k = key()
                expect("=")
                skipSpaces()
                out[k] = value()
                skipSpaces()
                if (i < s.length && s[i] == ',') i++
            }
        }

        fun basicString(): String {
            i++
            val out = StringBuilder()
            while (true) {
                if (i >= s.length || s[i] == '\n') throw Error("ligne ${line()} : chaîne non fermée")
                val c = s[i++]
                when (c) {
                    '"' -> return out.toString()
                    '\\' -> escape(out)
                    else -> out.append(c)
                }
            }
        }

        fun literalString(): String {
            i++
            val end = s.indexOf('\'', i)
            if (end < 0 || s.substring(i, end).contains('\n')) throw Error("ligne ${line()} : chaîne non fermée")
            return s.substring(i, end).also { i = end + 1 }
        }

        fun multiline(delim: String, escapes: Boolean): String {
            i += 3
            if (s.startsWith("\r\n", i)) i += 2 else if (i < s.length && s[i] == '\n') i++
            val out = StringBuilder()
            while (true) {
                if (i >= s.length) throw Error("chaîne multiligne non fermée")
                if (s.startsWith(delim, i)) { i += 3; return out.toString() }
                val c = s[i++]
                if (escapes && c == '\\') {
                    if (i < s.length && (s[i] == '\n' || s[i] == '\r' || s[i] == ' ')) {
                        // Barre oblique en fin de ligne : on recolle la ligne suivante.
                        while (i < s.length && s[i].isWhitespace()) i++
                    } else {
                        escape(out)
                    }
                } else {
                    out.append(c)
                }
            }
        }

        fun escape(out: StringBuilder) {
            if (i >= s.length) throw Error("échappement incomplet")
            when (val e = s[i++]) {
                'n' -> out.append('\n')
                't' -> out.append('\t')
                'r' -> out.append('\r')
                'b' -> out.append('\b')
                'f' -> out.append('\u000C')
                '"' -> out.append('"')
                '\\' -> out.append('\\')
                'u', 'U' -> {
                    val n = if (e == 'u') 4 else 8
                    val code = s.substring(i, minOf(i + n, s.length)).toIntOrNull(16)
                        ?: throw Error("ligne ${line()} : \\$e invalide")
                    out.appendCodePoint(code)
                    i += n
                }
                else -> throw Error("ligne ${line()} : échappement \\$e inconnu")
            }
        }

        fun expect(token: String) {
            if (!s.startsWith(token, i)) throw Error("ligne ${line()} : « $token » attendu")
            i += token.length
        }

        fun skipSpaces() {
            while (i < s.length && (s[i] == ' ' || s[i] == '\t')) i++
        }

        fun skipComment() {
            while (i < s.length && s[i] != '\n') i++
        }

        fun skipBlank() {
            while (i < s.length) {
                when {
                    s[i].isWhitespace() -> i++
                    s[i] == '#' -> skipComment()
                    else -> return
                }
            }
        }

        fun line() = s.substring(0, minOf(i, s.length)).count { it == '\n' } + 1
    }
}
