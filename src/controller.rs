//! Contrôleur de dictée : capture → VAD → FLAC → transcription →
//! remplacements → frappe, plus l'état partagé avec la boucle principale.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tokio::sync::{mpsc, Mutex};

use crate::audio::capture::Recorder;
use crate::audio::{duration_of, flac, vad, TARGET_RATE};
use crate::config::Config;
use crate::dictation::{self, Effect, Phase};
use crate::history::{self, Entry};
use crate::hotkey::Action;
use crate::inject;
use crate::metrics::Latencies;
use crate::platform::{self, WindowHandle};
use crate::transcription::{AudioPayload, Backend, Progress};
use crate::tray::TrayState;
use crate::vocab::{Vocab, VocabFile};

/// Réglages en vigueur, remplacés à chaud depuis l'interface.
pub struct Shared {
    pub config: Config,
    pub vocab_file: VocabFile,
    pub vocab: Arc<Vocab>,
    pub api_key: Option<String>,
    pub backend: Option<Arc<dyn Backend>>,
}

pub type SharedRef = Arc<RwLock<Shared>>;

/// Événement destiné à la boucle principale (interface).
pub enum Event {
    History(Entry),
    /// Une dictée a été tentée sans clé API.
    NeedApiKey,
}

/// État de la dictée, lu par la boucle principale (icône, indicateur,
/// interface).
pub struct Status {
    pub main_thread: u32,
    /// Volume RMS du micro (bits d'un `f32`), écrit par le thread audio.
    pub mic_level: Arc<AtomicU32>,
    recording: AtomicBool,
    processing: AtomicUsize,
    error: AtomicBool,
}

impl Status {
    pub fn new(main_thread: u32) -> Self {
        Self {
            main_thread,
            mic_level: Arc::new(AtomicU32::new(0)),
            recording: AtomicBool::new(false),
            processing: AtomicUsize::new(0),
            error: AtomicBool::new(false),
        }
    }

    pub fn get(&self) -> TrayState {
        if self.recording.load(Ordering::SeqCst) {
            TrayState::Recording
        } else if self.processing.load(Ordering::SeqCst) > 0 {
            TrayState::Processing
        } else if self.error.load(Ordering::SeqCst) {
            TrayState::Error
        } else {
            TrayState::Idle
        }
    }

    pub fn mic_rms(&self) -> f32 {
        f32::from_bits(self.mic_level.load(Ordering::Relaxed))
    }

    pub fn notify(&self) {
        platform::post_wake(self.main_thread);
    }

    fn set_recording(&self, on: bool) {
        if on {
            self.error.store(false, Ordering::SeqCst);
        }
        self.recording.store(on, Ordering::SeqCst);
        if !on {
            self.mic_level.store(0, Ordering::Relaxed);
        }
        self.notify();
    }

    fn begin_processing(&self) {
        self.processing.fetch_add(1, Ordering::SeqCst);
        self.notify();
    }

    fn end_processing(&self) {
        self.processing.fetch_sub(1, Ordering::SeqCst);
        self.notify();
    }

    fn fail(&self, context: &str, err: &dyn std::fmt::Display) {
        log::error!("{context} : {err}");
        self.error.store(true, Ordering::SeqCst);
        platform::beep_error();
        self.notify();
    }
}

/// Enregistrement en cours.
struct Session {
    recorder: Recorder,
    target: Option<WindowHandle>,
    started: Instant,
    mode: &'static str,
}

/// Dictée enregistrée, à transcrire.
struct Capture {
    samples: Vec<f32>,
    target: Option<WindowHandle>,
    recording: Duration,
    stopped: Instant,
    mode: &'static str,
}

#[derive(Clone)]
pub struct Controller {
    pub shared: SharedRef,
    pub status: Arc<Status>,
    pub events: std::sync::mpsc::Sender<Event>,
    /// Sérialise les frappes si deux dictées se terminent en même temps.
    pub inject_lock: Arc<Mutex<()>>,
    pub metrics_path: PathBuf,
    pub history_path: PathBuf,
}

impl Controller {
    pub async fn run(self, mut rx: mpsc::UnboundedReceiver<Action>) {
        let mut phase = Phase::Idle;
        let mut session: Option<Session> = None;
        // Arrêt programmé (prolongement après relâchement).
        let mut deadline: Option<tokio::time::Instant> = None;
        loop {
            let (next, effect) = tokio::select! {
                action = rx.recv() => match action {
                    Some(action) => dictation::on_action(phase, action, self.release_tail()),
                    None => break,
                },
                () = sleep_until_opt(deadline) => dictation::on_tail_elapsed(phase),
            };
            phase = next;
            match effect {
                Effect::Start(mode) => {
                    session = self.start(mode);
                    if session.is_none() {
                        phase = Phase::Idle;
                    }
                }
                Effect::Stop => {
                    deadline = None;
                    if let Some(s) = session.take() {
                        self.stop(s);
                    }
                }
                Effect::StopAfter(tail) => {
                    deadline = Some(tokio::time::Instant::now() + tail);
                }
                Effect::KeepRecording => {
                    deadline = None;
                    log::debug!("nouvel appui pendant le prolongement : la dictée continue");
                }
                Effect::Nothing => {}
            }
        }
    }

    fn release_tail(&self) -> Duration {
        let ms = self
            .shared
            .read()
            .map_or(0, |s| s.config.audio.release_tail_ms);
        Duration::from_millis(u64::from(ms))
    }

    fn backend(&self) -> Option<Arc<dyn Backend>> {
        self.shared.read().ok()?.backend.clone()
    }

