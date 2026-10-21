//! Raccourcis clavier globaux : mode bascule et mode « maintenir pour parler ».

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Result};

use crate::config::HotkeyConfig;

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

/// Comportement d'un raccourci.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 1er appui = début, 2e appui = fin.
    Toggle,
    /// Enregistre tant que le raccourci est maintenu.
    PushToTalk,
}

/// Ordre transmis au contrôleur de dictée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Toggle,
    Start,
    Stop,
}

/// Raccourcis actifs d'après la configuration (le push-to-talk est
/// facultatif). Refuse deux fois le même raccourci.
pub fn bindings(cfg: &HotkeyConfig) -> Result<Vec<(String, Mode)>> {
    let mut out = vec![(cfg.shortcut.trim().to_owned(), Mode::Toggle)];
    let ptt = cfg.push_to_talk.trim();
    if !ptt.is_empty() {
        if normalize(ptt)? == normalize(&cfg.shortcut)? {
            bail!("shortcut et push_to_talk doivent être deux raccourcis différents ({ptt})");
        }
        out.push((ptt.to_owned(), Mode::PushToTalk));
    }
    Ok(out)
}

/// Description courte pour l'infobulle et le journal.
pub fn describe(cfg: &HotkeyConfig) -> String {
    let ptt = cfg.push_to_talk.trim();
    if ptt.is_empty() {
        cfg.shortcut.trim().to_owned()
    } else {
        format!("{}, ou maintenir {ptt}", cfg.shortcut.trim())
    }
}

/// Traduit les appuis et relâchements d'un raccourci en actions, en
/// ignorant l'auto-répétition du clavier pendant un maintien.
#[derive(Debug)]
pub struct Binding {
    mode: Mode,
    held: AtomicBool,
}

impl Binding {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            held: AtomicBool::new(false),
        }
    }

    pub fn on_pressed(&self) -> Option<Action> {
        if self.held.swap(true, Ordering::SeqCst) {
            return None; // auto-répétition
        }
        Some(match self.mode {
            Mode::Toggle => Action::Toggle,
            Mode::PushToTalk => Action::Start,
        })
    }

    pub fn on_released(&self) -> Option<Action> {
        let was_held = self.held.swap(false, Ordering::SeqCst);
        (was_held && self.mode == Mode::PushToTalk).then_some(Action::Stop)
    }
}

#[cfg(windows)]
pub use self::win::Registration;

#[cfg(windows)]
mod win {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use anyhow::{anyhow, Context, Result};
    use global_hotkey::hotkey::HotKey;
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

    use super::{normalize, Action, Binding, Mode};

    type Table = Arc<Mutex<HashMap<u32, Binding>>>;

    /// Raccourcis enregistrés auprès de Windows, modifiables à chaud.
    pub struct Registration {
        manager: GlobalHotKeyManager,
        table: Table,
        active: Vec<(String, HotKey, Mode)>,
        suspended: bool,
    }

    fn parse(shortcut: &str) -> Result<HotKey> {
        normalize(shortcut)?
            .parse()
            .map_err(|e| anyhow!("raccourci {shortcut:?} non reconnu : {e}"))
    }

