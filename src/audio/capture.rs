//! Capture du micro (WASAPI via cpal).
//!
//! Le flux vit dans un thread dédié (les flux cpal ne sont pas `Send` sur
//! toutes les plateformes). Chaque bloc reçu est converti à la volée en mono
//! 16 kHz : à l'arrêt, l'audio est immédiatement prêt pour la VAD.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};

use super::resample::{downmix, StreamResampler};
use super::vad::rms;
use super::TARGET_RATE;
use crate::transcription::realtime;

type ErrorCallback = Arc<dyn Fn(String) + Send + Sync>;

/// Audio envoyé en direct : PCM 16 bits mono à la fréquence `rate`.
pub struct LiveAudio {
    pub tx: tokio::sync::mpsc::UnboundedSender<Vec<i16>>,
    pub rate: u32,
}

/// Conversion à la fréquence du moteur et envoi par blocs de l'audio en
/// direct.
struct LiveSink {
    tx: tokio::sync::mpsc::UnboundedSender<Vec<i16>>,
    resampler: StreamResampler,
    pending: Vec<f32>,
    /// Taille d'un bloc : 50 ms, assez petit pour la réactivité, assez gros
    /// pour le réseau.
    chunk: usize,
}

impl LiveSink {
    fn new(live: LiveAudio, device_rate: u32) -> Self {
        let chunk = (live.rate / 20) as usize;
        Self {
            tx: live.tx,
            resampler: StreamResampler::new(device_rate, live.rate),
            pending: Vec::with_capacity(chunk * 2),
            chunk,
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

/// Enregistrement en cours dans le rappel du micro.
struct Take {
    buffer: Vec<f32>,
    live: Option<LiveAudio>,
    /// Convertisseurs, créés au premier bloc (la fréquence du micro n'est
    /// connue que du thread audio).
    state: Option<(StreamResampler, Option<LiveSink>)>,
}

/// Micro ouvert. Il peut rester ouvert entre deux dictées (réglage « Laisser
/// le micro ouvert ») : seul ce qui arrive entre [`begin`](Self::begin) et
/// [`end`](Self::end) est conservé, le reste est ignoré.
pub struct Microphone {
    device: String,
    take: Arc<Mutex<Option<Take>>>,
    alive: Arc<AtomicBool>,
    stop_tx: mpsc::Sender<()>,
}

impl Microphone {
    /// Ouvre le micro `device` (nom exact, ou vide pour celui de Windows)
    /// sans bloquer l'appelant ; `on_error` est appelé depuis le thread
    /// audio si le micro ne peut pas être ouvert ou décroche. `level` reçoit
    /// en continu le volume RMS du dernier bloc (bits d'un `f32`).
    pub fn open(
        device: &str,
        level: Arc<AtomicU32>,
        on_error: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<Self> {
        let take: Arc<Mutex<Option<Take>>> = Arc::new(Mutex::new(None));
        let alive = Arc::new(AtomicBool::new(true));
        let on_error: ErrorCallback = {
            let alive = Arc::clone(&alive);
            Arc::new(move |e| {
                alive.store(false, Ordering::SeqCst);
                on_error(e);
            })
        };
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (name, shared) = (device.to_owned(), Arc::clone(&take));
        thread::Builder::new()
            .name("dictia-audio".into())
            .spawn(move || {
                match open_stream(&name, shared, level, Arc::clone(&on_error)) {
                    // Le flux vit jusqu'à l'ordre d'arrêt (ou l'abandon du micro).
                    Ok(_stream) => {
                        let _ = stop_rx.recv();
                    }
                    Err(e) => on_error(format!("{e:#}")),
                }
            })
            .context("création du thread audio")?;
        Ok(Self {
            device: device.to_owned(),
            take,
            alive,
            stop_tx,
        })
    }

    /// Nom demandé à l'ouverture (vide : micro de Windows).
    pub fn device(&self) -> &str {
        &self.device
    }

    /// Faux si le micro n'a pas pu s'ouvrir ou a décroché.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Commence à garder l'audio. `live`, s'il est fourni, reçoit l'audio en
    /// PCM 16 bits au fil de la capture (transcription en direct) ;
    /// il est fermé par [`end`](Self::end).
    pub fn begin(&self, live: Option<LiveAudio>) {
        if let Ok(mut take) = self.take.lock() {
            *take = Some(Take {
                buffer: Vec::with_capacity(TARGET_RATE as usize * 30),
                live,
                state: None,
            });
        }
    }

    /// Arrête de garder l'audio et le renvoie, en mono 16 kHz.
    pub fn end(&self) -> Result<Vec<f32>> {
        let take = self
            .take
            .lock()
            .map_err(|_| anyhow!("tampon audio corrompu"))?
            .take();
        // Envoie le reste de l'audio en direct et ferme son canal.
        let samples = take.map(|t| t.buffer).unwrap_or_default();
        if samples.is_empty() && !self.is_alive() {
            return Err(anyhow!("micro indisponible"));
        }
        Ok(samples)
    }
}

impl Drop for Microphone {
    /// Ferme le micro (sans attendre le thread audio).
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
    }
}

/// Vrai si l'erreur arrête le flux. Un tampon débordé (machine chargée),
/// un changement de micro par défaut suivi automatiquement ou une priorité
/// temps réel refusée laissent le micro enregistrer.
fn is_fatal(kind: cpal::ErrorKind) -> bool {
    !matches!(
        kind,
        cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied
    )
}

/// Noms des micros branchés, pour le choix dans les réglages.
pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let mut names: Vec<String> = host
        .input_devices()
        .map(|devices| {
            devices
                .filter_map(|d| d.description().ok().map(|d| d.name().to_owned()))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names.dedup();
    names
}

/// Micro demandé, ou celui de Windows s'il est vide ou débranché.
fn find_device(host: &cpal::Host, name: &str) -> Result<cpal::Device> {
    if !name.is_empty() {
        let found = host.input_devices().ok().and_then(|mut devices| {
            devices.find(|d| d.description().is_ok_and(|desc| desc.name() == name))
        });
        match found {
            Some(device) => return Ok(device),
            None => log::warn!("micro « {name} » introuvable : micro de Windows utilisé"),
        }
    }
    host.default_input_device()
        .context("aucun micro par défaut")
}

fn open_stream(
    name: &str,
    take: Arc<Mutex<Option<Take>>>,
    level: Arc<AtomicU32>,
    on_error: ErrorCallback,
) -> Result<Stream> {
    let host = cpal::default_host();
    let device = find_device(&host, name)?;
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
        SampleFormat::F32 => build::<f32>(&device, config, take, level, on_error),
        SampleFormat::I16 => build::<i16>(&device, config, take, level, on_error),
        SampleFormat::U16 => build::<u16>(&device, config, take, level, on_error),
        SampleFormat::I32 => build::<i32>(&device, config, take, level, on_error),
        SampleFormat::I8 => build::<i8>(&device, config, take, level, on_error),
        SampleFormat::U8 => build::<u8>(&device, config, take, level, on_error),
        SampleFormat::F64 => build::<f64>(&device, config, take, level, on_error),
        other => return Err(anyhow!("format audio non pris en charge : {other:?}")),
    }?;
    stream.play().context("démarrage du micro")?;
    Ok(stream)
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    take: Arc<Mutex<Option<Take>>>,
    level: Arc<AtomicU32>,
    on_error: ErrorCallback,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let rate = config.sample_rate;
    let mut scratch: Vec<f32> = Vec::new();
    let stream = device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                let Ok(mut guard) = take.lock() else {
                    return;
                };
                // Hors dictée : rien n'est gardé.
                let Some(take) = guard.as_mut() else {
                    return;
                };
                scratch.clear();
                scratch.extend(data.iter().map(|&x| x.to_sample::<f32>()));
                level.store(rms(&scratch).to_bits(), Ordering::Relaxed);
                let Take {
                    buffer,
                    live,
                    state,
                } = take;
                let (resampler, sink) = state.get_or_insert_with(|| {
                    (
                        StreamResampler::new(rate, TARGET_RATE),
                        live.take().map(|l| LiveSink::new(l, rate)),
                    )
                });
                downmix(&scratch, channels, |x| {
                    resampler.push(x, buffer);
                    if let Some(sink) = sink.as_mut() {
                        sink.push(x);
                    }
                });
                if let Some(sink) = sink.as_mut() {
                    sink.flush(sink.chunk);
                }
            },
            move |err| {
                if is_fatal(err.kind()) {
                    on_error(format!("erreur du micro : {err}"));
                } else {
                    // Le flux continue : un simple raté audio ne coupe pas la dictée.
                    log::warn!("micro : {err}");
                }
            },
            None,
        )
        .context("ouverture du flux micro")?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glitches_do_not_stop_the_dictation() {
        assert!(!is_fatal(cpal::ErrorKind::Xrun));
        assert!(!is_fatal(cpal::ErrorKind::DeviceChanged));
        assert!(is_fatal(cpal::ErrorKind::DeviceNotAvailable));
        assert!(is_fatal(cpal::ErrorKind::StreamInvalidated));
    }
}
