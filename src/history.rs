//! Historique des dictées : une ligne JSON par dictée dans
//! `%APPDATA%\Dictia\historique.jsonl`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const HISTORY_FILE: &str = "historique.jsonl";
/// Nombre de dictées conservées en mémoire et affichées.
pub const MAX_ENTRIES: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Horodatage Unix en millisecondes (sert aussi d'identifiant).
    pub at_ms: u64,
    pub text: String,
    /// Application où le texte a été tapé (« Outlook », « VS Code »…).
    pub app: String,
    /// Durée de l'enregistrement.
    pub duration_ms: u64,
    /// « Bascule » ou « Maintien ».
    pub mode: String,
}

/// Problème survenu pendant une dictée, affiché dans la fenêtre pour ne pas
/// obliger à ouvrir `dictia.log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    /// Horodatage Unix en millisecondes.
    pub at_ms: u64,
    pub text: String,
}

/// Ajoute une dictée à la fin du fichier.
pub fn append(path: &Path, entry: &Entry) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(entry)?)?;
    Ok(())
}

/// Les `MAX_ENTRIES` dernières dictées, de la plus récente à la plus
/// ancienne. Les lignes illisibles sont ignorées.
pub fn load(path: &Path) -> Vec<Entry> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = text
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str(l).ok())
        .take(MAX_ENTRIES)
        .collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.at_ms));
    entries
}

/// Nom lisible d'une application à partir de son exécutable.
pub fn friendly_app_name(exe: &str) -> String {
    let stem = exe
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(exe)
        .trim_end_matches(".exe")
        .trim_end_matches(".EXE");
    let known = match stem.to_lowercase().as_str() {
        "outlook" | "olk" => "Outlook",
        "winword" => "Word",
        "excel" => "Excel",
        "powerpnt" => "PowerPoint",
        "onenote" => "OneNote",
        "code" => "VS Code",
        "chrome" => "Chrome",
        "msedge" => "Edge",
        "firefox" => "Firefox",
        "brave" => "Brave",
        "slack" => "Slack",
        "teams" | "ms-teams" => "Teams",
        "notepad" => "Bloc-notes",
        "notepad++" => "Notepad++",
        "explorer" => "Explorateur",
        "windowsterminal" => "Terminal",
        "cmd" => "Invite de commandes",
        "powershell" | "pwsh" => "PowerShell",
        "todoist" => "Todoist",
        "notion" => "Notion",
        "obsidian" => "Obsidian",
        "discord" => "Discord",
        "whatsapp" => "WhatsApp",
        "thunderbird" => "Thunderbird",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_owned();
    }
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(at_ms: u64, text: &str) -> Entry {
        Entry {
            at_ms,
            text: text.into(),
            app: "Outlook".into(),
            duration_ms: 1_000,
            mode: "Bascule".into(),
        }
    }

    #[test]
    fn append_then_load_most_recent_first() {
        let path = std::env::temp_dir().join(format!("dictia-hist-{}.jsonl", std::process::id()));
        let _ = fs::remove_file(&path);
        assert!(load(&path).is_empty());
        append(&path, &entry(1, "première")).unwrap();
        append(&path, &entry(2, "deuxième, avec \"guillemets\"\net retour")).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"ligne abimee\n")
            .unwrap();
        let loaded = load(&path);
        assert_eq!(
            loaded,
            [
                entry(2, "deuxième, avec \"guillemets\"\net retour"),
                entry(1, "première")
            ]
        );
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn friendly_names() {
        assert_eq!(
            friendly_app_name(r"C:\Program Files\Microsoft Office\OUTLOOK.EXE"),
            "Outlook"
        );
        assert_eq!(friendly_app_name("Code.exe"), "VS Code");
        assert_eq!(friendly_app_name("monappli.exe"), "Monappli");
        assert_eq!(friendly_app_name(""), "");
    }
}
