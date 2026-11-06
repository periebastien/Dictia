//! Transcription : trait commun aux backends.
//!
//! - `standard` : `/v1/audio/transcriptions`, envoi de l'audio complet puis
//!   réponse en streaming (MVP) ;
//! - `realtime` : streaming de l'audio pendant la dictée (étape ultérieure).

pub mod realtime;
pub mod sse;
pub mod standard;

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Audio encodé prêt à l'envoi.
#[derive(Debug, Clone)]
pub struct AudioPayload {
    pub bytes: Vec<u8>,
    pub file_name: &'static str,
    pub mime: &'static str,
}

impl AudioPayload {
    pub fn flac(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            file_name: "audio.flac",
            mime: "audio/flac",
        }
    }
}

/// Jalons signalés pendant une transcription, pour mesurer les latences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress<'a> {
    /// En-têtes de réponse reçus : l'upload est terminé.
    ResponseStarted,
    /// Fragment de texte reçu.
    Delta(&'a str),
}

/// Aide donnée au modèle : contexte libre et termes du vocabulaire.
#[derive(Debug, Clone, Copy, Default)]
pub struct Guidance<'a> {
    pub prompt: &'a str,
    pub terms: &'a [String],
}

/// Termes envoyés à Voxtral (`context_bias`). Mistral refuse les espaces et
/// les virgules : un terme de plusieurs mots est découpé, sans ses petits
/// mots (« la hero section » → « hero », « section »).
pub fn mistral_bias_terms(terms: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for term in terms {
        let words: Vec<&str> = term
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|w| !w.is_empty())
            .collect();
        let several = words.len() > 1;
        for word in words {
            if several && word.chars().count() < 3 {
                continue;
            }
            if !out.iter().any(|w| w == word) {
                out.push(word.to_owned());
            }
        }
    }
    out
}

pub type ProgressFn<'a> = dyn FnMut(Progress<'_>) + Send + 'a;

pub trait Backend: Send + Sync {
    fn name(&self) -> &'static str;

    /// Ouvre la connexion (DNS, TCP, TLS) à l'avance pour que la requête
    /// de transcription parte sur une connexion déjà établie.
    fn warm_up(&self) -> BoxFuture<'_, ()>;

    /// Transcrit l'audio et renvoie le texte complet.
    fn transcribe<'a>(
        &'a self,
        audio: AudioPayload,
        guidance: Guidance<'a>,
        progress: &'a mut ProgressFn<'_>,
    ) -> BoxFuture<'a, Result<String>>;
}

#[cfg(test)]
mod bias_tests {
    use super::*;

    #[test]
    fn mistral_terms_have_no_spaces_or_commas() {
        let terms: Vec<String> = ["la hero section", " n8n ", "", "AI", "Hero, CTA", "section"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            mistral_bias_terms(&terms),
            ["hero", "section", "n8n", "AI", "Hero", "CTA"]
        );
    }
}
