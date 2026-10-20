//! Capture du micro par défaut (WASAPI via cpal).
//!
//! Le flux vit dans un thread dédié (les flux cpal ne sont pas `Send` sur
//! toutes les plateformes). Chaque bloc reçu est converti à la volée en mono
//! 16 kHz : à l'arrêt, l'audio est immédiatement prêt pour la VAD.

use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};

use super::resample::{downmix, StreamResampler};
use super::TARGET_RATE;

type ErrorCallback = Arc<dyn Fn(String) + Send + Sync>;

/// Enregistrement en cours. `stop` renvoie l'audio mono 16 kHz.
pub struct Recorder {
    stop_tx: mpsc::Sender<()>,
    handle: JoinHandle<Result<Vec<f32>>>,
}

impl Recorder {
    /// Démarre la capture sans bloquer l'appelant ; `on_error` est appelé
    /// depuis le thread audio si le micro ne peut pas être ouvert ou décroche.
    pub fn start(on_error: impl Fn(String) + Send + Sync + 'static) -> Result<Self> {
        let on_error: ErrorCallback = Arc::new(on_error);
        let (stop_tx, stop_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("dictia-audio".into())
            .spawn(move || {
                let buffer = Arc::new(Mutex::new(Vec::with_capacity(TARGET_RATE as usize * 30)));
                let stream = match open_stream(Arc::clone(&buffer), Arc::clone(&on_error)) {
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

fn open_stream(buffer: Arc<Mutex<Vec<f32>>>, on_error: ErrorCallback) -> Result<Stream> {
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
        SampleFormat::F32 => build::<f32>(&device, config, buffer, on_error),
        SampleFormat::I16 => build::<i16>(&device, config, buffer, on_error),
        SampleFormat::U16 => build::<u16>(&device, config, buffer, on_error),
        SampleFormat::I32 => build::<i32>(&device, config, buffer, on_error),
        SampleFormat::I8 => build::<i8>(&device, config, buffer, on_error),
        SampleFormat::U8 => build::<u8>(&device, config, buffer, on_error),
        SampleFormat::F64 => build::<f64>(&device, config, buffer, on_error),
        other => return Err(anyhow!("format audio non pris en charge : {other:?}")),
    }?;
    stream.play().context("démarrage du micro")?;
    Ok(stream)
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    buffer: Arc<Mutex<Vec<f32>>>,
    on_error: ErrorCallback,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let mut resampler = StreamResampler::new(config.sample_rate, TARGET_RATE);
    let mut scratch: Vec<f32> = Vec::new();
    let stream = device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                scratch.clear();
                scratch.extend(data.iter().map(|&x| x.to_sample::<f32>()));
                if let Ok(mut out) = buffer.lock() {
                    downmix(&scratch, channels, |x| resampler.push(x, &mut out));
                }
            },
            move |err| on_error(format!("erreur du micro : {err}")),
            None,
        )
        .context("ouverture du flux micro")?;
    Ok(stream)
}
