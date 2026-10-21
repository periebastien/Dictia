//! Statistiques de latence calculées à partir de `latences.csv`.
//!
//! La mesure principale est l'**attente** : du moment où vous arrêtez la
//! dictée (relâchement ou 2e appui) à la fin de la frappe du texte.

use serde::Serialize;

/// Dictées prises en compte par moteur (les plus récentes).
pub const WINDOW: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EngineStats {
    /// « Standard » / « Realtime » (OpenAI), « Repli » (realtime retombé en
    /// standard), « Mistral » ou « Local ».
    pub engine: &'static str,
    pub count: usize,
    pub wait_mean_ms: u64,
    pub wait_median_ms: u64,
    pub wait_p90_ms: u64,
    /// Médiane du temps jusqu'au premier fragment de texte.
    pub first_text_median_ms: Option<u64>,
    /// Médiane de la durée de parole.
    pub speech_median_ms: u64,
}

struct Row {
    engine: &'static str,
    speech_ms: u64,
    first_token_ms: Option<u64>,
    total_ms: u64,
}

fn parse_row(line: &str) -> Option<Row> {
    let cols: Vec<&str> = line.split(',').collect();
    if cols.len() < 12 {
        return None;
    }
    let num = |i: usize| cols[i].trim().parse::<u64>().ok();
    let engine = match cols[1].trim() {
        "standard" => "Standard",
        "realtime" => "Realtime",
        "realtime>standard" => "Repli",
        "mistral" => "Mistral",
        "local" => "Local",
        _ => return None,
    };
    Some(Row {
        engine,
        speech_ms: num(3)?,
        first_token_ms: num(7),
        total_ms: num(10)?,
    })
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

fn median(values: &mut [u64]) -> u64 {
    values.sort_unstable();
    if values.is_empty() {
        0
    } else if values.len() % 2 == 1 {
        values[values.len() / 2]
    } else {
        let (a, b) = (values[values.len() / 2 - 1], values[values.len() / 2]);
        (a + b) / 2
    }
}

/// Statistiques par moteur, dans l'ordre Standard, Realtime, Repli
/// (seuls les moteurs utilisés apparaissent).
pub fn compute(csv: &str) -> Vec<EngineStats> {
    let rows: Vec<Row> = csv.lines().skip(1).filter_map(parse_row).collect();
    ["Standard", "Realtime", "Repli", "Mistral", "Local"]
        .into_iter()
        .filter_map(|engine| {
            let mut recent: Vec<&Row> = rows
                .iter()
                .rev()
                .filter(|r| r.engine == engine)
                .take(WINDOW)
                .collect();
            if recent.is_empty() {
                return None;
            }
            recent.reverse();
            let mut waits: Vec<u64> = recent.iter().map(|r| r.total_ms).collect();
            waits.sort_unstable();
            let mut firsts: Vec<u64> = recent.iter().filter_map(|r| r.first_token_ms).collect();
            let mut speech: Vec<u64> = recent.iter().map(|r| r.speech_ms).collect();
            Some(EngineStats {
                engine,
                count: recent.len(),
                wait_mean_ms: waits.iter().sum::<u64>() / waits.len() as u64,
                wait_median_ms: median(&mut waits.clone()),
                wait_p90_ms: percentile(&waits, 0.9),
                first_text_median_ms: (!firsts.is_empty()).then(|| median(&mut firsts)),
                speech_median_ms: median(&mut speech),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::CSV_HEADER;

    fn csv(rows: &[&str]) -> String {
        let mut s = format!("{CSV_HEADER}\n");
        for r in rows {
            s.push_str(r);
            s.push('\n');
        }
        s
    }

    #[test]
    fn computes_per_engine_statistics() {
        let data = csv(&[
            "1,standard,4000,3000,50000,10,200,600,700,30,730,40",
            "2,standard,3000,2000,40000,10,200,500,600,30,630,30",
            "3,standard,5000,4000,60000,10,200,700,900,30,930,50",
            "4,realtime,4000,3000,0,0,20,0,150,30,180,40",
            "5,realtime,4000,3000,0,0,20,-,250,30,280,40",
            "6,realtime>standard,4000,3000,50000,10,200,600,1200,30,1230,40",
            "ligne abîmée",
        ]);
        let stats = compute(&data);
        assert_eq!(stats.len(), 3);
        let std = &stats[0];
        assert_eq!((std.engine, std.count), ("Standard", 3));
        assert_eq!(std.wait_mean_ms, 763);
        assert_eq!(std.wait_median_ms, 730);
        assert_eq!(std.wait_p90_ms, 930);
        assert_eq!(std.first_text_median_ms, Some(600));
        assert_eq!(std.speech_median_ms, 3000);
        let rt = &stats[1];
        assert_eq!(
            (rt.engine, rt.count, rt.wait_median_ms),
            ("Realtime", 2, 230)
        );
        assert_eq!(rt.first_text_median_ms, Some(0));
        assert_eq!(stats[2].engine, "Repli");
    }

    #[test]
    fn empty_or_missing_file_gives_nothing() {
        assert!(compute("").is_empty());
        assert!(compute(&csv(&[])).is_empty());
    }

    #[test]
    fn keeps_only_the_most_recent_dictations() {
        let rows: Vec<String> = (0..WINDOW + 50)
            .map(|i| {
                format!(
                    "{i},standard,1,1,1,1,1,1,1,1,{},1",
                    if i < 50 { 10_000 } else { 100 }
                )
            })
            .collect();
        let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
        let stats = compute(&csv(&refs));
        assert_eq!(stats[0].count, WINDOW);
        assert_eq!(
            stats[0].wait_p90_ms, 100,
            "les anciennes dictées lentes sont exclues"
        );
    }
}
