//! Capture du micro par défaut (WASAPI via cpal).
//!
//! Le flux vit dans un thread dédié (les flux cpal ne sont pas `Send` sur
//! toutes les plateformes). Chaque bloc reçu est converti à la volée en mono
//! 16 kHz : à l'arrêt, l'audio est immédiatement prêt pour la VAD.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};

use super::resample::{downmix, StreamResampler};
use super::vad::rms;
use super::TARGET_RATE;
use crate::transcription::realtime;

type ErrorCallback = Arc<dyn Fn(String) + Send + Sync>;

/// Canal de l'audio envoyé en direct (PCM 16 bits, 24 kHz, mono).
pub type LiveAudio = tokio::sync::mpsc::UnboundedSender<Vec<i16>>;

/// Conversion en 24 kHz et envoi par blocs de l'audio en direct.
struct LiveSink {
    tx: LiveAudio,
    resampler: StreamResampler,
    pending: Vec<f32>,
}

impl LiveSink {
    /// 50 ms : assez petit pour la réactivité, assez gros pour le réseau.
    const CHUNK: usize = (realtime::RATE / 20) as usize;

    fn new(tx: LiveAudio, device_rate: u32) -> Self {
        Self {
            tx,
            resampler: StreamResampler::new(device_rate, realtime::RATE),
            pending: Vec::with_capacity(Self::CHUNK * 2),
        }
    }

    fn push(&mut self, x: f32) {
        self.resampler.push(x, &mut self.pending);
    }

    fn flush(&mut self, min: usize) {
        if !self.pending.is_empty() && self.pending.len() >= min {
            let _ = self.tx.send(realtime::to_pcm16(&self.pending));
            self.pending.clear();
        }
    }
}

impl Drop for LiveSink {
    /// Fin de la capture : envoie le reste, puis ferme le canal.
    fn drop(&mut self) {
        self.flush(1);
    }
}

/// Enregistrement en cours. `stop` renvoie l'audio mono 16 kHz.
pub struct Recorder {
    stop_tx: mpsc::Sender<()>,
    handle: JoinHandle<Result<Vec<f32>>>,
}

impl Recorder {
    /// Démarre la capture sans bloquer l'appelant ; `on_error` est appelé
    /// depuis le thread audio si le micro ne peut pas être ouvert ou décroche.
    /// `level` reçoit en continu le volume RMS du dernier bloc (bits d'un `f32`).
    /// `live`, s'il est fourni, reçoit l'audio en PCM 16 bits 24 kHz au fil de
    /// la capture (transcription en direct) ; il est fermé à l'arrêt.
    pub fn start(
        level: Arc<AtomicU32>,
        live: Option<LiveAudio>,
        on_error: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<Self> {
        let on_error: ErrorCallback = Arc::new(on_error);
        let (stop_tx, stop_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("dictia-audio".into())
            .spawn(move || {
                let buffer = Arc::new(Mutex::new(Vec::with_capacity(TARGET_RATE as usize * 30)));
                let stream =
                    match open_stream(Arc::clone(&buffer), level, live, Arc::clone(&on_error)) {
                        Ok(s) => s,
                        Err(e) => {
                            on_error(format!("{e:#}"));
                            return Err(e);
                        }
                    };
                // Attend l'ordre d'arrêt (ou l'abandon du Recorder).
                let _ = stop_rx.recv();
                drop(stream);
                let samples = std::mem::take(
                    &mut *buffer
                        .lock()
                        .map_err(|_| anyhow!("tampon audio corrompu"))?,
                );
                Ok(samples)
            })
            .context("création du thread audio")?;
        Ok(Self { stop_tx, handle })
    }

    pub fn stop(self) -> Result<Vec<f32>> {
        let _ = self.stop_tx.send(());
        self.handle
            .join()
            .map_err(|_| anyhow!("le thread audio a paniqué"))?
    }
}

fn open_stream(
    buffer: Arc<Mutex<Vec<f32>>>,
    level: Arc<AtomicU32>,
    live: Option<LiveAudio>,
    on_error: ErrorCallback,
) -> Result<Stream> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("aucun micro par défaut")?;
    let supported = device
        .default_input_config()
        .context("configuration du micro illisible")?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    if let Ok(desc) = device.description() {
        log::info!(
            "micro : {} ({} Hz, {} canal(aux), {format:?})",
            desc.name(),
            config.sample_rate,
            config.channels
        );
    }

    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, config, buffer, level, live, on_error),
        SampleFormat::I16 => build::<i16>(&device, config, buffer, level, live, on_error),
        SampleFormat::U16 => build::<u16>(&device, config, buffer, level, live, on_error),
        SampleFormat::I32 => build::<i32>(&device, config, buffer, level, live, on_error),
        SampleFormat::I8 => build::<i8>(&device, config, buffer, level, live, on_error),
        SampleFormat::U8 => build::<u8>(&device, config, buffer, level, live, on_error),
        SampleFormat::F64 => build::<f64>(&device, config, buffer, level, live, on_error),
        other => return Err(anyhow!("format audio non pris en charge : {other:?}")),
    }?;
    stream.play().context("démarrage du micro")?;
    Ok(stream)
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    buffer: Arc<Mutex<Vec<f32>>>,
    level: Arc<AtomicU32>,
    live: Option<LiveAudio>,
    on_error: ErrorCallback,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let mut resampler = StreamResampler::new(config.sample_rate, TARGET_RATE);
    let mut scratch: Vec<f32> = Vec::new();
    let mut live = live.map(|tx| LiveSink::new(tx, config.sample_rate));
    let stream = device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                scratch.clear();
                scratch.extend(data.iter().map(|&x| x.to_sample::<f32>()));
                level.store(rms(&scratch).to_bits(), Ordering::Relaxed);
                if let Ok(mut out) = buffer.lock() {
                    downmix(&scratch, channels, |x| {
                        resampler.push(x, &mut out);
                        if let Some(live) = &mut live {
                            live.push(x);
                        }
                    });
                }
                if let Some(live) = &mut live {
                    live.flush(LiveSink::CHUNK);
                }
            },
            move |err| on_error(format!("erreur du micro : {err}")),
            None,
        )
        .context("ouverture du flux micro")?;
    Ok(stream)
}
