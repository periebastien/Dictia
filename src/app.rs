//! Assemblage de l'application Windows.
//!
//! - Thread principal : boucle de messages Win32 (raccourci global, icône).
//! - Runtime tokio : contrôleur qui enchaîne capture → VAD → FLAC →
//!   transcription → remplacements → frappe.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use simplelog::{
    CombinedLogger, ConfigBuilder, LevelFilter, SharedLogger, SimpleLogger, WriteLogger,
};
use tokio::sync::{mpsc, Mutex};
use tray_icon::menu::MenuEvent;

use crate::audio::capture::Recorder;
use crate::audio::{duration_of, flac, vad, TARGET_RATE};
use crate::config::{self, Config, API_KEY_ENV, CONFIG_FILE};
use crate::metrics::{Latencies, METRICS_FILE};
use crate::platform::{self, WindowHandle};
use crate::transcription::standard::StandardBackend;
use crate::transcription::{AudioPayload, Backend, Progress};
use crate::tray::{Tray, TrayState, MENU_OPEN_CONFIG, MENU_QUIT};
use crate::vocab::Vocab;
use crate::{hotkey, inject};

const LOG_FILE: &str = "dictia.log";
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

pub fn run() -> Result<()> {
    let dir = config::config_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("création de {}", dir.display()))?;
    init_logging(&dir);
    log::info!("Dictia {} démarre", env!("CARGO_PKG_VERSION"));

    let Some(_instance) = platform::acquire_single_instance() else {
        platform::show_warning(
            "Dictia est déjà lancé : son icône se trouve dans la zone de notification.",
        );
        return Ok(());
    };

    let config = Config::load_or_create(&dir)?;
    let vocab = Vocab::load_or_create(&dir)?;
    log::info!(
        "configuration : {:?} ; prompt de {} caractères, {} remplacement(s)",
        config.api,
        vocab.prompt().chars().count(),
        vocab.rule_count()
    );
    let Some(api_key) = config.api.resolve_api_key(std::env::var(API_KEY_ENV).ok()) else {
        open_folder(&dir);
        platform::show_error(&format!(
            "Aucune clé API OpenAI.\n\nRenseignez api_key dans\n{}\nou définissez la variable \
             d'environnement {API_KEY_ENV}, puis relancez Dictia.",
            dir.join(CONFIG_FILE).display()
        ));
        return Ok(());
    };
    let backend: Arc<dyn Backend> = Arc::new(StandardBackend::new(&config.api, api_key)?);

    let status = Arc::new(Status::new(platform::current_thread_id()));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("dictia-worker")
        .enable_all()
        .build()
        .context("démarrage du runtime")?;
    let (tx, rx) = mpsc::unbounded_channel();

    let mut tray = Tray::new(&config.hotkey.shortcut)?;
    {
        let dir = dir.clone();
        let main_thread = status.main_thread;
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == MENU_QUIT {
                platform::post_quit(main_thread);
            } else if event.id == MENU_OPEN_CONFIG {
                open_folder(&dir);
            }
        }));
    }

    let _hotkey = match hotkey::register(&config.hotkey.shortcut, move || {
        let _ = tx.send(Command::Toggle);
    }) {
        Ok(h) => h,
        Err(e) => {
            platform::show_error(&format!(
                "{e:#}\n\nChoisissez un autre raccourci dans {} puis relancez Dictia.",
                dir.join(CONFIG_FILE).display()
            ));
            log::error!("{e:#}");
            return Ok(());
        }
    };
    log::info!("prêt : {} pour dicter", config.hotkey.shortcut);

    let controller = Controller {
        backend,
        vocab: Arc::new(vocab),
        config: Arc::new(config),
        status: Arc::clone(&status),
        inject_lock: Arc::new(Mutex::new(())),
        metrics_path: dir.join(METRICS_FILE),
    };
    runtime.spawn(controller.run(rx));

    platform::run_message_loop(|| tray.set_state(status.get()));

    log::info!("arrêt de Dictia");
    runtime.shutdown_timeout(Duration::from_millis(500));
    Ok(())
}

enum Command {
    Toggle,
}

/// État partagé, lu par la boucle principale pour mettre à jour l'icône.
struct Status {
    main_thread: u32,
    recording: AtomicBool,
    processing: AtomicUsize,
    error: AtomicBool,
}

impl Status {
    fn new(main_thread: u32) -> Self {
        Self {
            main_thread,
            recording: AtomicBool::new(false),
            processing: AtomicUsize::new(0),
            error: AtomicBool::new(false),
        }
    }

