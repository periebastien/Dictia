//! Transcription : trait commun aux backends.
//!
//! - `standard` : `/v1/audio/transcriptions`, envoi de l'audio complet puis
//!   réponse en streaming (MVP) ;
//! - `realtime` : streaming de l'audio pendant la dictée (étape ultérieure).

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
        prompt: &'a str,
        progress: &'a mut ProgressFn<'_>,
    ) -> BoxFuture<'a, Result<String>>;
}