    impl Registration {
        /// Enregistre les raccourcis ; `on_action` est appelé depuis la boucle
        /// de messages (appui) ou un thread de `global-hotkey` (relâchement).
        pub fn new(
            bindings: &[(String, Mode)],
            on_action: impl Fn(Action) + Send + Sync + 'static,
        ) -> Result<Self> {
            let manager =
                GlobalHotKeyManager::new().context("initialisation des raccourcis globaux")?;
            let table: Table = Arc::default();
            let handler_table = Arc::clone(&table);
            GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                let action = {
                    let Ok(table) = handler_table.lock() else {
                        return;
                    };
                    let Some(binding) = table.get(&event.id()) else {
                        return;
                    };
                    match event.state() {
                        HotKeyState::Pressed => binding.on_pressed(),
                        HotKeyState::Released => binding.on_released(),
                    }
                };
                if let Some(action) = action {
                    on_action(action);
                }
            }));
            let mut registration = Self {
                manager,
                table,
                active: Vec::new(),
                suspended: false,
            };
            registration.replace(bindings)?;
            Ok(registration)
        }

        fn register_all(&self, list: &[(String, HotKey, Mode)]) -> Result<()> {
            for (i, (shortcut, hotkey, _)) in list.iter().enumerate() {
                if let Err(e) = self.manager.register(*hotkey) {
                    for (_, done, _) in &list[..i] {
                        let _ = self.manager.unregister(*done);
                    }
                    return Err(anyhow!(
                        "le raccourci {shortcut} est indisponible (déjà utilisé par une autre application ?) : {e}"
                    ));
                }
            }
            Ok(())
        }

        fn unregister_all(&self) {
            for (_, hotkey, _) in &self.active {
                let _ = self.manager.unregister(*hotkey);
            }
        }

        /// Remplace les raccourcis. En cas d'échec, les anciens restent actifs.
        pub fn replace(&mut self, bindings: &[(String, Mode)]) -> Result<()> {
            let next = bindings
                .iter()
                .map(|(s, m)| Ok((s.clone(), parse(s)?, *m)))
                .collect::<Result<Vec<_>>>()?;
            if !self.suspended {
                self.unregister_all();
                if let Err(e) = self.register_all(&next) {
                    let _ = self.register_all(&self.active);
                    return Err(e);
                }
            }
            if let Ok(mut table) = self.table.lock() {
                *table = next
                    .iter()
                    .map(|(_, h, m)| (h.id(), Binding::new(*m)))
                    .collect();
            }
            self.active = next;
            Ok(())
        }

        /// Désactive temporairement les raccourcis (saisie d'une combinaison
        /// dans l'interface).
        pub fn suspend(&mut self, on: bool) {
            if on == self.suspended {
                return;
            }
            if on {
                self.unregister_all();
            } else if let Err(e) = self.register_all(&self.active) {
                log::error!("{e:#}");
            }
            self.suspended = on;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(shortcut: &str, ptt: &str) -> HotkeyConfig {
        HotkeyConfig {
            shortcut: shortcut.into(),
            push_to_talk: ptt.into(),
        }
    }

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
    fn toggle_ignores_auto_repeat_and_release() {
        let b = Binding::new(Mode::Toggle);
        assert_eq!(b.on_pressed(), Some(Action::Toggle));
        assert_eq!(b.on_pressed(), None); // répétition pendant le maintien
        assert_eq!(b.on_released(), None);
        assert_eq!(b.on_pressed(), Some(Action::Toggle));
    }

    #[test]
    fn push_to_talk_starts_on_press_and_stops_once_on_release() {
        let b = Binding::new(Mode::PushToTalk);
        assert_eq!(b.on_pressed(), Some(Action::Start));
        assert_eq!(b.on_pressed(), None);
        assert_eq!(b.on_released(), Some(Action::Stop));
        // Relâchements en double (un par répétition) : un seul arrêt.
        assert_eq!(b.on_released(), None);
    }

    #[test]
    fn bindings_from_config() {
        let both = bindings(&cfg("Ctrl+Alt+Espace", " Alt+Maj+Espace ")).unwrap();
        assert_eq!(
            both,
            [
                ("Ctrl+Alt+Espace".to_owned(), Mode::Toggle),
                ("Alt+Maj+Espace".to_owned(), Mode::PushToTalk)
            ]
        );
        assert_eq!(bindings(&cfg("Ctrl+Alt+Espace", "")).unwrap().len(), 1);
        assert!(bindings(&cfg("Ctrl+Alt+Espace", "ctrl+alt+space")).is_err());
    }

    #[test]
    fn describes_shortcuts() {
        assert_eq!(
            describe(&cfg("Ctrl+Alt+Espace", "Alt+Maj+Espace")),
            "Ctrl+Alt+Espace, ou maintenir Alt+Maj+Espace"
        );
        assert_eq!(describe(&cfg("Ctrl+Alt+Espace", " ")), "Ctrl+Alt+Espace");
    }
}
