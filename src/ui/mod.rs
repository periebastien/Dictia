//! Fenêtre de Dictia : page HTML embarquée et protocole d'échange JSON.
//!
//! La page (`index.html`) envoie des [`Command`] via `window.ipc.postMessage`
//! et reçoit des [`Message`] via `window.dictia.receive(…)`. L'hôte WebView2
//! est dans `window.rs` (Windows uniquement).

#[cfg(windows)]
pub mod window;

use serde::{Deserialize, Serialize};

use crate::config::{Config, Provider, ENGINE_REALTIME, ENGINE_STANDARD};
use crate::history::{Entry, Problem};
use crate::settings::{
    mask_api_key, suggested_models, Change, VocabEdit, ACCENTS, LANGUAGES, RELEASE_TAILS,
    SHORT_TEXT_WORDS,
};
use crate::stats::ModelStats;
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
    /// Le problème affiché a été lu.
    DismissProblem,
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
    /// Fournisseur actif.
    pub provider: &'static str,
    pub providers: Vec<ProviderView>,
    pub engine: String,
    /// Code de langue (`auto` compris) et choix proposés.
    pub language: String,
    pub languages: Vec<LanguageView>,
    pub shortcut: String,
    pub push_to_talk: String,
    pub indicator: bool,
    pub trailing_space: bool,
    pub short_text_words: u32,
    pub short_text_choices: Vec<u32>,
    pub live_typing: bool,
    pub release_tail_ms: u32,
    pub release_tails: Vec<u32>,
    /// Micro choisi (vide : celui de Windows) et micros branchés.
    pub microphone: String,
    pub microphones: Vec<String>,
    pub keep_mic_open: bool,
    pub start_with_windows: bool,
    pub accent: String,
    pub accents: Vec<AccentView>,
    pub version: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LanguageView {
    pub code: String,
    pub name: String,
}

/// Langues proposées ; un code saisi à la main dans `config.toml` est ajouté
/// pour rester affiché.
fn language_views(current: &str) -> Vec<LanguageView> {
    let mut views: Vec<LanguageView> = LANGUAGES
        .iter()
        .map(|&(code, name)| LanguageView {
            code: code.into(),
            name: name.into(),
        })
        .collect();
    if !views.iter().any(|v| v.code == current) {
        views.push(LanguageView {
            code: current.into(),
            name: current.into(),
        });
    }
    views
}

/// Réglages d'un fournisseur (jamais la clé entière).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProviderView {
    pub id: &'static str,
    pub name: &'static str,
    pub api_key_set: bool,
    pub api_key_hint: String,
    pub needs_key: bool,
    pub base_url: String,
    pub model: String,
    pub models: &'static [&'static str],
    /// Moteur realtime proposé, avec son modèle et les choix possibles.
    pub realtime: bool,
    pub realtime_model: String,
    pub realtime_models: &'static [&'static str],
}

