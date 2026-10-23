//! Contrôleur de dictée : capture → transcription (en direct ou à la fin)
//! → remplacements → frappe, plus l'état partagé avec la boucle principale.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tokio::sync::{mpsc, Mutex};

use crate::audio::capture::{LiveAudio, Microphone};
use crate::audio::{duration_of, flac, vad, TARGET_RATE};
use crate::config::Config;
use crate::dictation::{self, Effect, Phase};
use crate::history::{self, Entry, Problem};
use crate::hotkey::Action;
use crate::inject;
use crate::live_typing;
use crate::metrics::Latencies;
use crate::platform::{self, WindowHandle};
use crate::transcription::realtime::{self, RealtimeConfig};
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
    /// Dernier problème à montrer dans la fenêtre, pas encore lu.
    problem: std::sync::Mutex<Option<Problem>>,
}

impl Status {
    pub fn new(main_thread: u32) -> Self {
        Self {
            main_thread,
            mic_level: Arc::new(AtomicU32::new(0)),
            recording: AtomicBool::new(false),
            processing: AtomicUsize::new(0),
            error: AtomicBool::new(false),
            problem: std::sync::Mutex::new(None),
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

    /// Problème signalé depuis la dernière lecture.
    pub fn take_problem(&self) -> Option<Problem> {
        self.problem.lock().ok()?.take()
    }

    /// Note un problème pour la fenêtre (sans changer l'état de l'icône).
    fn report(&self, text: String) {
        if let Ok(mut p) = self.problem.lock() {
            *p = Some(Problem {
                at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64),
                text,
            });
        }
        self.notify();
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
        self.report(format!("Erreur ({context}) : {err}"));
    }
}

/// Enregistrement en cours.
struct Session {
    mic: Arc<Microphone>,
    target: Option<WindowHandle>,
    started: Instant,
    mode: &'static str,
    live: Option<Live>,
}

/// Dictée enregistrée, à transcrire.
struct Capture {
    samples: Vec<f32>,
    target: Option<WindowHandle>,
    recording: Duration,
    stopped: Instant,
    mode: &'static str,
    live: Option<Live>,
}

/// Transcription en direct qui accompagne une dictée.
struct Live {
    task: tokio::task::JoinHandle<Result<String>>,
    marks: Arc<std::sync::Mutex<LiveMarks>>,
    typer: Option<Typer>,
}

/// Instants des jalons de la session en direct, pour les latences.
#[derive(Default)]
struct LiveMarks {
    commit: Option<Instant>,
    first_delta: Option<Instant>,
}

impl Live {
    fn spawn(
        config: RealtimeConfig,
        audio: mpsc::UnboundedReceiver<Vec<i16>>,
        typer: Option<Typer>,
    ) -> Self {
        let marks = Arc::new(std::sync::Mutex::new(LiveMarks::default()));
        let recorder = Arc::clone(&marks);
        let to_typer = typer.as_ref().map(|t| t.tx.clone());
        let task = tokio::spawn(realtime::run_session(config, audio, move |p| {
            if let Ok(mut m) = recorder.lock() {
                match p {
                    Progress::ResponseStarted => m.commit = Some(Instant::now()),
                    Progress::Delta(_) => {
                        m.first_delta.get_or_insert_with(Instant::now);
                    }
                }
            }
            if let (Progress::Delta(d), Some(tx)) = (p, &to_typer) {
                let _ = tx.send(TyperMsg::Delta(d.to_owned()));
            }
        }));
        Self { task, marks, typer }
    }
}

/// Ordre transmis à la frappe en direct.
enum TyperMsg {
    /// Fragment de transcription reçu pendant la dictée.
    Delta(String),
    /// Texte final (remplacements appliqués) : dernière correction.
    Finish(String),
    /// Rien à garder : efface ce qui a été tapé.
    Erase,
}

/// Frappe au fil de la transcription en direct (moteur realtime).
struct Typer {
    tx: mpsc::UnboundedSender<TyperMsg>,
    task: tokio::task::JoinHandle<Result<Duration>>,
}

impl Typer {
    fn spawn(target: Option<WindowHandle>, vocab: Arc<Vocab>, lock: Arc<Mutex<()>>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run_typer(target, vocab, lock, rx));
        Self { tx, task }
    }

