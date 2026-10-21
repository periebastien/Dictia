//! Frappe du texte au curseur, sans passer par le presse-papier.
//!
//! Le texte est découpé en frappes : unités UTF-16 envoyées avec
//! `KEYEVENTF_UNICODE` (indépendantes de la disposition du clavier), sauf les
//! retours à la ligne et tabulations, envoyés comme vraies touches pour que
//! les applications les interprètent normalement.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keystroke {
    /// Unité UTF-16 (les caractères hors BMP en produisent deux).
    Unit(u16),
    Enter,
    Tab,
    /// Efface le caractère précédent (correction de la frappe en direct).
    Backspace,
}

/// Texte final à taper : sans blancs superflus, avec l'espace finale
/// optionnelle pour enchaîner les dictées.
pub fn prepare_text(text: &str, trailing_space: bool) -> String {
    let trimmed = text.trim_matches(|c: char| c == ' ' || c == '\u{a0}' || c == '\t');
    let trimmed = trimmed.trim_start_matches(['\r', '\n']);
    let mut out = trimmed.to_owned();
    if trailing_space && !out.is_empty() && !out.ends_with('\n') {
        out.push(' ');
    }
    out
}

pub fn plan(text: &str) -> Vec<Keystroke> {
    let mut keys = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut buf = [0u16; 2];
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                // \r\n compte pour un seul retour à la ligne.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                keys.push(Keystroke::Enter);
            }
            '\n' => keys.push(Keystroke::Enter),
            '\t' => keys.push(Keystroke::Tab),
            c => keys.extend(c.encode_utf16(&mut buf).iter().map(|&u| Keystroke::Unit(u))),
        }
    }
    keys
}

/// Frappes qui effacent `erase` caractères puis tapent `text`.
pub fn correction(erase: usize, text: &str) -> Vec<Keystroke> {
    let mut keys = vec![Keystroke::Backspace; erase];
    keys.extend(plan(text));
    keys
}

/// Frappe en direct possible maintenant : la fenêtre d'origine a le focus
/// et aucune touche ne transformerait les lettres en raccourcis.
#[cfg(windows)]
pub fn can_type_live(target: Option<crate::platform::WindowHandle>) -> bool {
    use crate::platform;
    !platform::shortcut_modifiers_down()
        && target.is_none_or(|t| platform::foreground_window() == Some(t))
}

/// Applique une correction de la frappe en direct. `last` : correction
/// finale, qui attend le relâchement des touches et rend le focus.
/// Renvoie le temps passé à attendre les touches.
#[cfg(windows)]
pub fn apply_correction(
    target: Option<crate::platform::WindowHandle>,
    erase: usize,
    text: &str,
    last: bool,
) -> anyhow::Result<std::time::Duration> {
    use std::time::Duration;

    use crate::platform;

    if erase == 0 && text.is_empty() {
        return Ok(Duration::ZERO);
    }
    let mut keys_wait = Duration::ZERO;
    if last {
        keys_wait = platform::wait_modifiers_released(Duration::from_millis(1500));
        if let Some(target) = target {
            platform::focus_window(target);
        }
    }
    platform::send_keystrokes(&correction(erase, text))?;
    Ok(keys_wait)
}

/// Rend le focus à `target` puis tape `text`. Renvoie le temps passé à
/// attendre le relâchement des touches.
#[cfg(windows)]
pub fn type_into(
    target: Option<crate::platform::WindowHandle>,
    text: &str,
) -> anyhow::Result<std::time::Duration> {
    use std::time::Duration;

    use crate::platform;

    // Ctrl/Alt encore enfoncés transformeraient les frappes en raccourcis.
    let keys_wait = platform::wait_modifiers_released(Duration::from_millis(1500));
    if let Some(target) = target {
        if !platform::focus_window(target) {
            log::warn!("impossible de rendre le focus à la fenêtre d'origine");
        }
    }
    platform::send_keystrokes(&plan(text))?;
    Ok(keys_wait)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_ascii_and_accents() {
        assert_eq!(plan("Çà"), [Keystroke::Unit(0xC7), Keystroke::Unit(0xE0)]);
    }

    #[test]
    fn plans_surrogate_pairs() {
        assert_eq!(
            plan("😀"),
            [Keystroke::Unit(0xD83D), Keystroke::Unit(0xDE00)]
        );
    }

    #[test]
    fn plans_newlines_and_tabs_as_keys() {
        use Keystroke::*;
        assert_eq!(
            plan("a\r\nb\nc\rd\te"),
            [
                Unit(b'a' as u16),
                Enter,
                Unit(b'b' as u16),
                Enter,
                Unit(b'c' as u16),
                Enter,
                Unit(b'd' as u16),
                Tab,
                Unit(b'e' as u16)
            ]
        );
    }

    #[test]
    fn correction_erases_then_types() {
        use Keystroke::*;
        assert_eq!(
            correction(2, "a\n"),
            [Backspace, Backspace, Unit(b'a' as u16), Enter]
        );
        assert!(correction(0, "").is_empty());
    }

    #[test]
    fn prepares_text() {
        assert_eq!(prepare_text("  Bonjour.  ", true), "Bonjour. ");
        assert_eq!(prepare_text("Bonjour.", false), "Bonjour.");
        assert_eq!(prepare_text("\nLigne\n", true), "Ligne\n");
        assert_eq!(prepare_text("   ", true), "");
    }
}
