//! Vocabulaire : prompt envoyé au modèle et remplacements appliqués au texte.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use regex::{NoExpand, Regex};
use serde::Deserialize;

use crate::config::ensure_file;

pub const VOCAB_FILE: &str = "vocab.toml";

/// Contenu écrit dans `vocab.toml` au premier lancement.
pub const DEFAULT_VOCAB: &str = r#"# Vocabulaire de Dictia
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
#   to             : texte de remplacement ("\n" pour un retour à la ligne)
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
# to = "\n"
"#;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct VocabFile {
    pub context: String,
    pub terms: Vec<String>,
    pub replacements: Vec<ReplacementRule>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ReplacementRule {
    pub from: String,
    pub to: String,
    #[serde(default = "default_true")]
    pub whole_word: bool,
    #[serde(default)]
    pub case_sensitive: bool,
}

fn default_true() -> bool {
    true
}

/// Vocabulaire prêt à l'emploi : prompt construit et règles compilées.
#[derive(Debug, Clone, Default)]
pub struct Vocab {
    prompt: String,
    replacements: Vec<(Regex, String)>,
}

impl Vocab {
    pub fn from_file(file: &VocabFile) -> Result<Self> {
        let replacements = file
            .replacements
            .iter()
            .filter(|r| !r.from.trim().is_empty())
            .map(|r| Ok((compile_rule(r)?, r.to.clone())))
            .collect::<Result<_>>()?;
        Ok(Self {
            prompt: build_prompt(&file.context, &file.terms),
            replacements,
        })
    }

    pub fn parse(text: &str) -> Result<Self> {
        Self::from_file(&toml::from_str(text)?)
    }

    /// Charge `vocab.toml` depuis `dir`, en le créant au besoin.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let path = dir.join(VOCAB_FILE);
        if ensure_file(&path, DEFAULT_VOCAB)? {
            log::info!("{} créé avec les valeurs par défaut", path.display());
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("lecture de {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("contenu invalide dans {}", path.display()))
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn rule_count(&self) -> usize {
        self.replacements.len()
    }

    /// Applique les remplacements, dans l'ordre du fichier.
    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for (re, to) in &self.replacements {
            if let std::borrow::Cow::Owned(replaced) = re.replace_all(&out, NoExpand(to)) {
                out = replaced;
            }
        }
        out
    }
}

/// Contexte libre suivi de la liste des termes.
pub fn build_prompt(context: &str, terms: &[String]) -> String {
    let terms: Vec<&str> = terms
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect();
    let mut parts = Vec::new();
    let context = context.trim();
    if !context.is_empty() {
        parts.push(context.to_owned());
    }
    if !terms.is_empty() {
        parts.push(format!("Vocabulaire : {}.", terms.join(", ")));
    }
    parts.join(" ")
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn compile_rule(rule: &ReplacementRule) -> Result<Regex> {
    let from = rule.from.trim();
    let mut pattern = String::new();
    if !rule.case_sensitive {
        pattern.push_str("(?i)");
    }
    // `\b` n'a de sens qu'à côté d'un caractère de mot : « ; » ou « ... »
    // en bordure doivent rester remplaçables.
    let starts_word = from.chars().next().is_some_and(is_word_char);
    let ends_word = from.chars().last().is_some_and(is_word_char);
    if rule.whole_word && starts_word {
        pattern.push_str(r"\b");
    }
    pattern.push_str(&regex::escape(from));
    if rule.whole_word && ends_word {
        pattern.push_str(r"\b");
    }
    Regex::new(&pattern).with_context(|| format!("remplacement invalide : {from:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(from: &str, to: &str) -> ReplacementRule {
        ReplacementRule {
            from: from.into(),
            to: to.into(),
            whole_word: true,
            case_sensitive: false,
        }
    }

    fn vocab(rules: Vec<ReplacementRule>) -> Vocab {
        Vocab::from_file(&VocabFile {
            replacements: rules,
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn default_file_parses() {
        let v = Vocab::parse(DEFAULT_VOCAB).unwrap();
        assert_eq!(
            v.prompt(),
            "Dictée en français, avec une ponctuation soignée. Vocabulaire : Dictia, OpenAI."
        );
        assert_eq!(v.rule_count(), 0);
    }

    #[test]
    fn prompt_skips_empty_parts() {
        assert_eq!(build_prompt("  ", &[]), "");
        assert_eq!(
            build_prompt("", &["Rust".into(), " ".into(), "Tokio".into()]),
            "Vocabulaire : Rust, Tokio."
        );
        assert_eq!(build_prompt(" Bonjour. ", &[]), "Bonjour.");
    }

    #[test]
    fn replaces_whole_words_case_insensitively() {
        let v = vocab(vec![rule("open ai", "OpenAI")]);
        assert_eq!(
            v.apply("J'utilise Open AI et open ai."),
            "J'utilise OpenAI et OpenAI."
        );
        // Pas de remplacement au milieu d'un mot.
        assert_eq!(v.apply("reopen aire"), "reopen aire");
    }

    #[test]
    fn handles_accents_as_word_characters() {
        let v = vocab(vec![rule("dictia", "Dictia")]);
        assert_eq!(v.apply("dictia, dictiaé"), "Dictia, dictiaé");
    }

    #[test]
    fn case_sensitive_and_partial_rules() {
        let v = vocab(vec![
            ReplacementRule {
                case_sensitive: true,
                ..rule("Api", "API")
            },
            ReplacementRule {
                whole_word: false,
                ..rule("ph", "f")
            },
        ]);
        assert_eq!(v.apply("Api api photo"), "API api foto");
    }

    #[test]
    fn punctuation_rules_and_special_characters() {
        let v = vocab(vec![
            rule("...", "…"),
            rule("à la ligne", "\n"),
            rule("dollar", "$1"),
        ]);
        assert_eq!(v.apply("Fin... à la ligne"), "Fin… \n");
        // `$1` est inséré tel quel, sans interprétation.
        assert_eq!(v.apply("un dollar"), "un $1");
    }

    #[test]
    fn rules_apply_in_order_and_skip_empty() {
        let v = vocab(vec![rule("", "x"), rule("a", "b"), rule("b", "c")]);
        assert_eq!(v.rule_count(), 2);
        assert_eq!(v.apply("a b"), "c c");
    }

    #[test]
    fn parses_replacements_from_toml() {
        let v = Vocab::parse(
            "[[replacements]]\nfrom = \"point virgule\"\nto = \";\"\n\
             [[replacements]]\nfrom = \"NB\"\nto = \"N.B.\"\ncase_sensitive = true\n",
        )
        .unwrap();
        assert_eq!(v.apply("a point virgule nb NB"), "a ; nb N.B.");
    }
}
