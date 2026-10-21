//! Fenêtre de Dictia : page HTML embarquée et protocole d'échange JSON.
//!
//! La page (`index.html`) envoie des [`Command`] via `window.ipc.postMessage`
//! et reçoit des [`Message`] via `window.dictia.receive(…)`. L'hôte WebView2
//! est dans `window.rs` (Windows uniquement).

#[cfg(windows)]
pub mod window;

use serde::{Deserialize, Serialize};

use crate::config::{Config, ENGINE_REALTIME, ENGINE_STANDARD};
use crate::history::Entry;
use crate::settings::{mask_api_key, Change, VocabEdit, ACCENTS, MODELS, RELEASE_TAILS};
use crate::stats::EngineStats;
use crate::transcription::realtime;
use crate::tray::TrayState;

/// Page de l'interface, embarquée dans l'exécutable.
pub const HTML: &str = include_str!("index.html");
/// Taille de la fenêtre (px logiques, comme les maquettes).
pub const WIDTH: u32 = 380;
pub const HEIGHT: u32 = 560;

/// Ordre envoyé par la page.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Page chargée : envoyer l'état complet.
    Ready,
    /// Début du déplacement de la fenêtre (barre de titre).
    Drag,
    Minimize,
    Close,
    Change {
        change: Change,
    },
    Vocab {
        vocab: VocabEdit,
    },
    Startup {
        value: bool,
    },
    Copy {
        text: String,
    },
    Reinsert {
        text: String,
    },
    /// Saisie d'un raccourci en cours : les raccourcis globaux sont suspendus
    /// pour que la page reçoive la combinaison.
    Capture {
        active: bool,
    },
    OpenFolder,
}

impl Command {
    pub fn parse(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}

/// Onglets de la page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tab {
    Raccourcis,
    Reglages,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccentView {
    pub hex: &'static str,
    pub name: &'static str,
}

/// Réglages tels que la page les affiche (jamais la clé API complète).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsView {
    pub api_key_set: bool,
    pub api_key_hint: String,
    pub engine: String,
    pub model: String,
    pub models: Vec<&'static str>,
    pub realtime_model: String,
    pub realtime_models: Vec<&'static str>,
    pub shortcut: String,
    pub push_to_talk: String,
    pub indicator: bool,
    pub trailing_space: bool,
    pub live_typing: bool,
    pub release_tail_ms: u32,
    pub release_tails: Vec<u32>,
    pub start_with_windows: bool,
    pub accent: String,
    pub accents: Vec<AccentView>,
    pub version: &'static str,
}

impl SettingsView {
    pub fn new(config: &Config, api_key: Option<&str>, start_with_windows: bool) -> Self {
        const NAMES: [&str; 4] = ["Indigo", "Bleu ciel", "Vert", "Ambre"];
        Self {
            api_key_set: api_key.is_some(),
            api_key_hint: api_key.map(mask_api_key).unwrap_or_default(),
            engine: if config.api.is_realtime() {
                ENGINE_REALTIME
            } else {
                ENGINE_STANDARD
            }
            .to_owned(),
            model: config.api.model.clone(),
            models: MODELS.to_vec(),
            realtime_model: config.api.realtime_model.clone(),
            realtime_models: realtime::MODELS.to_vec(),
            shortcut: config.hotkey.shortcut.clone(),
            push_to_talk: config.hotkey.push_to_talk.clone(),
            indicator: config.indicator.enabled,
            trailing_space: config.inject.trailing_space,
            live_typing: config.inject.live_typing,
            release_tail_ms: config.audio.release_tail_ms,
            release_tails: RELEASE_TAILS.to_vec(),
            start_with_windows,
            accent: config.ui.accent.clone(),
            accents: ACCENTS
                .iter()
                .zip(NAMES)
                .map(|(&hex, name)| AccentView { hex, name })
                .collect(),
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

pub fn status_name(state: TrayState) -> &'static str {
    match state {
        TrayState::Idle => "idle",
        TrayState::Recording => "recording",
        TrayState::Processing => "processing",
        TrayState::Error => "error",
    }
}

/// Message envoyé à la page.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Snapshot {
        status: &'static str,
        settings: SettingsView,
        vocab: VocabEdit,
        history: Vec<Entry>,
        stats: Vec<EngineStats>,
        tab: Option<Tab>,
    },
    Stats {
        stats: Vec<EngineStats>,
    },
    Settings {
        settings: SettingsView,
    },
    Vocab {
        vocab: VocabEdit,
    },
    Status {
        status: &'static str,
    },
    HistoryAdd {
        entry: Entry,
    },
    Toast {
        text: String,
        error: bool,
    },
    Tab {
        tab: Tab,
    },
}

