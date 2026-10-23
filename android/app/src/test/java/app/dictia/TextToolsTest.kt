package app.dictia

import org.junit.Assert.assertEquals
import org.junit.Test

class TextToolsTest {
    @Test
    fun stripsPunctuationOfShortTexts() {
        assertEquals("Ça va", TextTools.stripShortPunctuation("Ça va ?", 3))
        assertEquals("Oui d'accord", TextTools.stripShortPunctuation("Oui, d'accord !", 3))
        assertEquals("Rendez-vous à 12:30", TextTools.stripShortPunctuation("Rendez-vous à 12:30.", 3))
        assertEquals("Un deux trois quatre.", TextTools.stripShortPunctuation("Un deux trois quatre.", 3))
        assertEquals("Oui.", TextTools.stripShortPunctuation("Oui.", 0))
    }

    @Test
    fun insertsAtCursorWithSpacing() {
        assertEquals(TextTools.Merged("Bonjour", 7), TextTools.merge("", -1, -1, "Bonjour"))
        assertEquals(TextTools.Merged("Salut Marc", 10), TextTools.merge("Salut", 5, 5, "Marc"))
        assertEquals(TextTools.Merged("Salut Marc", 10), TextTools.merge("Salut ", 6, 6, "Marc"))
        assertEquals(TextTools.Merged("Un deux trois", 8), TextTools.merge("Un trois", 3, 3, "deux"))
        assertEquals(TextTools.Merged("Un deux", 7), TextTools.merge("Un trois", 3, 8, "deux"))
        assertEquals(TextTools.Merged("Fin.", 4), TextTools.merge("Fin", 3, 3, "."))
    }
}
