//! Raccourci clavier global, en mode bascule.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Result};

/// Convertit un raccourci saisi en français (« Ctrl+Alt+Espace ») vers la
/// syntaxe de `global-hotkey` (« ctrl+alt+space »).
pub fn normalize(shortcut: &str) -> Result<String> {
    let tokens: Vec<String> = shortcut
        .split('+')
        .map(|t| t.trim().to_lowercase())
        .collect();
    if tokens.iter().any(String::is_empty) {
        bail!("raccourci invalide : {shortcut:?}");
    }
    let mapped: Vec<&str> = tokens
        .iter()
        .map(|t| match t.as_str() {
            "ctrl" | "control" | "contrôle" | "controle" => "ctrl",
            "maj" | "majuscule" | "shift" => "shift",
            "win" | "windows" | "super" | "meta" => "super",
            "espace" | "space" => "space",
            "entrée" | "entree" | "enter" => "enter",
            "échap" | "echap" | "échappement" | "esc" | "escape" => "escape",
            "suppr" | "supprimer" => "delete",
            "inser" | "insertion" => "insert",
            "début" | "debut" => "home",
            "fin" => "end",
            "retour" | "retour arrière" => "backspace",
            other => other,
        })
        .collect();
    Ok(mapped.join("+"))
}

/// Ignore l'auto-répétition : un maintien prolongé ne compte que pour un
/// appui, le suivant n'étant accepté qu'après relâchement.
#[derive(Debug, Default)]
pub struct PressFilter {
    held: AtomicBool,
}

impl PressFilter {
    /// Renvoie `true` pour un appui à traiter.
    pub fn pressed(&self) -> bool {
        !self.held.swap(true, Ordering::SeqCst)
    }

    pub fn released(&self) {
        self.held.store(false, Ordering::SeqCst);
    }
}

#[cfg(windows)]
pub use self::win::register;

#[cfg(windows)]
mod win {
    use anyhow::{anyhow, Context, Result};
    use global_hotkey::hotkey::HotKey;
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

    use super::{normalize, PressFilter};

    /// Garde le raccourci enregistré tant qu'elle vit.
    pub struct Registration {
        _manager: GlobalHotKeyManager,
    }

    /// Enregistre `shortcut` ; `on_press` est appelé depuis la boucle de
    /// messages à chaque appui (hors auto-répétition).
    pub fn register(
        shortcut: &str,
        on_press: impl Fn() + Send + Sync + 'static,
    ) -> Result<Registration> {
        let hotkey: HotKey = normalize(shortcut)?
            .parse()
            .map_err(|e| anyhow!("raccourci {shortcut:?} non reconnu : {e}"))?;
        let manager =
            GlobalHotKeyManager::new().context("initialisation des raccourcis globaux")?;
        manager.register(hotkey).map_err(|e| {
            anyhow!("le raccourci {shortcut} est indisponible (déjà utilisé par une autre application ?) : {e}")
        })?;
        let id = hotkey.id();
        let filter = PressFilter::default();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.id() != id {
                return;
            }
            match event.state() {
                HotKeyState::Pressed => {
                    if filter.pressed() {
                        on_press();
                    }
                }
                HotKeyState::Released => filter.released(),
            }
        }));
        Ok(Registration { _manager: manager })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_french_shortcuts() {
        assert_eq!(normalize("Ctrl+Alt+Espace").unwrap(), "ctrl+alt+space");
        assert_eq!(normalize(" Maj + Win + F9 ").unwrap(), "shift+super+f9");
        assert_eq!(normalize("Ctrl+Échap").unwrap(), "ctrl+escape");
        assert_eq!(normalize("ctrl+alt+space").unwrap(), "ctrl+alt+space");
    }

    #[test]
    fn rejects_empty_tokens() {
        assert!(normalize("").is_err());
        assert!(normalize("Ctrl++A").is_err());
    }

    #[test]
    fn press_filter_ignores_auto_repeat() {
        let f = PressFilter::default();
        assert!(f.pressed());
        assert!(!f.pressed()); // répétition pendant le maintien
        f.released();
        assert!(f.pressed());
    }
}
