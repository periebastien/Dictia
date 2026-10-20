//! Audio : capture micro, conversion mono 16 kHz, suppression des silences,
//! encodage FLAC en mémoire.

#[cfg(windows)]
pub mod capture;
pub mod flac;
pub mod resample;
pub mod vad;

/// Fréquence envoyée à l'API : suffisante pour la voix, upload minimal.
pub const TARGET_RATE: u32 = 16_000;

/// Durée d'un nombre d'échantillons à `rate` Hz.
pub fn duration_of(samples: usize, rate: u32) -> std::time::Duration {
    std::time::Duration::from_secs_f64(samples as f64 / f64::from(rate))
}
