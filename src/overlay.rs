//! Indicateur affiché à côté du curseur pendant une dictée.
//!
//! - **Écoute** : point rouge qui pulse et barres qui suivent le volume du
//!   micro, pour voir que la voix est bien captée.
//! - **Transcription** : trois points bleus qui ondulent pendant
//!   l'interrogation de l'API.
//!
//! Le dessin est fait ici, en mémoire, sans dépendance système ; la fenêtre
//! transparente qui l'affiche est dans `platform`.

use crate::tray::TrayState;

pub const WIDTH: u32 = 60;
pub const HEIGHT: u32 = 26;
/// Décalage par rapport au curseur (en bas à droite, comme une infobulle).
pub const OFFSET: (i32, i32) = (14, 18);
/// Cadence d'animation.
pub const FRAME_MS: u32 = 33;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Animation {
    Listening,
    Thinking,
}

impl Animation {
    pub fn for_state(state: TrayState) -> Option<Self> {
        match state {
            TrayState::Recording => Some(Self::Listening),
            TrayState::Processing => Some(Self::Thinking),
            TrayState::Idle | TrayState::Error => None,
        }
    }
}

/// Volume du micro (RMS) ramené entre 0 et 1 sur une échelle en dB :
/// -60 dBFS (silence) → 0, -15 dBFS (voix forte) → 1.
pub fn level_from_rms(rms: f32) -> f32 {
    let db = crate::audio::vad::dbfs(rms);
    ((db + 60.0) / 45.0).clamp(0.0, 1.0)
}

const BACKGROUND: [f32; 4] = [0.094, 0.094, 0.106, 0.9];
const RED: [f32; 3] = [0.973, 0.443, 0.443];
const BLUE: [f32; 3] = [0.376, 0.647, 0.980];
const WHITE: [f32; 3] = [0.95, 0.95, 0.95];

/// Taille en pixels physiques pour un facteur d'échelle d'écran.
pub fn size(scale: f32) -> (u32, u32) {
    let scale = scale.max(0.5);
    (
        (WIDTH as f32 * scale).round() as u32,
        (HEIGHT as f32 * scale).round() as u32,
    )
}

/// Image de l'indicateur au temps `t` (secondes), en BGRA prémultiplié
/// (format attendu par `UpdateLayeredWindow`), ligne par ligne de haut en
/// bas, de taille [`size`]`(scale)`. Le dessin est décrit en pixels logiques.
pub fn render(animation: Animation, t: f32, level: f32, scale: f32) -> Vec<u8> {
    let mut canvas = Canvas::new(scale);
    let (w, h) = (WIDTH as f32, HEIGHT as f32);
    canvas.fill(BACKGROUND, |x, y| {
        in_round_rect(x, y, 0.0, 0.0, w, h, h / 2.0)
    });

    let cy = h / 2.0;
    match animation {
        Animation::Listening => {
            // Onde qui s'élargit et s'efface autour du point rouge.
            let phase = (t / 1.2).fract();
            let r = 4.0 + 5.0 * phase;
            canvas.fill([RED[0], RED[1], RED[2], 0.45 * (1.0 - phase)], |x, y| {
                let d = dist(x, y, 13.0, cy);
                d <= r && d >= r - 1.6
            });
            canvas.fill([RED[0], RED[1], RED[2], 1.0], |x, y| {
                dist(x, y, 13.0, cy) <= 4.0
            });

            // Barres de volume, légèrement désynchronisées entre elles.
            let level = level.clamp(0.0, 1.0);
            for i in 0..5 {
                let wobble = 0.65 + 0.35 * (t * 9.0 + i as f32 * 1.3).sin().abs();
                let bar_h = 3.0 + 15.0 * level * wobble;
                let x0 = 26.0 + i as f32 * 6.0;
                canvas.fill([WHITE[0], WHITE[1], WHITE[2], 1.0], |x, y| {
                    in_round_rect(x, y, x0, cy - bar_h / 2.0, 3.0, bar_h, 1.5)
                });
            }
        }
        Animation::Thinking => {
            for i in 0..3 {
                let wave = (t * std::f32::consts::TAU * 1.4 - i as f32 * 0.9)
                    .sin()
                    .max(0.0);
                let (cx, dy) = (18.0 + i as f32 * 12.0, -4.0 * wave);
                let alpha = 0.7 + 0.3 * wave;
                canvas.fill([BLUE[0], BLUE[1], BLUE[2], alpha], |x, y| {
                    dist(x, y, cx, cy + dy) <= 3.4
                });
            }
        }
    }
    canvas.into_bgra()
}