    /// Envoie la dernière consigne et attend que la frappe soit terminée.
    /// Renvoie le temps passé à attendre le relâchement des touches.
    async fn finish(self, last: TyperMsg) -> Result<Duration> {
        let _ = self.tx.send(last);
        self.task.await?
    }
}

async fn run_typer(
    target: Option<WindowHandle>,
    vocab: Arc<Vocab>,
    lock: Arc<Mutex<()>>,
    mut rx: mpsc::UnboundedReceiver<TyperMsg>,
) -> Result<Duration> {
    let mut keys_wait = Duration::ZERO;
    let mut typed = String::new();
    let mut raw = String::new();
    // Pris à la première frappe seulement : une dictée précédente encore en
    // cours de traitement tape son texte avant celui-ci.
    let mut guard = None;
    while let Some(msg) = rx.recv().await {
        let (wanted, last) = match msg {
            TyperMsg::Delta(d) => {
                raw.push_str(&d);
                let applied = vocab.apply(&raw);
                (live_typing::stable_prefix(&applied).to_owned(), false)
            }
            TyperMsg::Finish(text) => (text, true),
            TyperMsg::Erase => (String::new(), true),
        };
        if !last && !inject::can_type_live(target) {
            continue;
        }
        let (erase, insert) = live_typing::diff(&typed, &wanted);
        if erase > 0 || !insert.is_empty() {
            if guard.is_none() {
                guard = Some(lock.lock().await);
            }
            let insert = insert.to_owned();
            keys_wait += tokio::task::spawn_blocking(move || {
                inject::apply_correction(target, erase, &insert, last)
            })
            .await??;
            typed = wanted;
        }
        if last {
            break;
        }
    }
    Ok(keys_wait)
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
    /// Micro gardé ouvert entre les dictées (réglage « Laisser le micro
    /// ouvert ») ; vide sinon.
    pub kept_mic: Arc<std::sync::Mutex<Option<Arc<Microphone>>>>,
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

    /// Moteur standard (toujours prêt, sert aussi de repli) et, si le moteur
    /// realtime est choisi, la configuration de sa session.
    fn engines(&self) -> Option<(Arc<dyn Backend>, Option<RealtimeConfig>)> {
        let shared = self.shared.read().ok()?;
        let backend = shared.backend.clone()?;
        let config = &shared.config;
        let api = &config.api;
        let provider = config.provider();
        let realtime = config
            .uses_realtime()
            .then(|| shared.api_key.clone())
            .flatten()
            .map(|api_key| {
                let dialect = provider.dialect();
                let model = config.realtime_model(provider);
                let base_url = config.endpoint(provider, |_| None).base_url;
                RealtimeConfig {
                    dialect,
                    url: realtime::endpoint(dialect, &base_url, &model),
                    api_key,
                    model,
                    language: api.language_code().to_owned(),
                    prompt: shared.vocab.prompt().to_owned(),
                    keywords: shared.vocab.terms().to_vec(),
                    timeout: Duration::from_secs(api.timeout_secs.max(1)),
                }
            });
        Some((backend, realtime))
    }

    /// Ouvre un micro ; ses erreurs n'interrompent que la dictée en cours.
    fn open_microphone(&self, device: &str) -> Result<Microphone> {
        let status = Arc::clone(&self.status);
        Microphone::open(device, Arc::clone(&self.status.mic_level), move |e| {
            if status.recording.swap(false, Ordering::SeqCst) {
                status.fail("micro", &e);
            } else {
                log::warn!("micro : {e}");
            }
        })
    }

    /// Micro de la dictée : celui gardé ouvert s'il est prêt, sinon un
    /// nouveau (gardé ouvert ensuite si le réglage le demande).
    fn microphone(&self) -> Result<Arc<Microphone>> {
        let (device, keep) = self.mic_settings();
        let mut kept = self
            .kept_mic
            .lock()
            .map_err(|_| anyhow::anyhow!("micro indisponible"))?;
        if let Some(mic) = kept.as_ref() {
            if keep && mic.is_alive() && mic.device() == device {
                return Ok(Arc::clone(mic));
            }
        }
        let mic = Arc::new(self.open_microphone(&device)?);
        *kept = keep.then(|| Arc::clone(&mic));
        Ok(mic)
    }

    /// Ouvre, rouvre ou ferme le micro gardé ouvert selon les réglages.
    pub fn sync_microphone(&self) {
        let (device, keep) = self.mic_settings();
        let Ok(mut kept) = self.kept_mic.lock() else {
            return;
        };
        let ready = kept
            .as_ref()
            .is_some_and(|m| m.is_alive() && m.device() == device);
        if !keep {
            *kept = None;
        } else if !ready {
            *kept = match self.open_microphone(&device) {
                Ok(mic) => Some(Arc::new(mic)),
                Err(e) => {
                    log::warn!("micro : {e:#}");
                    None
                }
            };
        }
    }

    fn mic_settings(&self) -> (String, bool) {
        self.shared.read().map_or((String::new(), false), |s| {
            (s.config.audio.device.clone(), s.config.audio.keep_mic_open)
        })
    }

    /// Début : mémorise la fenêtre, ouvre le micro et la connexion (session
    /// en direct ou préchauffage du moteur standard).
    fn start(&self, mode: &'static str) -> Option<Session> {
        let started = Instant::now();
        let Some((backend, realtime)) = self.engines() else {
            let _ = self.events.send(Event::NeedApiKey);
            self.status.notify();
            return None;
        };
        let target = platform::foreground_window();
        let (live, live_audio) = match realtime {
            Some(config) => {
                let (tx, rx) = mpsc::unbounded_channel();
                let rate = realtime::rate(config.dialect);
                let typer = self.shared.read().ok().and_then(|s| {
                    s.config.inject.live_typing.then(|| {
                        Typer::spawn(target, Arc::clone(&s.vocab), Arc::clone(&self.inject_lock))
                    })
                });
                (
                    Some(Live::spawn(config, rx, typer)),
                    Some(LiveAudio { tx, rate }),
                )
            }
            None => {
                tokio::spawn(async move { backend.warm_up().await });
                (None, None)
            }
        };

        match self.microphone() {
            Ok(mic) => {
                mic.begin(live_audio);
                self.status.set_recording(true);
                log::info!(
                    "enregistrement démarré ({mode}, moteur {})",
                    if live.is_some() {
                        "realtime"
                    } else {
                        "standard"
                    }
                );
                Some(Session {
                    mic,
                    target,
                    started,
                    mode,
                    live,
                })
            }
            Err(e) => {
                if let Some(live) = live {
                    live.task.abort();
                }
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
        let samples = session.mic.end();
        // Micro ouvert pour cette seule dictée : il est fermé ici.
        drop(session.mic);
        let samples = match samples {
            Ok(s) => s,
            Err(e) => {
                // Signalé par le rappel d'erreur du micro, sauf s'il a échoué
                // avant le début de l'enregistrement.
                if !self.status.error.load(Ordering::SeqCst) {
                    self.status.fail("micro", &format!("{e:#}"));
                }
                if let Some(live) = session.live {
                    live.task.abort();
                }
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
                live: session.live,
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
        let provider = config.provider();
        let standard_model = config.endpoint(provider, |_| None).model;
        let mut lat = Latencies {
            backend: if capture.live.is_some() {
                "realtime"
            } else {
                backend.name()
            },
            provider: provider.name(),
            model: if capture.live.is_some() {
                config.realtime_model(provider)
            } else {
                standard_model.clone()
            },
            recording: capture.recording,
            ..Default::default()
        };

        // Détection de parole locale : évite d'injecter une « transcription »
        // de silence, quel que soit le moteur.
        let audio_cfg = config.audio.clone();
        let samples = capture.samples;
        let (samples, speech) = tokio::task::spawn_blocking(move || {
            let range = vad::speech_bounds(&samples, TARGET_RATE, &audio_cfg);
            (samples, range)
        })
        .await?;
        let mut live = capture.live;
        let typer = live.as_mut().and_then(|l| l.typer.take());
        let Some(speech) = speech else {
            if let Some(live) = live {
                live.task.abort();
            }
            if let Some(typer) = typer {
                typer.finish(TyperMsg::Erase).await?;
            }
            log::info!("aucune parole détectée, rien à transcrire");
            return Ok(());
        };
        lat.speech = duration_of(speech.len(), TARGET_RATE);

        let text = match live {
            Some(live) => match finish_live(live, stopped, &mut lat).await {
                Ok(text) => text,
                Err(e) => {
                    log::warn!("transcription en direct indisponible, repli sur le moteur standard : {e:#}");
                    self.status.report(format!(
                        "Realtime indisponible, dictée transcrite en Standard : {e:#}"
                    ));
                    lat.backend = "realtime>standard";
                    lat.model = standard_model;
                    transcribe_file(
                        &*backend,
                        &vocab,
                        samples[speech].to_vec(),
                        stopped,
                        &mut lat,
                    )
                    .await?
                }
            },
            None => {
                transcribe_file(
                    &*backend,
                    &vocab,
                    samples[speech].to_vec(),
                    stopped,
                    &mut lat,
                )
                .await?
            }
        };
        lat.done = stopped.elapsed();

        let text = if vocab.is_prompt_echo(&text) {
            log::info!("transcription ignorée : le modèle a recopié le prompt (pas de parole ?)");
            String::new()
        } else {
            // Avant les remplacements : la ponctuation dictée (« point virgule »)
            // reste.
            let text = inject::strip_short_punctuation(&text, config.inject.short_text_words);
            inject::prepare_text(&vocab.apply(&text), config.inject.trailing_space)
        };
        lat.chars = text.chars().count();

        let target = capture.target;
        let typing = Instant::now();
        match typer {
            // Frappe en direct : il ne reste qu'à corriger ou compléter.
            Some(typer) => {
                let last = if text.is_empty() {
                    TyperMsg::Erase
                } else {
                    TyperMsg::Finish(text.clone())
                };
                lat.keys_wait = typer.finish(last).await?;
            }
            None if !text.is_empty() => {
                let _guard = self.inject_lock.lock().await;
                let typed = text.clone();
                lat.keys_wait =
                    tokio::task::spawn_blocking(move || inject::type_into(target, &typed))
                        .await??;
            }
            None => {}
        }
        if text.is_empty() {
            log::info!("transcription vide");
            return Ok(());
        }
        lat.inject = typing.elapsed();
        lat.total = stopped.elapsed();
        let app = tokio::task::spawn_blocking(move || {
            target
                .and_then(platform::window_exe)
                .map(|exe| history::friendly_app_name(&exe))
                .unwrap_or_default()
        })
        .await?;

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

/// Attend le texte de la session en direct (l'audio est déjà parti).
async fn finish_live(live: Live, stopped: Instant, lat: &mut Latencies) -> Result<String> {
    let text = live.task.await??;
    if let Ok(marks) = live.marks.lock() {
        let since_stop = |t: Instant| t.saturating_duration_since(stopped);
        lat.upload = marks.commit.map(since_stop);
        // Un fragment reçu avant l'arrêt compte pour 0 ms.
        lat.first_token = marks.first_delta.map(since_stop);
    }
    Ok(text)
}

/// Moteur standard : FLAC de la parole puis requête de transcription.
async fn transcribe_file(
    backend: &dyn Backend,
    vocab: &Vocab,
    speech: Vec<f32>,
    stopped: Instant,
    lat: &mut Latencies,
) -> Result<String> {
    let encoding = Instant::now();
    let flac_bytes =
        tokio::task::spawn_blocking(move || flac::encode_mono(&speech, TARGET_RATE)).await??;
    lat.encode = encoding.elapsed();
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
                vocab.guidance(),
                &mut progress,
            )
            .await?
    };
    lat.upload = upload;
    lat.first_token = first_token;
    Ok(text)
}

/// Attend `deadline`, ou indéfiniment s'il n'y en a pas.
async fn sleep_until_opt(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}
