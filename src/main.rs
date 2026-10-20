//! Dictia — dictée vocale pour Windows.
//!
//! Raccourci global → capture micro → transcription OpenAI → frappe du texte
//! au curseur, avec une priorité absolue : la latence minimale.

// Pas de console en release : Dictia vit dans la zone de notification.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Modules câblés progressivement ; retiré quand l'application est assemblée.
#![allow(dead_code)]

mod audio;
mod config;
mod vocab;

fn main() {
    eprintln!("Dictia : initialisation en cours.");
}
