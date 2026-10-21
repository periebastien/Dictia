//! Dictia — dictée vocale pour Windows.
//!
//! Raccourci global → capture micro → transcription OpenAI → frappe du texte
//! au curseur, avec une priorité absolue : la latence minimale.

// Pas de console en release : Dictia vit dans la zone de notification.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Hors Windows, seule la logique métier est compilée (pour les tests).
#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(windows)]
mod app;
mod audio;
mod config;
#[cfg(windows)]
mod controller;
mod dictation;
mod history;
mod hotkey;
mod inject;
mod live_typing;
mod metrics;
mod overlay;
mod platform;
mod settings;
mod stats;
mod transcription;
mod tray;
mod ui;
mod vocab;

#[cfg(windows)]
fn main() {
    if let Err(e) = app::run() {
        log::error!("{e:#}");
        platform::show_error(&format!("Dictia n'a pas pu démarrer :\n\n{e:#}"));
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "Dictia ne fonctionne que sous Windows ; utilisez `cargo test` pour la logique métier."
    );
    std::process::exit(1);
}
