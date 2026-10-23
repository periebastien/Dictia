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
pub const ENGINE_STANDARD: &str = "standard";
pub const ENGINE_REALTIME: &str = "realtime";
pub const API_KEY_ENV: &str = "OPENAI_API_KEY";
/// Permet de pointer vers un autre dossier (tests, profils multiples).
const DIR_ENV: &str = "DICTIA_CONFIG_DIR";

/// Contenu écrit dans `config.toml` au premier lancement.
/// Doit rester synchronisé avec les `Default` ci-dessous (vérifié par un test).
pub const DEFAULT_CONFIG: &str = r##"# Configuration de Dictia
# Fichier créé automatiquement au premier lancement.
# Relancez Dictia après toute modification.

[api]
# Fournisseur de transcription actif : "openai", "mistral" ou "local".
# Chaque fournisseur a sa clé, son adresse et son modèle (sections
# [api] pour OpenAI, [mistral] et [local] ci-dessous), modifiables aussi
# dans Réglages.
provider = "openai"
# --- OpenAI ---
# Clé API OpenAI. Laissez vide pour utiliser la variable d'environnement
# OPENAI_API_KEY. Ne partagez jamais ce fichier s'il contient votre clé.
api_key = ""
# URL de base de l'API OpenAI.
base_url = "https://api.openai.com/v1"
# Moteur de transcription :
#   "standard" : l'audio est envoyé à la fin de la dictée (éprouvé) ;
#   "realtime" : l'audio est envoyé pendant la dictée, le texte arrive
#                presque aussitôt au relâchement (repli automatique sur
#                "standard" en cas d'échec). OpenAI et Mistral.
engine = "standard"
# Modèle du moteur standard.
model = "gpt-4o-transcribe"
# Modèle du moteur realtime : gpt-live-transcribe, gpt-realtime-whisper,
# gpt-4o-transcribe ou gpt-4o-mini-transcribe.
realtime_model = "gpt-live-transcribe"
# --- Communs à tous les fournisseurs ---
# Langue de la dictée (code ISO 639-1 : fr, en, es…) ou "auto" pour que le
# modèle la reconnaisse lui-même (utile si vous dictez en plusieurs langues).
language = "fr"
# Délai maximal d'une transcription, en secondes.
timeout_secs = 30

[mistral]
# Clé API Mistral. Laissez vide pour utiliser MISTRAL_API_KEY.
api_key = ""
base_url = "https://api.mistral.ai/v1"
# Modèle Voxtral du moteur standard (ex. voxtral-mini-latest).
model = "voxtral-mini-latest"
# Modèle Voxtral du moteur realtime (choisi avec engine = "realtime").
realtime_model = "voxtral-mini-transcribe-realtime-2602"

[local]
# Serveur de transcription sur votre PC, compatible avec l'API OpenAI
# (par exemple vLLM avec un modèle Voxtral). Clé facultative.
api_key = ""
base_url = "http://localhost:8000/v1"
model = "mistralai/Voxtral-Mini-4B-Realtime-2602"

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
# « Maintenir pour parler » : l'écoute continue ce temps (ms) après le
# relâchement, pour ne pas couper la fin d'une phrase. 0 = arrêt immédiat.
release_tail_ms = 300
# Micro à utiliser (nom exact, voir la fenêtre Dictia → Réglages).
# Vide : micro par défaut de Windows.
device = ""
# Garde le micro ouvert entre les dictées : l'enregistrement démarre sans
# délai (utile avec un casque Bluetooth). Rien n'est gardé ni envoyé hors des
# dictées, mais Windows affiche en permanence « micro en cours d'utilisation ».
keep_mic_open = false

[inject]
# Ajoute une espace après le texte tapé, pour enchaîner les dictées.
trailing_space = true
# Retire la ponctuation (point final, virgules…) des textes courts : jusqu'à
# ce nombre de mots. 0 : jamais.
short_text_words = 3
# Moteur realtime : tape le texte au fil de la dictée (mot par mot), dès
# qu'aucune touche Ctrl, Alt ou Win n'est enfoncée. Le texte final, avec les
# remplacements, corrige au besoin ce qui a été tapé.
live_typing = true

[indicator]
# Animation à côté du curseur : point rouge et barres de volume pendant
# l'écoute, points bleus pendant la transcription.
enabled = true

[ui]
# Couleur d'accent de la fenêtre Dictia.
accent = "#7C83FF"
"##;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub api: ApiConfig,
    pub mistral: ProviderConfig,
    pub local: ProviderConfig,
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

/// Valeur de `language` qui laisse le modèle reconnaître la langue.
pub const LANGUAGE_AUTO: &str = "auto";

#[derive(Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    pub provider: String,
    pub api_key: String,
    pub base_url: String,
    pub engine: String,
    pub model: String,
    pub realtime_model: String,
    pub language: String,
    pub timeout_secs: u64,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            provider: "openai".into(),
            api_key: String::new(),
            base_url: "https://api.openai.com/v1".into(),
            engine: ENGINE_STANDARD.into(),
            model: "gpt-4o-transcribe".into(),
            realtime_model: "gpt-live-transcribe".into(),
            language: "fr".into(),
            timeout_secs: 30,
        }
    }
}