    fn get(&self) -> TrayState {
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

    fn notify(&self) {
        platform::post_wake(self.main_thread);
    }

    fn set_recording(&self, on: bool) {
        if on {
            self.error.store(false, Ordering::SeqCst);
        }
        self.recording.store(on, Ordering::SeqCst);
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
}

/// Dictée enregistrée, à transcrire.
struct Capture {
    samples: Vec<f32>,
    target: Option<WindowHandle>,
    recording: Duration,
    stopped: Instant,
}

#[derive(Clone)]
struct Controller {
    backend: Arc<dyn Backend>,
    vocab: Arc<Vocab>,
    config: Arc<Config>,
    status: Arc<Status>,
    /// Sérialise les frappes si deux dictées se terminent en même temps.
    inject_lock: Arc<Mutex<()>>,
    metrics_path: PathBuf,
}

impl Controller {
    async fn run(self, mut rx: mpsc::UnboundedReceiver<Command>) {
        let mut session: Option<Session> = None;
        while let Some(Command::Toggle) = rx.recv().await {
            session = match session.take() {
                None => self.start(),
                Some(s) => {
                    self.stop(s);
                    None
                }
            };
        }
    }

    /// 1er appui : mémorise la fenêtre, ouvre le micro et la connexion.
    fn start(&self) -> Option<Session> {
        let started = Instant::now();
        let target = platform::foreground_window();

        let backend = Arc::clone(&self.backend);
        tokio::spawn(async move { backend.warm_up().await });

        let status = Arc::clone(&self.status);
        match Recorder::start(move |e| {
            status.recording.store(false, Ordering::SeqCst);
            status.fail("micro", &e);
        }) {
            Ok(recorder) => {
                self.status.set_recording(true);
                log::info!("enregistrement démarré");
                Some(Session {
                    recorder,
                    target,
                    started,
                })
            }
            Err(e) => {
                self.status.fail("micro", &format!("{e:#}"));
                None
            }
        }
    }

    /// 2e appui : arrête le micro et lance le traitement en tâche de fond,
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
            };
            if let Err(e) = this.process(capture).await {
                this.status.fail("dictée", &format!("{e:#}"));
            }
            this.status.end_processing();
        });
    }

    async fn process(&self, capture: Capture) -> Result<()> {
        let stopped = capture.stopped;
        let mut lat = Latencies {
            backend: self.backend.name(),
            recording: capture.recording,
            ..Default::default()
        };

        // Retrait des silences + FLAC, hors du runtime asynchrone.
        let audio_cfg = self.config.audio.clone();
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
            self.backend
                .transcribe(
                    AudioPayload::flac(flac_bytes),
                    self.vocab.prompt(),
                    &mut progress,
                )
                .await?
        };
        lat.upload = upload;
        lat.first_token = first_token;
        lat.done = stopped.elapsed();

        let text =
            inject::prepare_text(&self.vocab.apply(&text), self.config.inject.trailing_space);
        lat.chars = text.chars().count();
        if text.is_empty() {
            log::info!("transcription vide");
            return Ok(());
        }

        let _guard = self.inject_lock.lock().await;
        let typing = Instant::now();
        let target = capture.target;
        tokio::task::spawn_blocking(move || inject::type_into(target, &text)).await??;
        lat.inject = typing.elapsed();
        lat.total = stopped.elapsed();

        log::info!("{}", lat.summary());
        if let Err(e) = lat.append_csv(&self.metrics_path) {
            log::warn!(
                "écriture de {} impossible : {e}",
                self.metrics_path.display()
            );
        }
        Ok(())
    }
}

fn open_folder(dir: &Path) {
    if let Err(e) = std::process::Command::new("explorer").arg(dir).spawn() {
        log::warn!("ouverture de {} impossible : {e}", dir.display());
    }
}

/// Journal dans `%APPDATA%\Dictia\dictia.log` (plus la console en debug).
/// `DICTIA_LOG=debug` active les détails.
fn init_logging(dir: &Path) {
    let level = match std::env::var("DICTIA_LOG").as_deref() {
        Ok("debug") => LevelFilter::Debug,
        Ok("trace") => LevelFilter::Trace,
        _ => LevelFilter::Info,
    };
    let config = ConfigBuilder::new().add_filter_allow_str("dictia").build();

    let path = dir.join(LOG_FILE);
    if fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX_BYTES) {
        let _ = fs::rename(&path, dir.join(format!("{LOG_FILE}.old")));
    }
    let mut loggers: Vec<Box<dyn SharedLogger>> = Vec::new();
    if let Ok(file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        loggers.push(WriteLogger::new(level, config.clone(), file));
    }
    if cfg!(debug_assertions) {
        loggers.push(SimpleLogger::new(level, config));
    }
    let _ = CombinedLogger::init(loggers);

    // En release, panic = "abort" : on consigne la cause avant de quitter.
    std::panic::set_hook(Box::new(|info| {
        log::error!("panique : {info}");
    }));
}