/// Rectangle d'écran (gauche, haut, droite, bas) en pixels physiques.
pub type ScreenRect = (i32, i32, i32, i32);

/// Position de l'indicateur près de `anchor` (curseur), décalée de `offset`,
/// en restant entièrement dans `area` (zone de travail de l'écran) : à gauche
/// du curseur près du bord droit, au-dessus près du bas de l'écran.
pub fn place(
    anchor: (i32, i32),
    size: (u32, u32),
    offset: (i32, i32),
    area: ScreenRect,
) -> (i32, i32) {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let (left, top, right, bottom) = area;
    let mut x = anchor.0 + offset.0;
    if x + w > right {
        x = anchor.0 - offset.0 - w;
    }
    let mut y = anchor.1 + offset.1;
    if y + h > bottom {
        y = anchor.1 - offset.1 - h;
    }
    (x.min(right - w).max(left), y.min(bottom - h).max(top))
}

fn dist(x: f32, y: f32, cx: f32, cy: f32) -> f32 {
    ((x - cx).powi(2) + (y - cy).powi(2)).sqrt()
}

fn in_round_rect(x: f32, y: f32, x0: f32, y0: f32, w: f32, h: f32, r: f32) -> bool {
    if x < x0 || y < y0 || x > x0 + w || y > y0 + h {
        return false;
    }
    let r = r.min(w / 2.0).min(h / 2.0);
    let cx = x.clamp(x0 + r, x0 + w - r);
    let cy = y.clamp(y0 + r, y0 + h - r);
    dist(x, y, cx, cy) <= r
}

/// Toile RGBA (non prémultipliée) avec composition « over » et lissage par
/// suréchantillonnage 4×4.
struct Canvas {
    px: Vec<[f32; 4]>,
    width: u32,
    height: u32,
    scale: f32,
}

impl Canvas {
    fn new(scale: f32) -> Self {
        let (width, height) = size(scale);
        Self {
            px: vec![[0.0; 4]; (width * height) as usize],
            width,
            height,
            scale: width as f32 / WIDTH as f32,
        }
    }

    fn fill(&mut self, color: [f32; 4], inside: impl Fn(f32, f32) -> bool) {
        const SS: u32 = 4;
        for py in 0..self.height {
            for px in 0..self.width {
                let mut hits = 0;
                for sy in 0..SS {
                    for sx in 0..SS {
                        let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / self.scale;
                        let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / self.scale;
                        if inside(x, y) {
                            hits += 1;
                        }
                    }
                }
                if hits == 0 {
                    continue;
                }
                let a = color[3] * hits as f32 / (SS * SS) as f32;
                let dst = &mut self.px[(py * self.width + px) as usize];
                let out_a = a + dst[3] * (1.0 - a);
                for c in 0..3 {
                    dst[c] = (color[c] * a + dst[c] * dst[3] * (1.0 - a)) / out_a;
                }
                dst[3] = out_a;
            }
        }
    }

