//! Configuration : fichiers TOML dans `%APPDATA%\Dictia\`.
//!
//! Les fichiers sont créés au premier lancement avec des valeurs par défaut
//! commentées. Les champs absents prennent leur valeur par défaut, pour qu'un
//! fichier ancien reste valide quand de nouveaux réglages apparaissent.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

pub const CONFIG_FILE: &str = "config.toml";
pub const API_KEY_ENV: &str = "OPENAI_API_KEY";
/// Permet de pointer vers un autre dossier (tests, profils multiples).
const DIR_ENV: &str = "DICTIA_CONFIG_DIR";

/// Contenu écrit dans `config.toml` au premier lancement.
/// Doit rester synchronisé avec les `Default` ci-dessous (vérifié par un test).
pub const DEFAULT_CONFIG: &str = r##"# Configuration de Dictia
# Fichier créé automatiquement au premier lancement.
# Relancez Dictia après toute modification.

[api]
# Clé API OpenAI. Laissez vide pour utiliser la variable d'environnement
# OPENAI_API_KEY. Ne partagez jamais ce fichier s'il contient votre clé.
api_key = ""
# URL de base de l'API (compatible OpenAI).
base_url = "https://api.openai.com/v1"
# Modèle de transcription.
model = "gpt-4o-transcribe"
# Langue de la dictée (code ISO 639-1).
language = "fr"
# Délai maximal d'une transcription, en secondes.
timeout_secs = 30

[hotkey]
# Modificateurs : Ctrl, Alt, Maj (ou Shift), Win.
# Touches : Espace, Entrée, F1 à F24, A à Z, 0 à 9…
#
# Mode bascule : 1er appui = début, 2e appui = fin.
shortcut = "Ctrl+Alt+Espace"
# Mode « maintenir pour parler » (push-to-talk) : l'enregistrement dure tant
# que le raccourci est maintenu. Laissez vide ("") pour le désactiver.
push_to_talk = "Alt+Maj+Espace"

[audio]
# Niveau (dBFS) sous lequel un bloc de 20 ms est considéré comme du silence.
# Les silences de début et de fin sont retirés avant l'envoi.
# Augmentez (ex. -40) dans un environnement bruyant, baissez (ex. -55) si
# le début ou la fin de vos phrases est coupé.
silence_threshold_db = -50.0
# Marge conservée avant et après la parole, en millisecondes.
padding_ms = 150
# Durée minimale de parole pour lancer une transcription, en millisecondes.
min_speech_ms = 200

[inject]
# Ajoute une espace après le texte tapé, pour enchaîner les dictées.
trailing_space = true

[indicator]
# Animation à côté du curseur : point rouge et barres de volume pendant
# l'écoute, points orange pendant la transcription.
enabled = true

[ui]
# Couleur d'accent de la fenêtre Dictia.
accent = "#7C83FF"
"##;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub api: ApiConfig,
    pub hotkey: HotkeyConfig,
    pub audio: AudioConfig,
    pub inject: InjectConfig,
    pub indicator: IndicatorConfig,
    pub ui: UiConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub accent: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            accent: "#7C83FF".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct IndicatorConfig {
    pub enabled: bool,
}

impl Default for IndicatorConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub language: String,
    pub timeout_secs: u64,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4o-transcribe".into(),
            language: "fr".into(),
            timeout_secs: 30,
        }
    }
}

// Debug manuel : la clé API ne doit jamais apparaître dans les journaux.
impl fmt::Debug for ApiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiConfig")
            .field("api_key", &if self.api_key.is_empty() { "" } else { "***" })
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("language", &self.language)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

impl ApiConfig {
    /// Clé du fichier si renseignée, sinon celle de l'environnement.
    pub fn resolve_api_key(&self, env_value: Option<String>) -> Option<String> {
        let from_file = self.api_key.trim();
        if !from_file.is_empty() {
            return Some(from_file.to_owned());
        }
        env_value
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    pub shortcut: String,
    pub push_to_talk: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            shortcut: "Ctrl+Alt+Espace".into(),
            push_to_talk: "Alt+Maj+Espace".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub silence_threshold_db: f32,
    pub padding_ms: u32,
    pub min_speech_ms: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            silence_threshold_db: -50.0,
            padding_ms: 150,
            min_speech_ms: 200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct InjectConfig {
    pub trailing_space: bool,
}

impl Default for InjectConfig {
    fn default() -> Self {
        Self {
            trailing_space: true,
        }
    }
}

/// `%APPDATA%\Dictia\`, ou `DICTIA_CONFIG_DIR` si cette variable est définie.
pub fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    dirs::config_dir()
        .map(|d| d.join("Dictia"))
        .context("dossier %APPDATA% introuvable")
}

/// Crée `path` avec `default` s'il n'existe pas. Renvoie `true` si créé.
pub fn ensure_file(path: &Path, default: &str) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("création du dossier {}", parent.display()))?;
    }
    fs::write(path, default).with_context(|| format!("écriture de {}", path.display()))?;
    Ok(true)
}

impl Config {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// Charge `config.toml` depuis `dir`, en le créant au besoin.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let path = dir.join(CONFIG_FILE);
        if ensure_file(&path, DEFAULT_CONFIG)? {
            log::info!("{} créé avec les valeurs par défaut", path.display());
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("lecture de {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("syntaxe invalide dans {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_matches_default_struct() {
        assert_eq!(Config::parse(DEFAULT_CONFIG).unwrap(), Config::default());
    }

    #[test]
    fn missing_fields_take_defaults() {
        let cfg = Config::parse("[api]\nmodel = \"whisper-1\"\n").unwrap();
        assert_eq!(cfg.api.model, "whisper-1");
        assert_eq!(cfg.api.language, "fr");
        assert_eq!(cfg.hotkey, HotkeyConfig::default());
    }

    #[test]
    fn api_key_from_file_wins_over_env() {
        let api = ApiConfig {
            api_key: " sk-file ".into(),
            ..Default::default()
        };
        assert_eq!(
            api.resolve_api_key(Some("sk-env".into())).as_deref(),
            Some("sk-file")
        );
    }

    #[test]
    fn api_key_falls_back_to_env() {
        let api = ApiConfig::default();
        assert_eq!(
            api.resolve_api_key(Some("sk-env".into())).as_deref(),
            Some("sk-env")
        );
        assert_eq!(api.resolve_api_key(Some("  ".into())), None);
        assert_eq!(api.resolve_api_key(None), None);
    }

    #[test]
    fn debug_never_prints_api_key() {
        let api = ApiConfig {
            api_key: "sk-secret".into(),
            ..Default::default()
        };
        assert!(!format!("{api:?}").contains("sk-secret"));
    }

    #[test]
    fn load_or_create_writes_default_file_once() {
        let dir = std::env::temp_dir().join(format!("dictia-cfg-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let cfg = Config::load_or_create(&dir).unwrap();
        assert_eq!(cfg, Config::default());
        assert_eq!(
            fs::read_to_string(dir.join(CONFIG_FILE)).unwrap(),
            DEFAULT_CONFIG
        );
        assert!(!ensure_file(&dir.join(CONFIG_FILE), "x").unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }
}
