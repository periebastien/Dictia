package app.dictia

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class VocabTest {
    /** `DEFAULT_VOCAB` de la version Windows (src/vocab.rs). */
    private val windowsDefault = """# Vocabulaire de Dictia
# Fichier créé automatiquement au premier lancement.
# Relancez Dictia après toute modification.

# Contexte libre transmis au modèle (prompt). Décrivez le style attendu :
# le modèle imite la ponctuation et le registre de ce texte.
context = "Dictée en français, avec une ponctuation soignée."

# Noms propres, sigles et termes techniques que le modèle doit orthographier
# correctement. Ils sont ajoutés au prompt.
terms = ["Dictia", "OpenAI"]

# Remplacements appliqués au texte transcrit, dans l'ordre, avant la frappe.
#   from           : texte à chercher
#   to             : texte de remplacement ("\\n" pour un retour à la ligne)
#   whole_word     : ne remplace que des mots entiers (défaut : true)
#   case_sensitive : respecte la casse (défaut : false)
#
# Exemples (retirez les « # » pour les activer) :
#
# [[replacements]]
# from = "point virgule"
# to = ";"
#
# [[replacements]]
# from = "à la ligne"
# to = "\\n"
"""

    @Test
    fun readsTheWindowsFile() {
        val vocab = Vocab.fromToml(windowsDefault)
        assertEquals("Dictée en français, avec une ponctuation soignée.", vocab.context)
        assertEquals(listOf("Dictia", "OpenAI"), vocab.terms)
        assertTrue(vocab.replacements.isEmpty())
    }

    @Test
    fun readsReplacementsAndMultilineArrays() {
        val vocab = Vocab.fromToml(
            """
            context = 'Notes'
            terms = [
                "n8n", # outil
                "Dictia",
            ]

            [[replacements]]
            from = "point virgule"
            to = ";"

            [[replacements]]
            from = "à la ligne"
            to = "\n"
            whole_word = false
            case_sensitive = true
            """.trimIndent(),
        )
        assertEquals("Notes", vocab.context)
        assertEquals(listOf("n8n", "Dictia"), vocab.terms)
        assertEquals(Replacement("point virgule", ";"), vocab.replacements[0])
        assertEquals(Replacement("à la ligne", "\n", wholeWord = false, caseSensitive = true), vocab.replacements[1])
    }

    @Test
    fun exportReadsBackIdentically() {
        val vocab = Vocab(
            context = "Guillemets \" et barre \\ obliques",
            terms = listOf("Écoute", "C++"),
            replacements = listOf(Replacement("à la ligne", "\n"), Replacement("Etc", "etc.", false, true)),
        )
        assertEquals(vocab, Vocab.fromToml(vocab.toToml()))
    }

    @Test
    fun appliesReplacementsOnWholeWords() {
        val vocab = Vocab(
            replacements = listOf(
                Replacement("point virgule", ";"),
                Replacement("à la ligne", "\n"),
                Replacement("ça", "cela"),
            ),
        )
        assertEquals("Bonjour ; merci\nfin", vocab.apply("Bonjour point virgule merci à la ligne fin").replace(" \n ", "\n"))
        assertEquals("cela va, déçu", vocab.apply("Ça va, déçu"))
    }

    @Test
    fun buildsPromptAndDetectsEchoes() {
        val vocab = Vocab(context = "Dictée soignée.", terms = listOf("Dictia", " ", "n8n"))
        assertEquals("Dictée soignée. Vocabulaire : Dictia, n8n.", vocab.prompt)
        assertTrue(vocab.isPromptEcho("Vocabulaire : Dictia, n8n."))
        assertFalse(vocab.isPromptEcho("Dictia"))
    }
}
