//! Frappe au fil de la transcription en direct.
//!
//! Le texte est tapé mot par mot pendant la dictée : seul ce qui précède le
//! dernier blanc est considéré comme stable (le dernier mot peut encore
//! changer). À chaque étape, Dictia compare ce qu'il a déjà tapé à ce qu'il
//! faudrait avoir tapé et envoie le minimum : des retours arrière pour la
//! partie qui diffère, puis la suite. La même correction s'applique au texte
//! final, remplacements compris.

/// Partie stable d'un texte en cours : tout jusqu'au dernier blanc inclus.
pub fn stable_prefix(text: &str) -> &str {
    let text = text.trim_start();
    match text.rfind(char::is_whitespace) {
        Some(i) => {
            let end = i + text[i..].chars().next().map_or(1, char::len_utf8);
            &text[..end]
        }
        None => "",
    }
}

/// Modification minimale pour passer de `typed` à `wanted` : nombre de
/// caractères à effacer, puis texte à taper.
pub fn diff<'a>(typed: &str, wanted: &'a str) -> (usize, &'a str) {
    let common = typed
        .char_indices()
        .zip(wanted.char_indices())
        .find(|((_, a), (_, b))| a != b)
        .map_or_else(|| typed.len().min(wanted.len()), |((i, _), _)| i);
    // `common` est une frontière de caractère dans les deux chaînes.
    let erase = typed[common..].chars().count();
    (erase, &wanted[common..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_prefix_keeps_complete_words() {
        assert_eq!(stable_prefix(""), "");
        assert_eq!(stable_prefix("Bonj"), "");
        assert_eq!(stable_prefix(" Bonjour à t"), "Bonjour à ");
        assert_eq!(stable_prefix("Bonjour.\nNouv"), "Bonjour.\n");
        assert_eq!(stable_prefix("déjà vu "), "déjà vu ");
    }

    #[test]
    fn diff_appends_when_text_grows() {
        assert_eq!(diff("", "Bonjour "), (0, "Bonjour "));
        assert_eq!(diff("Bonjour ", "Bonjour à tous "), (0, "à tous "));
        assert_eq!(diff("Bonjour ", "Bonjour "), (0, ""));
    }

    #[test]
    fn diff_corrects_changed_words() {
        // « presta shop » remplacé par « PrestaShop » au texte final.
        assert_eq!(
            diff("Sur presta shop ", "Sur PrestaShop."),
            (12, "PrestaShop.")
        );
        // Accents : les retours arrière comptent des caractères, pas des octets.
        assert_eq!(diff("Il était été ", "Il était "), (4, ""));
        assert_eq!(diff("Ça va ", ""), (6, ""));
    }
}
