//! Détection d'activité vocale par énergie (RMS) : on retire les silences de
//! début et de fin pour envoyer moins d'audio et obtenir le texte plus vite.

use std::ops::Range;

use crate::config::AudioConfig;

/// Taille des blocs analysés.
pub const FRAME_MS: u32 = 20;

/// Racine de la moyenne des carrés.
pub fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f64 = frame.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    (sum / frame.len() as f64).sqrt() as f32
}

/// Niveau en dBFS (0 dBFS = pleine échelle), plancher à -120.
pub fn dbfs(rms: f32) -> f32 {
    if rms <= 1e-6 {
        -120.0
    } else {
        20.0 * rms.log10()
    }
}

/// Plage d'échantillons contenant la parole, marges comprises.
/// `None` si la parole détectée est plus courte que `min_speech_ms`.
pub fn speech_bounds(samples: &[f32], rate: u32, cfg: &AudioConfig) -> Option<Range<usize>> {
    let frame_len = (rate * FRAME_MS / 1000).max(1) as usize;
    let voiced: Vec<usize> = samples
        .chunks(frame_len)
        .enumerate()
        .filter(|(_, f)| dbfs(rms(f)) >= cfg.silence_threshold_db)
        .map(|(i, _)| i)
        .collect();
    let (&first, &last) = (voiced.first()?, voiced.last()?);
    if (voiced.len() as u32) * FRAME_MS < cfg.min_speech_ms {
        return None;
    }
    let pad = (rate as usize * cfg.padding_ms as usize) / 1000;
    let start = (first * frame_len).saturating_sub(pad);
    let end = ((last + 1) * frame_len + pad).min(samples.len());
    Some(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    fn cfg() -> AudioConfig {
        AudioConfig {
            silence_threshold_db: -40.0,
            padding_ms: 100,
            min_speech_ms: 100,
            release_tail_ms: 0,
        }
    }

    fn tone(ms: u32, amp: f32) -> Vec<f32> {
        let n = (RATE * ms / 1000) as usize;
        (0..n)
            .map(|i| amp * (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / RATE as f32).sin())
            .collect()
    }

    #[test]
    fn rms_of_known_signals() {
        assert_eq!(rms(&[]), 0.0);
        assert!((rms(&[0.5, -0.5, 0.5, -0.5]) - 0.5).abs() < 1e-6);
        // Sinusoïde d'amplitude A : RMS = A / √2.
        let r = rms(&tone(100, 1.0));
        assert!((r - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3, "{r}");
    }

    #[test]
    fn dbfs_scale() {
        assert!((dbfs(1.0)).abs() < 1e-6);
        assert!((dbfs(0.1) + 20.0).abs() < 1e-4);
        assert_eq!(dbfs(0.0), -120.0);
    }

    #[test]
    fn trims_leading_and_trailing_silence_with_padding() {
        let mut s = vec![0.0; (RATE / 2) as usize]; // 500 ms de silence
        let speech_start = s.len();
        s.extend(tone(300, 0.3));
        let speech_end = s.len();
        s.extend(vec![0.0; RATE as usize]); // 1 s de silence

        let r = speech_bounds(&s, RATE, &cfg()).unwrap();
        let pad = (RATE / 10) as usize;
        assert_eq!(r.start, speech_start - pad);
        assert_eq!(r.end, speech_end + pad);
    }

    #[test]
    fn padding_is_clamped_to_buffer() {
        let s = tone(200, 0.3);
        assert_eq!(speech_bounds(&s, RATE, &cfg()).unwrap(), 0..s.len());
    }

    #[test]
    fn silence_and_low_noise_are_rejected() {
        assert_eq!(speech_bounds(&[], RATE, &cfg()), None);
        assert_eq!(speech_bounds(&vec![0.0; 16_000], RATE, &cfg()), None);
        // Bruit à -60 dBFS, sous le seuil de -40 dBFS.
        assert_eq!(speech_bounds(&tone(1000, 0.0014), RATE, &cfg()), None);
    }

    #[test]
    fn short_clicks_are_rejected() {
        let mut s = vec![0.0; 8_000];
        s.extend(tone(40, 0.5)); // 40 ms < 100 ms minimum
        s.extend(vec![0.0; 8_000]);
        assert_eq!(speech_bounds(&s, RATE, &cfg()), None);
    }
}
