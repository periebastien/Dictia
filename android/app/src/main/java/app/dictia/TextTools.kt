package app.dictia

/** Traitements du texte transcrit, sans dépendance Android (testés sous JVM). */
object TextTools {
    private val PUNCTUATION = setOf('.', ',', ';', ':', '!', '?', '…', '¡', '¿')

    /**
     * Retire la ponctuation ajoutée par le modèle aux textes courts
     * (`maxWords` mots ou moins ; 0 = jamais). La ponctuation entre deux
     * caractères alphanumériques (12:30, 3,5) est conservée.
     */
    fun stripShortPunctuation(text: String, maxWords: Int): String {
        val words = text.split(Regex("\\s+")).count { w -> w.any { it.isLetterOrDigit() } }
        if (maxWords <= 0 || words == 0 || words > maxWords) return text
        val out = StringBuilder(text.length)
        for ((i, c) in text.withIndex()) {
            if (c in PUNCTUATION) {
                val between = i > 0 && text[i - 1].isLetterOrDigit() &&
                    text.getOrNull(i + 1)?.isLetterOrDigit() == true
                if (!between) continue
            }
            out.append(c)
        }
        return out.trim().split(Regex("\\s+")).joinToString(" ")
    }

    /** Résultat de l'insertion : nouveau contenu du champ et position du curseur. */
    data class Merged(val text: String, val cursor: Int)

    /**
     * Insère `insert` à la place de la sélection [start, end] de `existing`
     * (curseur en fin de texte si la sélection est inconnue), en ajoutant une
     * espace avant ou après quand le texte serait collé à un mot voisin.
     */
    fun merge(existing: String, start: Int, end: Int, insert: String): Merged {
        val len = existing.length
        val (a, b) = if (start < 0 || end < 0) {
            len to len
        } else {
            minOf(start, end).coerceIn(0, len) to maxOf(start, end).coerceIn(0, len)
        }
        val before = existing.substring(0, a)
        val after = existing.substring(b)
        var piece = insert
        val first = piece.firstOrNull()
        if (before.isNotEmpty() && !before.last().isWhitespace() && first != null && first.isLetterOrDigit()) {
            piece = " $piece"
        }
        if (after.isNotEmpty() && after.first().isLetterOrDigit()) {
            piece = "$piece "
        }
        return Merged(before + piece + after, before.length + piece.length)
    }
}