    /// Début : mémorise la fenêtre, ouvre le micro et la connexion.
    fn start(&self, mode: &'static str) -> Option<Session> {
        let started = Instant::now();
        let Some(backend) = self.backend() else {
            let _ = self.events.send(Event::NeedApiKey);
            self.status.notify();
            return None;
        };
        let target = platform::foreground_window();
        tokio::spawn(async move { backend.warm_up().await });

        let status = Arc::clone(&self.status);
        match Recorder::start(Arc::clone(&self.status.mic_level), move |e| {
            status.recording.store(false, Ordering::SeqCst);
            status.fail("micro", &e);
        }) {
            Ok(recorder) => {
                self.status.set_recording(true);
                log::info!("enregistrement démarré ({mode})");
                Some(Session {
                    recorder,
                    target,
                    started,
                    mode,
                })
            }
            Err(e) => {
                self.status.fail("micro", &format!("{e:#}"));
                None
            }
        }
    }

    /// Fin : arrête le micro et lance le traitement en tâche de fond,
    /// pour qu'une nouvelle dictée puisse commencer aussitôt.
    fn stop(&self, session: Session) {
        let stopped = Instant::now();
        let recording = session.started.elapsed();
        self.status.set_recording(false);
        let samples = match session.recorder.stop() {
            Ok(s) => s,
            Err(e) => {
                // Déjà signalé par le rappel d'erreur du micro.
                log::warn!("aucun audio : {e:#}");
                return;
            }
        };
        log::info!("enregistrement arrêté ({} ms)", recording.as_millis());

        let this = self.clone();
        self.status.begin_processing();
        tokio::spawn(async move {
            let capture = Capture {
                samples,
                target: session.target,
                recording,
                stopped,
                mode: session.mode,
            };
            if let Err(e) = this.process(capture).await {
                this.status.fail("dictée", &format!("{e:#}"));
            }
            this.status.end_processing();
        });
    }

    async fn process(&self, capture: Capture) -> Result<()> {
        // Réglages figés pour toute la durée de cette dictée.
        let (backend, vocab, config) = {
            let shared = self
                .shared
                .read()
                .map_err(|_| anyhow::anyhow!("réglages indisponibles"))?;
            let Some(backend) = shared.backend.clone() else {
                let _ = self.events.send(Event::NeedApiKey);
                return Ok(());
            };
            (backend, Arc::clone(&shared.vocab), shared.config.clone())
        };

        let stopped = capture.stopped;
        let mut lat = Latencies {
            backend: backend.name(),
            recording: capture.recording,
            ..Default::default()
        };

        // Retrait des silences + FLAC, hors du runtime asynchrone.
        let audio_cfg = config.audio.clone();
        let samples = capture.samples;
        let encoding = Instant::now();
        let encoded =
            tokio::task::spawn_blocking(move || -> Result<Option<(Vec<u8>, Duration)>> {
                let Some(speech) = vad::trim_silence(&samples, TARGET_RATE, &audio_cfg) else {
                    return Ok(None);
                };
                let bytes = flac::encode_mono(speech, TARGET_RATE)?;
                Ok(Some((bytes, duration_of(speech.len(), TARGET_RATE))))
            })
            .await??;
        lat.encode = encoding.elapsed();
        let Some((flac_bytes, speech)) = encoded else {
            log::info!("aucune parole détectée, rien à transcrire");
            return Ok(());
        };
        lat.speech = speech;
        lat.flac_bytes = flac_bytes.len();

        let sent = Instant::now();
        let (mut upload, mut first_token) = (None, None);
        let text = {
            let mut progress = |p: Progress<'_>| match p {
                Progress::ResponseStarted => {
                    upload.get_or_insert_with(|| sent.elapsed());
                }
                Progress::Delta(_) => {
                    first_token.get_or_insert_with(|| stopped.elapsed());
                }
            };
            backend
                .transcribe(
                    AudioPayload::flac(flac_bytes),
                    vocab.prompt(),
                    &mut progress,
                )
                .await?
        };
        lat.upload = upload;
        lat.first_token = first_token;
        lat.done = stopped.elapsed();

        if vocab.is_prompt_echo(&text) {
            log::info!("transcription ignorée : le modèle a recopié le prompt (pas de parole ?)");
            return Ok(());
        }
        let text = inject::prepare_text(&vocab.apply(&text), config.inject.trailing_space);
        lat.chars = text.chars().count();
        if text.is_empty() {
            log::info!("transcription vide");
            return Ok(());
        }

        let _guard = self.inject_lock.lock().await;
        let typing = Instant::now();
        let target = capture.target;
        let typed = text.clone();
        let app = tokio::task::spawn_blocking(move || -> Result<String> {
            inject::type_into(target, &typed)?;
            Ok(target
                .and_then(platform::window_exe)
                .map(|exe| history::friendly_app_name(&exe))
                .unwrap_or_default())
        })
        .await??;
        lat.inject = typing.elapsed();
        lat.total = stopped.elapsed();

        log::info!("{}", lat.summary());
        if let Err(e) = lat.append_csv(&self.metrics_path) {
            log::warn!(
                "écriture de {} impossible : {e}",
                self.metrics_path.display()
            );
        }

        let entry = Entry {
            at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
            text: text.trim_end().to_owned(),
            app,
            duration_ms: capture.recording.as_millis() as u64,
            mode: capture.mode.to_owned(),
        };
        if let Err(e) = history::append(&self.history_path, &entry) {
            log::warn!("historique non enregistré : {e}");
        }
        let _ = self.events.send(Event::History(entry));
        self.status.notify();
        Ok(())
    }
}

/// Attend `deadline`, ou indéfiniment s'il n'y en a pas.
async fn sleep_until_opt(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}
