//! Conversion en mono 16 kHz, faite au fil de la capture.
//!
//! Le travail est effectué dans le callback audio pendant que l'utilisateur
//! parle : au 2e appui du raccourci, l'audio est déjà au bon format et
//! aucune conversion ne s'ajoute à la latence.

/// Moyenne des canaux de chaque trame d'un signal entrelacé.
pub fn downmix(interleaved: &[f32], channels: usize, mut sink: impl FnMut(f32)) {
    if channels <= 1 {
        interleaved.iter().copied().for_each(sink);
        return;
    }
    let scale = 1.0 / channels as f32;
    for frame in interleaved.chunks_exact(channels) {
        sink(frame.iter().sum::<f32>() * scale);
    }
}

/// Rééchantillonneur en flux.
///
/// - Sous-échantillonnage (cas courant, 48 kHz → 16 kHz) : moyenne pondérée
///   des échantillons couverts par chaque échantillon de sortie, ce qui sert
///   aussi de filtre anti-repliement, simple et suffisant pour la voix.
/// - Sur-échantillonnage (micro à 8 kHz) : interpolation linéaire.
#[derive(Debug, Clone)]
pub struct StreamResampler {
    /// Échantillons d'entrée par échantillon de sortie.
    ratio: f64,
    /// Position (en échantillons d'entrée) du début de l'échantillon courant.
    consumed: f64,
    /// Prochaine frontière de sortie, en échantillons d'entrée.
    boundary: f64,
    acc: f64,
    weight: f64,
    prev: Option<f32>,
}

impl StreamResampler {
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        let ratio = f64::from(from_rate) / f64::from(to_rate);
        Self {
            ratio,
            consumed: 0.0,
            boundary: if ratio >= 1.0 { ratio } else { 0.0 },
            acc: 0.0,
            weight: 0.0,
            prev: None,
        }
    }

    pub fn push(&mut self, x: f32, out: &mut Vec<f32>) {
        if (self.ratio - 1.0).abs() < f64::EPSILON {
            out.push(x);
        } else if self.ratio > 1.0 {
            self.push_down(f64::from(x), out);
        } else {
            self.push_up(x, out);
        }
    }

    fn push_down(&mut self, x: f64, out: &mut Vec<f32>) {
        let end = self.consumed + 1.0;
        if end < self.boundary {
            self.acc += x;
            self.weight += 1.0;
        } else {
            // L'échantillon chevauche la frontière : sa part gauche termine
            // la sortie courante, sa part droite commence la suivante.
            let left = self.boundary - self.consumed;
            self.acc += x * left;
            self.weight += left;
            out.push((self.acc / self.weight) as f32);
            let right = 1.0 - left;
            self.acc = x * right;
            self.weight = right;
            self.boundary += self.ratio;
        }
        self.consumed = end;
    }

    fn push_up(&mut self, x: f32, out: &mut Vec<f32>) {
        // Les sorties sont émises aux instants k·ratio ≤ position de `x`.
        let pos = self.consumed;
        match self.prev {
            None => {
                out.push(x);
                self.boundary = self.ratio;
            }
            Some(prev) => {
                while self.boundary <= pos {
                    let t = (self.boundary - (pos - 1.0)) as f32;
                    out.push(prev + (x - prev) * t);
                    self.boundary += self.ratio;
                }
            }
        }
        self.prev = Some(x);
        self.consumed += 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &[f32], from: u32, to: u32) -> Vec<f32> {
        let mut r = StreamResampler::new(from, to);
        let mut out = Vec::new();
        for &x in input {
            r.push(x, &mut out);
        }
        out
    }

    #[test]
    fn downmix_stereo_and_mono() {
        let mut out = Vec::new();
        downmix(&[1.0, 0.0, 0.5, 0.5], 2, |x| out.push(x));
        assert_eq!(out, [0.5, 0.5]);
        out.clear();
        downmix(&[0.25, -0.25], 1, |x| out.push(x));
        assert_eq!(out, [0.25, -0.25]);
    }

    #[test]
    fn same_rate_is_identity() {
        assert_eq!(run(&[0.1, 0.2, 0.3], 16_000, 16_000), [0.1, 0.2, 0.3]);
    }

    #[test]
    fn integer_ratio_averages_blocks() {
        let out = run(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 48_000, 16_000);
        assert_eq!(out, [2.0, 5.0]);
    }

    #[test]
    fn fractional_ratio_keeps_length_and_level() {
        let input = vec![0.5f32; 44_100];
        let out = run(&input, 44_100, 16_000);
        assert!((out.len() as i64 - 16_000).abs() <= 1, "{}", out.len());
        assert!(out.iter().all(|&x| (x - 0.5).abs() < 1e-5));
    }

    #[test]
    fn upsampling_interpolates() {
        let out = run(&[0.0, 1.0, 0.0], 8_000, 16_000);
        assert_eq!(out, [0.0, 0.5, 1.0, 0.5, 0.0]);
    }

    #[test]
    fn chunked_input_gives_same_output() {
        let input: Vec<f32> = (0..4_800).map(|i| (i as f32 * 0.01).sin()).collect();
        let whole = run(&input, 48_000, 16_000);
        let mut r = StreamResampler::new(48_000, 16_000);
        let mut chunked = Vec::new();
        for chunk in input.chunks(441) {
            for &x in chunk {
                r.push(x, &mut chunked);
            }
        }
        assert_eq!(whole, chunked);
        assert_eq!(whole.len(), 1_600);
    }
}
