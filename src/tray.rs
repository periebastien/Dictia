//! Icône de la zone de notification : état courant et menu.

/// État affiché par l'icône.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Recording,
    Processing,
    Error,
}

impl TrayState {
    pub const ALL: [TrayState; 4] = [Self::Idle, Self::Recording, Self::Processing, Self::Error];

    fn index(self) -> usize {
        self as usize
    }

    pub fn tooltip(self, shortcut: &str) -> String {
        match self {
            Self::Idle => format!("Dictia — prêt ({shortcut})"),
            Self::Recording => "Dictia — enregistrement…".to_owned(),
            Self::Processing => "Dictia — transcription…".to_owned(),
            Self::Error => "Dictia — erreur, voir dictia.log".to_owned(),
        }
    }

    fn color(self) -> [u8; 3] {
        match self {
            Self::Idle => [0x3B, 0x6E, 0xD8],       // bleu
            Self::Recording => [0xE5, 0x39, 0x35],  // rouge
            Self::Processing => [0xF5, 0x8F, 0x00], // orange
            Self::Error => [0x75, 0x75, 0x75],      // gris
        }
    }
}

pub const ICON_SIZE: u32 = 32;

/// Pastille colorée avec un micro blanc, en RGBA, lissée par
/// suréchantillonnage 4×4.
pub fn icon_rgba(state: TrayState) -> Vec<u8> {
    const SS: u32 = 4;
    let [r, g, b] = state.color();
    let mut rgba = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
    for py in 0..ICON_SIZE {
        for px in 0..ICON_SIZE {
            let (mut disc, mut glyph) = (0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    // Coordonnées normalisées sur une grille de 32 unités.
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) * 32.0 / ICON_SIZE as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) * 32.0 / ICON_SIZE as f32;
                    if (x - 16.0).powi(2) + (y - 16.0).powi(2) <= 15.5f32.powi(2) {
                        disc += 1;
                        if in_microphone(x, y) {
                            glyph += 1;
                        }
                    }
                }
            }
            let n = (SS * SS) as f32;
            let white = glyph as f32 / disc.max(1) as f32;
            let mix = |c: u8| (f32::from(c) * (1.0 - white) + 255.0 * white).round() as u8;
            rgba.extend([
                mix(r),
                mix(g),
                mix(b),
                (255.0 * disc as f32 / n).round() as u8,
            ]);
        }
    }
    rgba
}

fn in_microphone(x: f32, y: f32) -> bool {
    // Capsule : rectangle arrondi centré en x = 16, de y = 6 à y = 19.
    let capsule = {
        let dx = (x - 16.0).abs() - 1.0;
        let cy = y.clamp(10.0, 15.0);
        dx.max(0.0).powi(2) + (y - cy).powi(2) <= 16.0 && dx <= 3.0
    };
    // Arceau : demi-anneau sous la capsule.
    let d = ((x - 16.0).powi(2) + (y - 15.0).powi(2)).sqrt();
    let arc = y >= 14.0 && (6.0..=8.2).contains(&d);
    let stem = (x - 16.0).abs() <= 0.9 && (22.5..=25.5).contains(&y);
    let base = (x - 16.0).abs() <= 4.5 && (24.8..=26.3).contains(&y);
    capsule || arc || stem || base
}

#[cfg(windows)]
pub use self::win::{Tray, MENU_OPEN_CONFIG, MENU_OPEN_UI, MENU_QUIT};

#[cfg(windows)]
mod win {
    use anyhow::{Context, Result};
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    use super::{icon_rgba, TrayState, ICON_SIZE};

    pub const MENU_OPEN_UI: &str = "open-ui";
    pub const MENU_OPEN_CONFIG: &str = "open-config";
    pub const MENU_QUIT: &str = "quit";

    pub struct Tray {
        icon: TrayIcon,
        icons: Vec<Icon>,
        shown: TrayState,
        shortcut: String,
    }

    impl Tray {
        /// Crée l'icône ; doit être appelé sur le thread de la boucle de messages.
        pub fn new(shortcut: &str) -> Result<Self> {
            let icons = TrayState::ALL
                .iter()
                .map(|&s| Icon::from_rgba(icon_rgba(s), ICON_SIZE, ICON_SIZE))
                .collect::<Result<Vec<_>, _>>()
                .context("création des icônes")?;

            let menu = Menu::new();
            let title = MenuItem::new(concat!("Dictia ", env!("CARGO_PKG_VERSION")), false, None);
            let open_ui = MenuItem::with_id(MENU_OPEN_UI, "Ouvrir Dictia", true, None);
            let open = MenuItem::with_id(
                MENU_OPEN_CONFIG,
                "Ouvrir le dossier de configuration",
                true,
                None,
            );
            let quit = MenuItem::with_id(MENU_QUIT, "Quitter", true, None);
            menu.append_items(&[
                &title,
                &PredefinedMenuItem::separator(),
                &open_ui,
                &open,
                &quit,
            ])
            .context("création du menu")?;

            let icon = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                // Clic gauche : ouvre la fenêtre ; clic droit : menu.
                .with_menu_on_left_click(false)
                .with_tooltip(TrayState::Idle.tooltip(shortcut))
                .with_icon(icons[TrayState::Idle.index()].clone())
                .build()
                .context("création de l'icône de notification")?;
            Ok(Self {
                icon,
                icons,
                shown: TrayState::Idle,
                shortcut: shortcut.to_owned(),
            })
        }

        /// Met à jour les raccourcis cités dans l'infobulle.
        pub fn set_shortcuts(&mut self, shortcut: &str) {
            self.shortcut = shortcut.to_owned();
            let _ = self
                .icon
                .set_tooltip(Some(self.shown.tooltip(&self.shortcut)));
        }

        pub fn set_state(&mut self, state: TrayState) {
            if state == self.shown {
                return;
            }
            self.shown = state;
            let _ = self.icon.set_icon(Some(self.icons[state.index()].clone()));
            let _ = self.icon.set_tooltip(Some(state.tooltip(&self.shortcut)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * ICON_SIZE + x) * 4) as usize;
        rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn icon_has_expected_shape() {
        let rgba = icon_rgba(TrayState::Idle);
        assert_eq!(rgba.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        assert_eq!(pixel(&rgba, 0, 0)[3], 0, "coin transparent");
        assert_eq!(pixel(&rgba, 16, 12), [255, 255, 255, 255], "micro blanc");
        assert_eq!(pixel(&rgba, 4, 16), [0x3B, 0x6E, 0xD8, 255], "fond bleu");
    }

    #[test]
    fn each_state_has_its_own_icon() {
        let icons: Vec<_> = TrayState::ALL.iter().map(|&s| icon_rgba(s)).collect();
        for (i, a) in icons.iter().enumerate() {
            for b in &icons[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn tooltips_mention_shortcut() {
        assert!(TrayState::Idle
            .tooltip("Ctrl+Alt+Espace")
            .contains("Ctrl+Alt+Espace"));
        // Limite Windows : 127 caractères.
        for s in TrayState::ALL {
            assert!(
                s.tooltip("Ctrl+Alt+Espace, ou maintenir Alt+Maj+Espace")
                    .chars()
                    .count()
                    < 128
            );
        }
    }
}