    fn into_bgra(self) -> Vec<u8> {
        let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        self.px
            .iter()
            .flat_map(|&[r, g, b, a]| [to_u8(b * a), to_u8(g * a), to_u8(r * a), to_u8(a)])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(img: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * WIDTH + x) * 4) as usize;
        img[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn stays_inside_the_screen() {
        let area = (0, 0, 1920, 1040);
        // Cas normal : en bas à droite du curseur.
        assert_eq!(place((500, 300), (60, 26), (14, 18), area), (514, 318));
        // Près du bord droit : à gauche du curseur.
        assert_eq!(place((1900, 300), (60, 26), (14, 18), area), (1826, 318));
        // Près du bas (barre des tâches) : au-dessus du curseur.
        assert_eq!(place((500, 1030), (60, 26), (14, 18), area), (514, 986));
        // Position aberrante : ramenée dans l'écran.
        assert_eq!(place((-5000, -5000), (60, 26), (14, 18), area), (0, 0));
        // Second écran à gauche (coordonnées négatives), près de son bord droit.
        assert_eq!(
            place((-30, 50), (60, 26), (14, 18), (-1920, 0, 0, 1080)),
            (-104, 68)
        );
    }

    #[test]
    fn animation_follows_dictation_state() {
        assert_eq!(
            Animation::for_state(TrayState::Recording),
            Some(Animation::Listening)
        );
        assert_eq!(
            Animation::for_state(TrayState::Processing),
            Some(Animation::Thinking)
        );
        assert_eq!(Animation::for_state(TrayState::Idle), None);
        assert_eq!(Animation::for_state(TrayState::Error), None);
    }

    #[test]
    fn level_scale() {
        assert_eq!(level_from_rms(0.0), 0.0);
        assert_eq!(level_from_rms(1.0), 1.0);
        let mid = level_from_rms(0.01); // -40 dBFS
        assert!((mid - 20.0 / 45.0).abs() < 1e-3, "{mid}");
    }

    #[test]
    fn frame_is_a_pill_with_transparent_corners() {
        let img = render(Animation::Listening, 0.0, 0.5, 1.0);
        assert_eq!(img.len(), (WIDTH * HEIGHT * 4) as usize);
        assert_eq!(pixel(&img, 0, 0)[3], 0);
        assert_eq!(pixel(&img, WIDTH - 1, HEIGHT - 1)[3], 0);
        assert!(pixel(&img, WIDTH / 2, 2)[3] > 200);
    }

    #[test]
    fn scales_for_high_dpi_screens() {
        assert_eq!(size(1.0), (WIDTH, HEIGHT));
        assert_eq!(size(1.5), (90, 39));
        let img = render(Animation::Listening, 0.0, 0.0, 1.5);
        assert_eq!(img.len(), 90 * 39 * 4);
        // Le point rouge est au même endroit, en coordonnées logiques.
        let i = ((20 * 90 + 20) * 4) as usize; // (13, 13) × 1,5 ≈ (20, 20)
        assert_eq!(img[i + 3], 255);
        assert!(img[i + 2] > 200);
    }

    #[test]
    fn premultiplied_alpha_is_consistent() {
        let img = render(Animation::Thinking, 0.3, 0.0, 1.0);
        for p in img.as_chunks::<4>().0 {
            assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "{p:?}");
        }
    }

    #[test]
    fn listening_dot_is_red() {
        let [b, g, r, a] = pixel(&render(Animation::Listening, 0.0, 0.0, 1.0), 13, 13);
        assert_eq!(a, 255);
        assert!(r > 200 && g < 150 && b < 150, "{:?}", (r, g, b));
    }

    #[test]
    fn bars_grow_with_voice_level() {
        let lit = |img: &[u8]| (0..HEIGHT).filter(|&y| pixel(img, 27, y)[0] > 200).count();
        let quiet = render(Animation::Listening, 0.0, 0.0, 1.0);
        let loud = render(Animation::Listening, 0.0, 1.0, 1.0);
        assert!(
            lit(&loud) > lit(&quiet) + 6,
            "{} vs {}",
            lit(&loud),
            lit(&quiet)
        );
    }

    #[test]
    fn the_two_animations_look_different_and_move() {
        let listening = render(Animation::Listening, 0.2, 0.5, 1.0);
        let thinking = render(Animation::Thinking, 0.2, 0.5, 1.0);
        assert_ne!(listening, thinking);
        assert_ne!(thinking, render(Animation::Thinking, 0.5, 0.5, 1.0));
        // Transcription : bleu, pas de rouge.
        let [b, g, r, _] = pixel(&render(Animation::Thinking, 0.0, 0.0, 1.0), 30, 13);
        assert!(b > 150 && b > g && g > r, "{:?}", (r, g, b));
    }
}