// Debug manuel : la clé API ne doit jamais apparaître dans les journaux.
impl fmt::Debug for ApiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiConfig")
            .field("provider", &self.provider)
            .field("api_key", &if self.api_key.is_empty() { "" } else { "***" })
            .field("base_url", &self.base_url)
            .field("engine", &self.engine)
            .field("model", &self.model)
            .field("realtime_model", &self.realtime_model)
            .field("language", &self.language)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

impl ApiConfig {
    /// Code de langue à transmettre au modèle ; vide en mode automatique.
    pub fn language_code(&self) -> &str {
        let code = self.language.trim();
        if code.eq_ignore_ascii_case(LANGUAGE_AUTO) {
            ""
        } else {
            code
        }
    }

    pub fn is_realtime(&self) -> bool {
        self.engine.trim().eq_ignore_ascii_case(ENGINE_REALTIME)
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
    pub release_tail_ms: u32,
    /// Nom du micro ; vide : micro par défaut de Windows.
    pub device: String,
    /// Garde le micro ouvert entre les dictées.
    pub keep_mic_open: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            silence_threshold_db: -50.0,
            padding_ms: 150,
            min_speech_ms: 200,
            release_tail_ms: 300,
            device: String::new(),
            keep_mic_open: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct InjectConfig {
    pub trailing_space: bool,
    /// Textes d'au plus ce nombre de mots tapés sans ponctuation (0 : jamais).
    pub short_text_words: u32,
    pub live_typing: bool,
}

impl Default for InjectConfig {
    fn default() -> Self {
        Self {
            trailing_space: true,
            short_text_words: 3,
            live_typing: true,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api: ApiConfig::default(),
            mistral: ProviderConfig {
                api_key: String::new(),
                base_url: Provider::Mistral.default_base_url().into(),
                model: Provider::Mistral.default_model().into(),
                realtime_model: Provider::Mistral.default_realtime_model().into(),
            },
            local: ProviderConfig {
                api_key: String::new(),
                base_url: Provider::Local.default_base_url().into(),
                model: Provider::Local.default_model().into(),
                realtime_model: String::new(),
            },
            hotkey: HotkeyConfig::default(),
            audio: AudioConfig::default(),
            inject: InjectConfig::default(),
            indicator: IndicatorConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

/// Réglages d'un fournisseur autre qu'OpenAI.
#[derive(Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    /// Modèle du moteur realtime (fournisseurs qui le proposent).
    pub realtime_model: String,
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("api_key", &if self.api_key.is_empty() { "" } else { "***" })
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("realtime_model", &self.realtime_model)
            .finish()
    }
}

/// Fournisseur de transcription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAi,
    Mistral,
    Local,
}

/// Format de requête et de réponse de l'API de transcription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    OpenAi,
    Mistral,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Self::OpenAi, Self::Mistral, Self::Local];

    pub fn id(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Mistral => "mistral",
            Self::Local => "local",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Mistral => "Mistral",
            Self::Local => "Local",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|p| p.id().eq_ignore_ascii_case(id.trim()))
    }

    /// Variable d'environnement de secours pour la clé.
    pub fn key_env(self) -> Option<&'static str> {
        match self {
            Self::OpenAi => Some(API_KEY_ENV),
            Self::Mistral => Some("MISTRAL_API_KEY"),
            Self::Local => None,
        }
    }

    /// Un serveur local peut se passer de clé.
    pub fn requires_key(self) -> bool {
        self != Self::Local
    }

    pub fn dialect(self) -> Dialect {
        match self {
            Self::Mistral => Dialect::Mistral,
            Self::OpenAi | Self::Local => Dialect::OpenAi,
        }
    }

    /// Adresse utilisée si aucune n'est renseignée.
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Mistral => "https://api.mistral.ai/v1",
            Self::Local => "http://localhost:8000/v1",
        }
    }

    /// Modèle utilisé si aucun n'est renseigné.
    pub fn default_model(self) -> &'static str {
        match self {
            Self::OpenAi => "gpt-4o-transcribe",
            Self::Mistral => "voxtral-mini-latest",
            Self::Local => "mistralai/Voxtral-Mini-4B-Realtime-2602",
        }
    }

    /// Fournisseurs proposant le moteur realtime de Dictia.
    pub fn supports_realtime(self) -> bool {
        self != Self::Local
    }

    /// Modèle realtime utilisé si aucun n'est renseigné.
    pub fn default_realtime_model(self) -> &'static str {
        match self {
            Self::OpenAi => "gpt-live-transcribe",
            Self::Mistral => "voxtral-mini-transcribe-realtime-2602",
            Self::Local => "",
        }
    }
}

/// Paramètres effectifs d'un fournisseur.
#[derive(Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub provider: Provider,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

impl Config {
    /// Fournisseur actif (OpenAI si la valeur est inconnue).
    pub fn provider(&self) -> Provider {
        Provider::parse(&self.api.provider).unwrap_or(Provider::OpenAi)
    }

