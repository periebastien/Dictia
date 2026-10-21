//! Statistiques de latence par modèle, calculées à partir de `latences.csv`.
//!
//! L'attente mesurée va de la fin de l'enregistrement au texte entièrement
//! tapé, **sans** le temps passé à attendre le relâchement de Ctrl/Alt/Win
//! (qui dépend de l'utilisateur, pas du modèle).
//!
//! Pour comparer des modèles sur des dictées de longueurs différentes, on
//! décompose l'attente en une part fixe (réseau, mise en route) et une part
//! proportionnelle à la durée de parole envoyée :
//!
//! `attente ≈ fixe + coût × secondes de parole`
//!
//! La droite est ajustée par la méthode de Theil-Sen (médiane des pentes),
//! insensible aux quelques dictées anormalement lentes. Le chiffre de
//! référence est l'attente prévue pour une dictée de 10 secondes.

use serde::Serialize;

/// Dictées prises en compte par modèle (les plus récentes).
pub const WINDOW: usize = 200;
/// Durée de parole de référence pour comparer les modèles.
pub const REFERENCE_SPEECH_MS: u64 = 10_000;
/// Minimum de dictées, et d'écart de durée, pour ajuster la droite.
const MIN_FIT_COUNT: usize = 5;
const MIN_FIT_SPREAD_MS: u64 = 2_000;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelStats {
    /// « Mistral · voxtral-mini-latest », « OpenAI Realtime · … »…
    pub label: String,
    pub count: usize,
    /// Attente prévue pour 10 s de parole (chiffre comparable).
    pub reference_ms: u64,
    /// `true` si `reference_ms` vient de la droite ajustée ; sinon c'est la
    /// médiane brute (trop peu de dictées ou durées trop semblables).
    pub fitted: bool,
    /// Part fixe de l'attente.
    pub fixed_ms: u64,
    /// Coût par seconde de parole.
    pub per_second_ms: u64,
    pub wait_median_ms: u64,
    pub wait_p90_ms: u64,
    pub speech_median_ms: u64,
    /// Médiane du temps jusqu'au premier fragment de texte.
    pub first_text_median_ms: Option<u64>,
}

struct Row {
    label: String,
    speech_ms: u64,
    first_token_ms: Option<u64>,
    wait_ms: u64,
}

fn provider_label(backend: &str, provider: Option<&str>, model: Option<&str>) -> String {
    let model = model.filter(|m| !m.is_empty());
    let head = match (backend, provider) {
        ("realtime", _) => "OpenAI Realtime",
        ("realtime>standard", _) => "Repli Realtime → standard",
        (_, Some(p)) if !p.is_empty() => p,
        ("mistral", None) => "Mistral",
        ("local", None) => "Local",
        _ => "OpenAI",
    };
    match model {
        Some(m) => format!("{head} · {m}"),
        None => format!("{head} · modèle non noté"),
    }
}

