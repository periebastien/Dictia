//! Modification des réglages depuis l'interface.
//!
//! Les fichiers TOML sont mis à jour avec `toml_edit` : seules les valeurs
//! changées bougent, les commentaires et la mise en forme sont conservés.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};

use crate::config::{Config, HotkeyConfig};
use crate::hotkey;
use crate::vocab::VocabFile;

/// Modèles proposés dans l'interface.
pub const MODELS: [&str; 3] = ["gpt-4o-transcribe", "gpt-4o-mini-transcribe", "whisper-1"];
/// Couleurs d'accent proposées.
pub const ACCENTS: [&str; 4] = ["#7C83FF", "#38BDF8", "#34D399", "#F59E0B"];

/// Modification demandée par l'interface.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    ApiKey { value: String },
    Model { value: String },
    Shortcut { value: String },
    PushToTalk { value: String },
    Indicator { value: bool },
    TrailingSpace { value: bool },
    Accent { value: String },
}

/// Vocabulaire tel que l'interface l'édite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VocabEdit {
    pub terms: Vec<String>,
    pub replacements: Vec<ReplacementView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplacementView {
    pub from: String,
    pub to: String,
}

/// Indice de clé API à afficher : « sk-…abcd », jamais la clé entière.
pub fn mask_api_key(key: &str) -> String {
    let key = key.trim();
    if key.is_empty() {
        return String::new();
    }
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let head: String = key.chars().take(3).collect();
    format!("{head}…{tail}")
}

fn table<'a>(doc: &'a mut DocumentMut, name: &str) -> &'a mut Table {
    if !doc.contains_table(name) {
        doc[name] = Item::Table(Table::new());
    }
    doc[name].as_table_mut().expect("table créée ci-dessus")
}

/// Applique `change` au texte de `config.toml` ; `current` sert à valider
/// les raccourcis (deux raccourcis identiques sont refusés).
pub fn apply_config_change(text: &str, current: &Config, change: &Change) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("config.toml illisible")?;
    match change {
        Change::ApiKey { value: v } => table(&mut doc, "api")["api_key"] = value(v.trim()),
        Change::Model { value: v } => {
            if !MODELS.contains(&v.as_str()) {
                bail!("modèle inconnu : {v}");
            }
            table(&mut doc, "api")["model"] = value(v.as_str());
        }
        Change::Shortcut { value: v } | Change::PushToTalk { value: v } => {
            let mut hk: HotkeyConfig = current.hotkey.clone();
            let key = if matches!(change, Change::Shortcut { .. }) {
                if v.trim().is_empty() {
                    bail!("le raccourci de bascule ne peut pas être vide");
                }
                hk.shortcut = v.trim().to_owned();
                "shortcut"
            } else {
                hk.push_to_talk = v.trim().to_owned();
                "push_to_talk"
            };
            hotkey::bindings(&hk)?;
            table(&mut doc, "hotkey")[key] = value(v.trim());
        }
        Change::Indicator { value: v } => table(&mut doc, "indicator")["enabled"] = value(*v),
        Change::TrailingSpace { value: v } => {
            table(&mut doc, "inject")["trailing_space"] = value(*v)
        }
        Change::Accent { value: v } => {
            if !ACCENTS.iter().any(|a| a.eq_ignore_ascii_case(v)) {
                bail!("couleur inconnue : {v}");
            }
            table(&mut doc, "ui")["accent"] = value(v.as_str());
        }
    }
    Ok(doc.to_string())
}

/// Vue éditable du vocabulaire (les options avancées des remplacements
/// restent dans le fichier).
pub fn vocab_view(file: &VocabFile) -> VocabEdit {
    VocabEdit {
        terms: file.terms.clone(),
        replacements: file
            .replacements
            .iter()
            .map(|r| ReplacementView {
                from: r.from.clone(),
                to: r.to.clone(),
            })
            .collect(),
    }
}