impl Message {
    pub fn toast(text: impl Into<String>) -> Self {
        Self::Toast {
            text: text.into(),
            error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self::Toast {
            text: text.into(),
            error: true,
        }
    }

    /// Script qui remet le message à la page.
    pub fn to_script(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_else(|_| "{}".into());
        format!("window.dictia&&window.dictia.receive({json})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ReplacementView;

    #[test]
    fn parses_page_commands() {
        assert_eq!(Command::parse(r#"{"cmd":"ready"}"#), Some(Command::Ready));
        assert_eq!(
            Command::parse(
                r#"{"cmd":"change","change":{"kind":"push_to_talk","value":"Ctrl+Maj+P"}}"#
            ),
            Some(Command::Change {
                change: Change::PushToTalk {
                    value: "Ctrl+Maj+P".into()
                }
            })
        );
        assert_eq!(
            Command::parse(r#"{"cmd":"change","change":{"kind":"indicator","value":false}}"#),
            Some(Command::Change {
                change: Change::Indicator { value: false }
            })
        );
        assert_eq!(
            Command::parse(
                r#"{"cmd":"vocab","vocab":{"terms":["n8n"],"replacements":[{"from":"a","to":"\n"}]}}"#
            ),
            Some(Command::Vocab {
                vocab: VocabEdit {
                    terms: vec!["n8n".into()],
                    replacements: vec![ReplacementView {
                        from: "a".into(),
                        to: "\n".into()
                    }],
                }
            })
        );
        assert_eq!(
            Command::parse(r#"{"cmd":"capture","active":true}"#),
            Some(Command::Capture { active: true })
        );
        assert_eq!(Command::parse(r#"{"cmd":"inconnu"}"#), None);
        assert_eq!(Command::parse("pas du json"), None);
    }

    #[test]
    fn settings_view_hides_the_key() {
        let view = SettingsView::new(&Config::default(), Some("sk-proj-secret1234"), true);
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("secret"), "{json}");
        assert_eq!(view.api_key_hint, "sk-…1234");
        assert_eq!(view.accents.len(), 4);
        assert_eq!(view.release_tail_ms, 300);
        assert_eq!(view.engine, "standard");
        assert_eq!(view.realtime_model, "gpt-live-transcribe");
        assert!(!SettingsView::new(&Config::default(), None, false).api_key_set);
    }

    #[test]
    fn messages_become_page_scripts() {
        let script = Message::Status {
            status: "recording",
        }
        .to_script();
        assert_eq!(
            script,
            r#"window.dictia&&window.dictia.receive({"type":"status","status":"recording"})"#
        );
        // Le texte est échappé en JSON : pas d'injection de script possible.
        let script = Message::toast("</script>\"); alert(1); (\"").to_script();
        assert!(script.contains(r#"\"); alert(1); (\""#), "{script}");
    }

    #[test]
    fn page_uses_every_message_and_command() {
        for needle in [
            "window.ipc.postMessage",
            "case 'snapshot'",
            "case 'settings'",
            "case 'vocab'",
            "case 'status'",
            "case 'history_add'",
            "case 'toast'",
            "case 'tab'",
            "case 'stats'",
            "cmd: 'ready'",
            "cmd: 'drag'",
            "cmd: 'minimize'",
            "cmd: 'close'",
            "cmd: 'change'",
            "cmd: 'vocab'",
            "cmd: 'startup'",
            "cmd: 'copy'",
            "cmd: 'reinsert'",
            "cmd: 'capture'",
            "cmd: 'open_folder'",
        ] {
            assert!(HTML.contains(needle), "index.html : {needle} manquant");
        }
        for status in ["idle", "recording", "processing", "error"] {
            assert!(HTML.contains(&format!(".pill.{status}")), "{status}");
        }
    }
}