fn parse_row(line: &str) -> Option<Row> {
    let cols: Vec<&str> = line.split(',').map(str::trim).collect();
    if cols.len() < 12 {
        return None;
    }
    let num = |i: usize| cols.get(i).and_then(|v| v.parse::<u64>().ok());
    let total = num(10)?;
    // Colonnes ajoutées ensuite : fournisseur, modèle, attente des touches.
    let keys_wait = num(14).unwrap_or(0);
    Some(Row {
        label: provider_label(cols[1], cols.get(12).copied(), cols.get(13).copied()),
        speech_ms: num(3)?,
        first_token_ms: num(7),
        wait_ms: total.saturating_sub(keys_wait),
    })
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

fn median_f(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

fn median(values: &[u64]) -> u64 {
    let mut v: Vec<f64> = values.iter().map(|&x| x as f64).collect();
    median_f(&mut v).round() as u64
}

/// Droite de Theil-Sen `y = a + b·x` : pente médiane des paires, puis
/// ordonnée médiane. Renvoie `None` si les `x` sont trop resserrés.
pub fn theil_sen(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let mut slopes = Vec::with_capacity(points.len() * points.len() / 2);
    for (i, &(x1, y1)) in points.iter().enumerate() {
        for &(x2, y2) in &points[i + 1..] {
            if (x2 - x1).abs() > f64::EPSILON {
                slopes.push((y2 - y1) / (x2 - x1));
            }
        }
    }
    if slopes.is_empty() {
        return None;
    }
    let b = median_f(&mut slopes);
    let mut intercepts: Vec<f64> = points.iter().map(|&(x, y)| y - b * x).collect();
    Some((median_f(&mut intercepts), b))
}

/// Statistiques par modèle, du plus rapide au plus lent (à 10 s de parole).
pub fn compute(csv: &str) -> Vec<ModelStats> {
    let rows: Vec<Row> = csv.lines().skip(1).filter_map(parse_row).collect();
    let mut labels: Vec<&str> = Vec::new();
    for r in &rows {
        if !labels.contains(&r.label.as_str()) {
            labels.push(&r.label);
        }
    }
    let mut stats: Vec<ModelStats> = labels
        .into_iter()
        .map(|label| {
            let mut recent: Vec<&Row> = rows
                .iter()
                .rev()
                .filter(|r| r.label == label)
                .take(WINDOW)
                .collect();
            recent.reverse();
            let mut waits: Vec<u64> = recent.iter().map(|r| r.wait_ms).collect();
            waits.sort_unstable();
            let speech: Vec<u64> = recent.iter().map(|r| r.speech_ms).collect();
            let firsts: Vec<u64> = recent.iter().filter_map(|r| r.first_token_ms).collect();

            let spread = speech.iter().max().unwrap_or(&0) - speech.iter().min().unwrap_or(&0);
            let fit = (recent.len() >= MIN_FIT_COUNT && spread >= MIN_FIT_SPREAD_MS)
                .then(|| {
                    let points: Vec<(f64, f64)> = recent
                        .iter()
                        .map(|r| (r.speech_ms as f64 / 1000.0, r.wait_ms as f64))
                        .collect();
                    theil_sen(&points)
                })
                .flatten();
            let wait_median = median(&waits);
            let (reference_ms, fixed_ms, per_second_ms, fitted) = match fit {
                Some((a, b)) => {
                    let (a, b) = (a.max(0.0), b.max(0.0));
                    let at_ref = a + b * REFERENCE_SPEECH_MS as f64 / 1000.0;
                    (
                        at_ref.round() as u64,
                        a.round() as u64,
                        b.round() as u64,
                        true,
                    )
                }
                None => (wait_median, wait_median, 0, false),
            };
            ModelStats {
                label: label.to_owned(),
                count: recent.len(),
                reference_ms,
                fitted,
                fixed_ms,
                per_second_ms,
                wait_median_ms: wait_median,
                wait_p90_ms: percentile(&waits, 0.9),
                speech_median_ms: median(&speech),
                first_text_median_ms: (!firsts.is_empty()).then(|| median(&firsts)),
            }
        })
        .collect();
    stats.sort_by_key(|s| (!s.fitted, s.reference_ms));
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::CSV_HEADER;

    fn csv(rows: &[String]) -> String {
        let mut s = format!("{CSV_HEADER}\n");
        for r in rows {
            s.push_str(r);
            s.push('\n');
        }
        s
    }

    /// Ligne au format actuel : attente totale, parole, attente des touches.
    fn row(
        backend: &str,
        provider: &str,
        model: &str,
        speech: u64,
        total: u64,
        keys: u64,
    ) -> String {
        format!("1,{backend},{speech},{speech},0,0,0,100,0,0,{total},10,{provider},{model},{keys}")
    }

    #[test]
    fn theil_sen_recovers_a_line_despite_an_outlier() {
        let mut points: Vec<(f64, f64)> = (1..=9)
            .map(|x| (x as f64, 300.0 + 50.0 * x as f64))
            .collect();
        points.push((5.0, 5_000.0)); // dictée anormalement lente
        let (a, b) = theil_sen(&points).unwrap();
        assert!((a - 300.0).abs() < 1.0, "{a}");
        assert!((b - 50.0).abs() < 1.0, "{b}");
        assert!(theil_sen(&[(2.0, 1.0), (2.0, 3.0)]).is_none());
    }

    #[test]
    fn compares_models_at_equal_speech_length() {
        let mut rows = Vec::new();
        // Modèle A : 400 ms fixes + 60 ms par seconde de parole.
        for s in [2, 4, 6, 8, 12, 15] {
            rows.push(row(
                "mistral",
                "Mistral",
                "voxtral-mini-latest",
                s * 1000,
                400 + 60 * s,
                0,
            ));
        }
        // Modèle B : 250 ms fixes + 120 ms/s, mais dictées plus courtes :
        // sa médiane brute paraît meilleure alors qu'il est plus lent à 10 s.
        for s in [1, 2, 3, 4, 5] {
            rows.push(row(
                "standard",
                "OpenAI",
                "gpt-4o-transcribe",
                s * 1000,
                250 + 120 * s,
                0,
            ));
        }
        let stats = compute(&csv(&rows));
        assert_eq!(stats.len(), 2);
        let a = &stats[0];
        assert_eq!(a.label, "Mistral · voxtral-mini-latest");
        assert!(a.fitted);
        assert_eq!(
            (a.fixed_ms, a.per_second_ms, a.reference_ms),
            (400, 60, 1_000)
        );
        let b = &stats[1];
        assert_eq!(b.label, "OpenAI · gpt-4o-transcribe");
        assert_eq!(b.reference_ms, 1_450);
        assert!(
            b.wait_median_ms < a.wait_median_ms,
            "piège de la médiane brute"
        );
    }

    #[test]
    fn excludes_time_spent_holding_modifier_keys() {
        let rows: Vec<String> = (0..3)
            .map(|_| row("standard", "OpenAI", "m", 3_000, 1_700, 1_200))
            .collect();
        assert_eq!(compute(&csv(&rows))[0].wait_median_ms, 500);
    }

    #[test]
    fn separates_models_and_engines() {
        let rows = vec![
            row("standard", "OpenAI", "gpt-4o-transcribe", 3_000, 900, 0),
            row("standard", "OpenAI", "whisper-1", 3_000, 900, 0),
            row("realtime", "OpenAI", "gpt-live-transcribe", 3_000, 300, 0),
            row(
                "realtime>standard",
                "OpenAI",
                "gpt-4o-transcribe",
                3_000,
                2_000,
                0,
            ),
            row(
                "local",
                "Local",
                "mistralai/Voxtral-Mini-4B-Realtime-2602",
                3_000,
                200,
                0,
            ),
        ];
        let labels: Vec<String> = compute(&csv(&rows)).into_iter().map(|s| s.label).collect();
        for expected in [
            "OpenAI · gpt-4o-transcribe",
            "OpenAI · whisper-1",
            "OpenAI Realtime · gpt-live-transcribe",
            "Repli Realtime → standard · gpt-4o-transcribe",
            "Local · mistralai/Voxtral-Mini-4B-Realtime-2602",
        ] {
            assert!(
                labels.iter().any(|l| l == expected),
                "{expected} absent de {labels:?}"
            );
        }
    }

    #[test]
    fn reads_old_rows_without_model() {
        let old = "1,standard,4000,3000,50000,10,200,600,700,30,730,40".to_owned();
        let stats = compute(&csv(&[old]));
        assert_eq!(stats[0].label, "OpenAI · modèle non noté");
        assert!(!stats[0].fitted);
        assert_eq!(stats[0].reference_ms, 730);
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
                row(
                    "standard",
                    "OpenAI",
                    "m",
                    3_000,
                    if i < 50 { 10_000 } else { 100 },
                    0,
                )
            })
            .collect();
        let stats = compute(&csv(&rows));
        assert_eq!(stats[0].count, WINDOW);
        assert_eq!(
            stats[0].wait_p90_ms, 100,
            "les anciennes dictées lentes sont exclues"
        );
    }
}
