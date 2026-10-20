//! Journalisation des latences de chaque dictée.
//!
//! Les durées « depuis l'arrêt » partent du 2e appui du raccourci : c'est ce
//! que l'utilisateur perçoit comme l'attente du texte.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const METRICS_FILE: &str = "latences.csv";

pub const CSV_HEADER: &str = "horodatage_unix,backend,enregistrement_ms,parole_ms,flac_octets,\
encodage_ms,upload_ms,premier_token_ms,fin_ms,frappe_ms,total_ms,caracteres";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Latencies {
    pub backend: &'static str,
    /// Durée de l'enregistrement brut.
    pub recording: Duration,
    /// Durée de l'audio envoyé, après retrait des silences.
    pub speech: Duration,
    pub flac_bytes: usize,
    /// Retrait des silences + encodage FLAC.
    pub encode: Duration,
    /// Envoi de la requête jusqu'à réception des en-têtes de réponse.
    pub upload: Option<Duration>,
    /// Depuis l'arrêt, jusqu'au premier fragment de texte.
    pub first_token: Option<Duration>,
    /// Depuis l'arrêt, jusqu'au texte complet.
    pub done: Duration,
    /// Durée de la frappe (focus compris).
    pub inject: Duration,
    /// Depuis l'arrêt, jusqu'à la fin de la frappe.
    pub total: Duration,
    pub chars: usize,
}

fn ms(d: Duration) -> u128 {
    d.as_millis()
}

fn opt_ms(d: Option<Duration>) -> String {
    d.map_or_else(|| "-".to_owned(), |d| d.as_millis().to_string())
}

impl Latencies {
    /// Ligne lisible pour le journal.
    pub fn summary(&self) -> String {
        format!(
            "latences : encodage {} ms, upload {} ms, premier token {} ms, fin {} ms, \
             frappe {} ms, total {} ms (audio {} ms → {} ms de parole, FLAC {} Ko, {} car.)",
            ms(self.encode),
            opt_ms(self.upload),
            opt_ms(self.first_token),
            ms(self.done),
            ms(self.inject),
            ms(self.total),
            ms(self.recording),
            ms(self.speech),
            self.flac_bytes.div_ceil(1024),
            self.chars,
        )
    }

    pub fn csv_row(&self, timestamp: u64) -> String {
        format!(
            "{timestamp},{},{},{},{},{},{},{},{},{},{},{}",
            self.backend,
            ms(self.recording),
            ms(self.speech),
            self.flac_bytes,
            ms(self.encode),
            opt_ms(self.upload),
            opt_ms(self.first_token),
            ms(self.done),
            ms(self.inject),
            ms(self.total),
            self.chars,
        )
    }

    /// Ajoute une ligne à `path` (en-tête écrit à la création).
    pub fn append_csv(&self, path: &Path) -> std::io::Result<()> {
        let is_new = !path.exists();
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        if is_new {
            writeln!(file, "{CSV_HEADER}")?;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        writeln!(file, "{}", self.csv_row(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Latencies {
        Latencies {
            backend: "standard",
            recording: Duration::from_millis(4_200),
            speech: Duration::from_millis(3_100),
            flac_bytes: 61_000,
            encode: Duration::from_millis(12),
            upload: Some(Duration::from_millis(180)),
            first_token: None,
            done: Duration::from_millis(640),
            inject: Duration::from_millis(25),
            total: Duration::from_millis(665),
            chars: 48,
        }
    }

    #[test]
    fn csv_row_matches_header() {
        let row = sample().csv_row(1_700_000_000);
        assert_eq!(
            row,
            "1700000000,standard,4200,3100,61000,12,180,-,640,25,665,48"
        );
        assert_eq!(row.split(',').count(), CSV_HEADER.split(',').count());
    }

    #[test]
    fn summary_is_readable() {
        let s = sample().summary();
        assert!(s.contains("upload 180 ms"), "{s}");
        assert!(s.contains("premier token - ms"), "{s}");
        assert!(s.contains("FLAC 60 Ko"), "{s}");
    }

    #[test]
    fn append_writes_header_once() {
        let path = std::env::temp_dir().join(format!("dictia-metrics-{}.csv", std::process::id()));
        let _ = std::fs::remove_file(&path);
        sample().append_csv(&path).unwrap();
        sample().append_csv(&path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], CSV_HEADER);
        std::fs::remove_file(&path).unwrap();
    }
}