/// Réécrit termes et remplacements dans le texte de `vocab.toml`, en gardant
/// le contexte, les commentaires et les options des remplacements existants.
pub fn apply_vocab_edit(text: &str, current: &VocabFile, edit: &VocabEdit) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("vocab.toml illisible")?;

    let mut terms = Array::new();
    for t in edit
        .terms
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
    {
        terms.push(t);
    }
    doc["terms"] = value(terms);

    let mut rules = ArrayOfTables::new();
    for r in &edit.replacements {
        let (from, to) = (r.from.trim(), r.to.as_str());
        if from.is_empty() {
            continue;
        }
        let mut t = Table::new();
        t["from"] = value(from);
        t["to"] = value(to);
        // Conserve les options d'une règle existante de même source.
        if let Some(old) = current.replacements.iter().find(|o| o.from.trim() == from) {
            if !old.whole_word {
                t["whole_word"] = value(false);
            }
            if old.case_sensitive {
                t["case_sensitive"] = value(true);
            }
        }
        rules.push(t);
    }
    doc.remove("replacements");
    if !rules.is_empty() {
        doc["replacements"] = Item::ArrayOfTables(rules);
    }
    let out = doc.to_string();
    // Vérifie que le résultat reste un vocabulaire valide.
    crate::vocab::Vocab::parse(&out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_CONFIG;
    use crate::vocab::{ReplacementRule, DEFAULT_VOCAB};

    fn rule(from: &str, to: &str) -> ReplacementRule {
        ReplacementRule {
            from: from.into(),
            to: to.into(),
            whole_word: true,
            case_sensitive: false,
        }
    }

    fn apply(change: Change) -> Result<Config> {
        let text = apply_config_change(DEFAULT_CONFIG, &Config::default(), &change)?;
        assert!(
            text.contains("# Configuration de Dictia"),
            "commentaires perdus"
        );
        Config::parse(&text)
    }

    #[test]
    fn masks_api_key() {
        assert_eq!(mask_api_key("sk-proj-1234567890abcd"), "sk-…abcd");
        assert_eq!(mask_api_key("  "), "");
    }

    #[test]
    fn edits_config_values_and_keeps_comments() {
        assert_eq!(
            apply(Change::ApiKey {
                value: " sk-x ".into()
            })
            .unwrap()
            .api
            .api_key,
            "sk-x"
        );
        assert_eq!(
            apply(Change::Model {
                value: "whisper-1".into()
            })
            .unwrap()
            .api
            .model,
            "whisper-1"
        );
        assert!(
            !apply(Change::Indicator { value: false })
                .unwrap()
                .indicator
                .enabled
        );
        assert!(
            !apply(Change::TrailingSpace { value: false })
                .unwrap()
                .inject
                .trailing_space
        );
        assert_eq!(
            apply(Change::Accent {
                value: "#34D399".into()
            })
            .unwrap()
            .ui
            .accent,
            "#34D399"
        );
        let cfg = apply(Change::PushToTalk {
            value: "Ctrl+Maj+P".into(),
        })
        .unwrap();
        assert_eq!(cfg.hotkey.push_to_talk, "Ctrl+Maj+P");
        assert_eq!(
            apply(Change::PushToTalk { value: "".into() })
                .unwrap()
                .hotkey
                .push_to_talk,
            ""
        );
    }

    #[test]
    fn rejects_invalid_changes() {
        assert!(apply(Change::Model {
            value: "gpt-9".into()
        })
        .is_err());
        assert!(apply(Change::Accent {
            value: "#000000".into()
        })
        .is_err());
        assert!(apply(Change::Shortcut { value: "".into() }).is_err());
        // Identique au raccourci push-to-talk par défaut.
        assert!(apply(Change::Shortcut {
            value: "Alt+Maj+Espace".into()
        })
        .is_err());
        assert!(apply(Change::Shortcut {
            value: "Ctrl++".into()
        })
        .is_err());
    }

    #[test]
    fn adds_missing_sections() {
        let text = apply_config_change("", &Config::default(), &Change::Indicator { value: false })
            .unwrap();
        assert!(!Config::parse(&text).unwrap().indicator.enabled);
    }

    #[test]
    fn edits_vocab_and_keeps_context_comments_and_options() {
        let current = VocabFile {
            replacements: vec![ReplacementRule {
                case_sensitive: true,
                ..rule("NB", "N.B.")
            }],
            ..Default::default()
        };
        let edit = VocabEdit {
            terms: vec!["JetEngine".into(), " ".into(), "n8n".into()],
            replacements: vec![
                ReplacementView {
                    from: "NB".into(),
                    to: "N.B.".into(),
                },
                ReplacementView {
                    from: "presta shop".into(),
                    to: "PrestaShop".into(),
                },
                ReplacementView {
                    from: "".into(),
                    to: "x".into(),
                },
            ],
        };
        let out = apply_vocab_edit(DEFAULT_VOCAB, &current, &edit).unwrap();
        assert!(out.contains("# Vocabulaire de Dictia"));
        let file: VocabFile = toml::from_str(&out).unwrap();
        assert_eq!(
            file.context,
            "Dictée en français, avec une ponctuation soignée."
        );
        assert_eq!(file.terms, ["JetEngine", "n8n"]);
        assert_eq!(file.replacements.len(), 2);
        assert!(file.replacements[0].case_sensitive);
        assert_eq!(file.replacements[1], rule("presta shop", "PrestaShop"));
        assert_eq!(vocab_view(&file).replacements[1].to, "PrestaShop");
    }

    #[test]
    fn removing_all_replacements() {
        let edit = VocabEdit {
            terms: vec![],
            replacements: vec![],
        };
        let out = apply_vocab_edit(
            "[[replacements]]\nfrom = \"a\"\nto = \"b\"\n",
            &VocabFile::default(),
            &edit,
        )
        .unwrap();
        let file: VocabFile = toml::from_str(&out).unwrap();
        assert!(file.replacements.is_empty() && file.terms.is_empty());
    }
}