impl SettingsView {
    /// `env` lit les variables d'environnement (clés de secours).
    pub fn new(
        config: &Config,
        env: impl Fn(&str) -> Option<String>,
        start_with_windows: bool,
        microphones: &[String],
    ) -> Self {
        const NAMES: [&str; 4] = ["Indigo", "Bleu ciel", "Vert", "Ambre"];
        let providers = Provider::ALL
            .into_iter()
            .map(|p| {
                let e = config.endpoint(p, &env);
                ProviderView {
                    id: p.id(),
                    name: p.name(),
                    api_key_set: e.api_key.is_some(),
                    api_key_hint: e.api_key.as_deref().map(mask_api_key).unwrap_or_default(),
                    needs_key: p.requires_key(),
                    base_url: e.base_url,
                    model: e.model,
                    models: suggested_models(p),
                    realtime: p.supports_realtime(),
                    realtime_model: config.realtime_model(p),
                    realtime_models: realtime::models(p),
                }
            })
            .collect();
        let language = match config.api.language_code() {
            "" => crate::config::LANGUAGE_AUTO.to_owned(),
            code => code.to_ascii_lowercase(),
        };
        Self {
            provider: config.provider().id(),
            providers,
            engine: if config.api.is_realtime() {
                ENGINE_REALTIME
            } else {
                ENGINE_STANDARD
            }
            .to_owned(),
            language: language.clone(),
            languages: language_views(&language),
            shortcut: config.hotkey.shortcut.clone(),
            push_to_talk: config.hotkey.push_to_talk.clone(),
            indicator: config.indicator.enabled,
            trailing_space: config.inject.trailing_space,
            short_text_words: config.inject.short_text_words,
            short_text_choices: SHORT_TEXT_WORDS.to_vec(),
            live_typing: config.inject.live_typing,
            release_tail_ms: config.audio.release_tail_ms,
            release_tails: RELEASE_TAILS.to_vec(),
            microphone: config.audio.device.clone(),
            microphones: {
                // Un micro choisi mais débranché reste affiché.
                let mut list = microphones.to_vec();
                let device = &config.audio.device;
                if !device.is_empty() && !list.contains(device) {
                    list.push(device.clone());
                }
                list
            },
            keep_mic_open: config.audio.keep_mic_open,
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
        stats: Vec<ModelStats>,
        tab: Option<Tab>,
        problem: Option<Problem>,
    },
    Stats {
        stats: Vec<ModelStats>,
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
    /// Dernier problème (`None` : rien à signaler).
    Problem {
        problem: Option<Problem>,
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
            Command::parse(r#"{"cmd":"dismiss_problem"}"#),
            Some(Command::DismissProblem)
        );
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
        let mut config = Config::default();
        config.mistral.api_key = "mk-secret-mistral-9876".into();
        let env = |name: &str| (name == "OPENAI_API_KEY").then(|| "sk-proj-secret1234".to_owned());
        let view = SettingsView::new(&config, env, true, &[]);
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("secret"), "{json}");
        assert_eq!(view.provider, "openai");
        let ids: Vec<_> = view.providers.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["openai", "mistral", "local"]);
        assert_eq!(view.providers[0].api_key_hint, "sk-…1234");
        assert_eq!(view.providers[1].api_key_hint, "mk-…9876");
        assert!(!view.providers[2].api_key_set && !view.providers[2].needs_key);
        assert!(view.providers[0].realtime && view.providers[1].realtime);
        assert!(!view.providers[2].realtime);
        assert_eq!(view.accents.len(), 4);
        assert_eq!(view.release_tail_ms, 300);
        assert_eq!(view.engine, "standard");
        assert_eq!(view.providers[0].realtime_model, "gpt-live-transcribe");
        assert_eq!(
            view.providers[1].realtime_model,
            "voxtral-mini-transcribe-realtime-2602"
        );
        assert_eq!(view.providers[1].realtime_models.len(), 2);
        assert!(
            !SettingsView::new(&Config::default(), |_| None, false, &[]).providers[0].api_key_set
        );
    }

    #[test]
    fn settings_view_keeps_an_unplugged_microphone() {
        let mut config = Config::default();
        let plugged = ["Micro (Realtek)".to_owned()];
        let view = SettingsView::new(&config, |_| None, false, &plugged);
        assert_eq!(view.microphone, "");
        assert_eq!(view.microphones, plugged);
        assert!(!view.keep_mic_open);
        config.audio.device = "Casque Bluetooth".into();
        config.audio.keep_mic_open = true;
        let view = SettingsView::new(&config, |_| None, false, &plugged);
        assert_eq!(view.microphones, ["Micro (Realtek)", "Casque Bluetooth"]);
        assert!(view.keep_mic_open);
    }

    #[test]
    fn settings_view_lists_languages() {
        let mut config = Config::default();
        let view = SettingsView::new(&config, |_| None, false, &[]);
        assert_eq!(view.language, "fr");
        assert_eq!(view.languages[0].code, "auto");
        assert_eq!(view.languages.len(), crate::settings::LANGUAGES.len());
        for (value, shown) in [("", "auto"), ("Auto", "auto"), ("SV", "sv")] {
            config.api.language = value.into();
            let view = SettingsView::new(&config, |_| None, false, &[]);
            assert_eq!(view.language, shown);
            assert!(view.languages.iter().any(|l| l.code == shown));
        }
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

    #[test]
    fn problem_message_for_the_page() {
        let message = Message::Problem {
            problem: Some(Problem {
                at_ms: 42,
                text: "Realtime indisponible : HTTP 401".into(),
            }),
        };
        let script = message.to_script();
        assert!(
            script.contains(r#"{"type":"problem","problem":{"at_ms":42,"text":"Realtime indisponible : HTTP 401"}}"#),
            "{script}"
        );
    }
}