    /// Paramètres d'un fournisseur ; `env` lit une variable d'environnement.
    pub fn endpoint(&self, provider: Provider, env: impl Fn(&str) -> Option<String>) -> Endpoint {
        let (key, base_url, model) = match provider {
            Provider::OpenAi => (&self.api.api_key, &self.api.base_url, &self.api.model),
            Provider::Mistral => (
                &self.mistral.api_key,
                &self.mistral.base_url,
                &self.mistral.model,
            ),
            Provider::Local => (&self.local.api_key, &self.local.base_url, &self.local.model),
        };
        let api_key = Some(key.trim().to_owned())
            .filter(|k| !k.is_empty())
            .or_else(|| provider.key_env().and_then(&env))
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty());
        // Une section créée par l'interface peut ne contenir que la clé.
        let or_default = |value: &str, default: &str| {
            let value = value.trim();
            if value.is_empty() { default } else { value }.to_owned()
        };
        Endpoint {
            provider,
            base_url: or_default(base_url, provider.default_base_url()),
            model: or_default(model, provider.default_model()),
            api_key,
        }
    }

    /// Modèle realtime d'un fournisseur.
    pub fn realtime_model(&self, provider: Provider) -> String {
        let model = match provider {
            Provider::OpenAi => &self.api.realtime_model,
            Provider::Mistral => &self.mistral.realtime_model,
            Provider::Local => &self.local.realtime_model,
        }
        .trim();
        if model.is_empty() {
            provider.default_realtime_model()
        } else {
            model
        }
        .to_owned()
    }

    /// Moteur realtime effectivement utilisé (choisi et disponible).
    pub fn uses_realtime(&self) -> bool {
        self.provider().supports_realtime() && self.api.is_realtime()
    }
}

/// Variables d'environnement du processus.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
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
    fn automatic_language_sends_no_code() {
        let mut api = ApiConfig::default();
        assert_eq!(api.language_code(), "fr");
        for auto in ["auto", " AUTO ", ""] {
            api.language = auto.into();
            assert_eq!(api.language_code(), "");
        }
    }

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
    fn providers_and_their_keys() {
        let mut cfg = Config::default();
        assert_eq!(cfg.provider(), Provider::OpenAi);
        cfg.api.provider = "Mistral".into();
        assert_eq!(cfg.provider(), Provider::Mistral);
        cfg.api.provider = "inconnu".into();
        assert_eq!(cfg.provider(), Provider::OpenAi);

        let env = |name: &str| (name == "MISTRAL_API_KEY").then(|| " sk-env ".to_owned());
        let m = cfg.endpoint(Provider::Mistral, env);
        assert_eq!(m.api_key.as_deref(), Some("sk-env"));
        assert_eq!(m.base_url, "https://api.mistral.ai/v1");
        assert_eq!(m.model, "voxtral-mini-latest");
        cfg.mistral.api_key = "mk".into();
        assert_eq!(
            cfg.endpoint(Provider::Mistral, env).api_key.as_deref(),
            Some("mk")
        );
        assert_eq!(cfg.endpoint(Provider::OpenAi, env).api_key, None);
        // Le serveur local ne lit aucune variable d'environnement.
        assert_eq!(
            cfg.endpoint(Provider::Local, |_| Some("x".into())).api_key,
            None
        );
    }

    #[test]
    fn section_with_only_a_key_keeps_default_address_and_model() {
        let cfg = Config::parse("[mistral]\napi_key = \"mk\"\n").unwrap();
        let m = cfg.endpoint(Provider::Mistral, |_| None);
        assert_eq!(m.base_url, "https://api.mistral.ai/v1");
        assert_eq!(m.model, "voxtral-mini-latest");
        assert_eq!(m.api_key.as_deref(), Some("mk"));
        let l = cfg.endpoint(Provider::Local, |_| None);
        assert_eq!(l.base_url, "http://localhost:8000/v1");
    }

    #[test]
    fn realtime_with_openai_and_mistral() {
        let mut cfg = Config::default();
        cfg.api.engine = "realtime".into();
        assert!(cfg.uses_realtime());
        assert_eq!(cfg.realtime_model(Provider::OpenAi), "gpt-live-transcribe");
        cfg.api.provider = "mistral".into();
        assert!(cfg.uses_realtime());
        assert_eq!(
            cfg.realtime_model(Provider::Mistral),
            "voxtral-mini-transcribe-realtime-2602"
        );
        // Une section [mistral] ancienne, sans realtime_model.
        cfg.mistral.realtime_model.clear();
        assert_eq!(
            cfg.realtime_model(Provider::Mistral),
            "voxtral-mini-transcribe-realtime-2602"
        );
        cfg.api.provider = "local".into();
        assert!(!cfg.uses_realtime());
    }

    #[test]
    fn provider_debug_hides_key() {
        let p = ProviderConfig {
            api_key: "secret".into(),
            ..Default::default()
        };
        assert!(!format!("{p:?}").contains("secret"));
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
