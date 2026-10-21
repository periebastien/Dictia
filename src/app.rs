//! Assemblage de l'application Windows.
//!
//! - Thread principal : boucle de messages Win32 (raccourcis, icône,
//!   indicateur, fenêtre) et application des réglages modifiés.
//! - Runtime tokio : [`Controller`] qui enchaîne capture → transcription →
//!   frappe.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{mpsc as std_mpsc, Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use simplelog::{
    CombinedLogger, ConfigBuilder, LevelFilter, SharedLogger, SimpleLogger, WriteLogger,
};
use tokio::sync::{mpsc, Mutex};
use tray_icon::menu::MenuEvent;
use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

use crate::config::{self, Config, CONFIG_FILE};
use crate::controller::{Controller, Event, Shared, SharedRef, Status};
use crate::history::{self, Entry, HISTORY_FILE, MAX_ENTRIES};
use crate::hotkey::{self, Registration};
use crate::inject;
use crate::metrics::METRICS_FILE;
use crate::overlay::{self, Animation};
use crate::platform::{self, HostWindow, OverlayWindow, UiRequest};
use crate::settings::{self, Change, VocabEdit};
use crate::stats::{self, ModelStats};
use crate::transcription::standard::StandardBackend;
use crate::transcription::Backend;
use crate::tray::{Tray, TrayState, MENU_OPEN_CONFIG, MENU_OPEN_UI, MENU_QUIT};
use crate::ui::window::UiWindow;
use crate::ui::{self, Command, Message, SettingsView, Tab};
use crate::vocab::{self, Vocab, VOCAB_FILE};

const LOG_FILE: &str = "dictia.log";
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

pub fn run() -> Result<()> {
    platform::enable_dpi_awareness();
    let dir = config::config_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("création de {}", dir.display()))?;
    init_logging(&dir);
    log::info!("Dictia {} démarre", env!("CARGO_PKG_VERSION"));

    let Some(_instance) = platform::acquire_single_instance() else {
        // Déjà lancé : on affiche plutôt la fenêtre de l'instance existante.
        if !platform::signal_running_instance() {
            platform::show_warning(
                "Dictia est déjà lancé : son icône se trouve dans la zone de notification.",
            );
        }
        return Ok(());
    };
    let _host = HostWindow::new()?;

    let config = Config::load_or_create(&dir)?;
    let vocab_file = vocab::load_file_or_create(&dir)?;
    let vocab = Vocab::from_file(&vocab_file)?;
    log::info!(
        "configuration : {:?} ; fournisseur {} ; prompt de {} caractères, {} remplacement(s)",
        config.api,
        config.provider().name(),
        vocab.prompt().chars().count(),
        vocab.rule_count()
    );
    let (backend, api_key) = make_backend(&config)?;
    let needs_key = backend.is_none();

    let status = Arc::new(Status::new(platform::current_thread_id()));
    let shared: SharedRef = Arc::new(RwLock::new(Shared {
        config: config.clone(),
        vocab_file,
        vocab: Arc::new(vocab),
        api_key,
        backend,
    }));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("dictia-worker")
        .enable_all()
        .build()
        .context("démarrage du runtime")?;
    let (actions_tx, actions_rx) = mpsc::unbounded_channel();
    let (events_tx, events_rx) = std_mpsc::channel();
    let (commands_tx, commands_rx) = std_mpsc::channel();

    let tray = Tray::new(&hotkey::describe(&config.hotkey))?;
    {
        let dir = dir.clone();
        let main_thread = status.main_thread;
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == MENU_QUIT {
                platform::post_quit(main_thread);
            } else if event.id == MENU_OPEN_UI {
                platform::request_ui(UiRequest::Show);
                platform::post_wake(main_thread);
            } else if event.id == MENU_OPEN_CONFIG {
                open_folder(&dir);
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                platform::request_ui(UiRequest::Show);
                platform::post_wake(main_thread);
            }
        }));
    }

    let mut hotkeys = Registration::new(&[], move |action| {
        let _ = actions_tx.send(action);
    })?;
    let mut startup_tab = needs_key.then_some(Tab::Reglages);
    match hotkey::bindings(&config.hotkey).and_then(|b| hotkeys.replace(&b)) {
        Ok(()) => log::info!("prêt : {} pour dicter", hotkey::describe(&config.hotkey)),
        Err(e) => {
            log::error!("{e:#}");
            platform::show_error(&format!(
                "{e:#}\n\nChoisissez un autre raccourci dans l'onglet Raccourcis de Dictia."
            ));
            startup_tab = Some(Tab::Raccourcis);
        }
    }

    let controller = Controller {
        shared: Arc::clone(&shared),
        status: Arc::clone(&status),
        events: events_tx,
        inject_lock: Arc::new(Mutex::new(())),
        metrics_path: dir.join(METRICS_FILE),
        history_path: dir.join(HISTORY_FILE),
    };
    runtime.spawn(controller.run(actions_rx));

    let mut app = MainLoop {
        indicator: Indicator::new(config.indicator.enabled),
        history: history::load(&dir.join(HISTORY_FILE)),
        dir,
        shared,
        status,
        tray,
        hotkeys,
        ui: None,
        commands_tx,
        commands_rx,
        events_rx,
        shown_status: None,
        pending_tab: None,
    };
    if let Some(tab) = startup_tab {
        app.open_ui(Some(tab));
    }
    platform::run_message_loop(|| app.tick());

    log::info!("arrêt de Dictia");
    drop(app);
    runtime.shutdown_timeout(Duration::from_millis(500));
    Ok(())
}

type BackendAndKey = (Option<Arc<dyn Backend>>, Option<String>);

/// Moteur du fournisseur actif, et sa clé. Aucun moteur si la clé manque
/// (sauf serveur local, pour qui elle est facultative).
fn make_backend(config: &Config) -> Result<BackendAndKey> {
    let provider = config.provider();
    let endpoint = config.endpoint(provider, config::process_env);
    if provider.requires_key() && endpoint.api_key.is_none() {
        return Ok((None, None));
    }
    let backend: Arc<dyn Backend> = Arc::new(StandardBackend::new(
        &endpoint,
        config.api.language_code(),
        config.api.timeout_secs,
    )?);
    Ok((Some(backend), endpoint.api_key))
}

/// État du thread principal.
struct MainLoop {
    dir: PathBuf,
    shared: SharedRef,
    status: Arc<Status>,
    tray: Tray,
    indicator: Indicator,
    hotkeys: Registration,
    ui: Option<UiWindow>,
    commands_tx: std_mpsc::Sender<Command>,
    commands_rx: std_mpsc::Receiver<Command>,
    events_rx: std_mpsc::Receiver<Event>,
    history: Vec<Entry>,
    shown_status: Option<TrayState>,
    /// Onglet à afficher quand la page sera prête.
    pending_tab: Option<Tab>,
}

impl MainLoop {
    /// Appelé après chaque message Win32.
    fn tick(&mut self) {
        let state = self.status.get();
        self.tray.set_state(state);
        self.indicator
            .update(Animation::for_state(state), self.status.mic_rms());
        if self.shown_status != Some(state) {
            self.shown_status = Some(state);
            self.send(&Message::Status {
                status: ui::status_name(state),
            });
        }

        match platform::take_ui_request() {
            Some(UiRequest::Show) => self.open_ui(None),
            Some(UiRequest::Close) => self.ui = None,
            None => {}
        }

        while let Ok(event) = self.events_rx.try_recv() {
            match event {
                Event::History(entry) => {
                    self.history.insert(0, entry.clone());
                    self.history.truncate(MAX_ENTRIES);
                    self.send(&Message::HistoryAdd { entry });
                    if self.ui.is_some() {
                        self.send(&Message::Stats {
                            stats: self.stats(),
                        });
                    }
                }
                Event::NeedApiKey => {
                    self.open_ui(Some(Tab::Reglages));
                    self.send(&Message::error("Ajoutez votre clé API pour dicter"));
                }
            }
        }

        while let Ok(command) = self.commands_rx.try_recv() {
            self.handle(command);
        }
    }

    fn send(&self, message: &Message) {
        if let Some(ui) = &self.ui {
            ui.send(message);
        }
    }

    fn open_ui(&mut self, tab: Option<Tab>) {
        if let Some(ui) = &self.ui {
            ui.show();
            if let Some(tab) = tab {
                ui.send(&Message::Tab { tab });
            }
            return;
        }
        self.pending_tab = tab;
        match UiWindow::open(
            self.dir.join("webview"),
            self.commands_tx.clone(),
            self.status.main_thread,
        ) {
            Ok(ui) => self.ui = Some(ui),
            Err(e) => {
                log::error!("{e:#}");
                platform::show_error(&format!("{e:#}"));
            }
        }
    }

    fn settings_view(&self) -> SettingsView {
        let shared = self.shared.read().expect("réglages");
        SettingsView::new(
            &shared.config,
            config::process_env,
            platform::run_at_startup(),
        )
    }

    /// Statistiques de latence à partir de `latences.csv`.
    fn stats(&self) -> Vec<ModelStats> {
        fs::read_to_string(self.dir.join(METRICS_FILE))
            .map(|csv| stats::compute(&csv))
            .unwrap_or_default()
    }

    fn vocab_view(&self) -> VocabEdit {
        settings::vocab_view(&self.shared.read().expect("réglages").vocab_file)
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Ready => {
                let message = Message::Snapshot {
                    status: ui::status_name(self.status.get()),
                    settings: self.settings_view(),
                    vocab: self.vocab_view(),
                    history: self.history.clone(),
                    stats: self.stats(),
                    tab: self.pending_tab.take(),
                };
                self.send(&message);
            }
            Command::Drag => {
                if let Some(ui) = &self.ui {
                    ui.begin_drag();
                }
            }
            Command::Minimize => {
                if let Some(ui) = &self.ui {
                    ui.minimize();
                }
            }
            Command::Close => self.ui = None,
            Command::Change { change } => match self.apply_change(&change) {
                Ok(()) => {
                    self.send(&Message::Settings {
                        settings: self.settings_view(),
                    });
                    self.send(&Message::toast("Enregistré"));
                }
                Err(e) => {
                    log::warn!("réglage refusé : {e:#}");
                    self.send(&Message::Settings {
                        settings: self.settings_view(),
                    });
                    self.send(&Message::error(format!("{e:#}")));
                }
            },
            Command::Vocab { vocab } => {
                if let Err(e) = self.apply_vocab(&vocab) {
                    log::warn!("vocabulaire refusé : {e:#}");
                    self.send(&Message::error(format!("{e:#}")));
                    self.send(&Message::Vocab {
                        vocab: self.vocab_view(),
                    });
                }
            }
            Command::Startup { value } => {
                match platform::set_run_at_startup(value) {
                    Ok(()) => self.send(&Message::toast(if value {
                        "Dictia démarrera avec Windows"
                    } else {
                        "Démarrage automatique désactivé"
                    })),
                    Err(e) => self.send(&Message::error(format!("{e:#}"))),
                }
                self.send(&Message::Settings {
                    settings: self.settings_view(),
                });
            }
            Command::Copy { text } => match platform::set_clipboard_text(&text) {
                Ok(()) => self.send(&Message::toast("Copié dans le presse-papier")),
                Err(e) => self.send(&Message::error(format!("Copie impossible : {e}"))),
            },
            Command::Reinsert { text } => {
                // La fenêtre se retire : Windows rend la main à l'application
                // précédente, où le texte est tapé.
                if let Some(ui) = &self.ui {
                    ui.hide();
                }
                let trailing = self
                    .shared
                    .read()
                    .map(|s| s.config.inject.trailing_space)
                    .unwrap_or(true);
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(300));
                    let text = inject::prepare_text(&text, trailing);
                    if let Err(e) = inject::type_into(platform::foreground_window(), &text) {
                        log::error!("réinsertion : {e:#}");
                        platform::beep_error();
                    }
                });
            }
            Command::Capture { active } => self.hotkeys.suspend(active),
            Command::OpenFolder => open_folder(&self.dir),
        }
    }

    /// Écrit le réglage dans `config.toml` puis l'applique immédiatement.
    fn apply_change(&mut self, change: &Change) -> Result<()> {
        let path = self.dir.join(CONFIG_FILE);
        let old_text = fs::read_to_string(&path).unwrap_or_default();
        let current = self.shared.read().expect("réglages").config.clone();
        let new_text = settings::apply_config_change(&old_text, &current, change)?;
        let config = Config::parse(&new_text)?;
        fs::write(&path, &new_text).with_context(|| format!("écriture de {}", path.display()))?;

        if matches!(change, Change::Shortcut { .. } | Change::PushToTalk { .. }) {
            if let Err(e) = hotkey::bindings(&config.hotkey).and_then(|b| self.hotkeys.replace(&b))
            {
                let _ = fs::write(&path, &old_text);
                return Err(e);
            }
            self.tray.set_shortcuts(&hotkey::describe(&config.hotkey));
        }
        if let Change::Indicator { value } = change {
            self.indicator.set_enabled(*value);
        }
        let mut shared = self.shared.write().expect("réglages");
        if matches!(
            change,
            Change::Provider { .. }
                | Change::ProviderKey { .. }
                | Change::ProviderModel { .. }
                | Change::ProviderUrl { .. }
                | Change::Language { .. }
        ) {
            let (backend, api_key) = make_backend(&config)?;
            shared.backend = backend;
            shared.api_key = api_key;
        }
        shared.config = config;
        Ok(())
    }

    /// Écrit le vocabulaire dans `vocab.toml` puis l'applique immédiatement.
    fn apply_vocab(&mut self, edit: &VocabEdit) -> Result<()> {
        let path = self.dir.join(VOCAB_FILE);
        let old_text = fs::read_to_string(&path).unwrap_or_default();
        let current = self.shared.read().expect("réglages").vocab_file.clone();
        let new_text = settings::apply_vocab_edit(&old_text, &current, edit)?;
        let file: vocab::VocabFile = toml::from_str(&new_text)?;
        let vocab = Vocab::from_file(&file)?;
        fs::write(&path, &new_text).with_context(|| format!("écriture de {}", path.display()))?;
        let mut shared = self.shared.write().expect("réglages");
        shared.vocab_file = file;
        shared.vocab = Arc::new(vocab);
        Ok(())
    }
}

/// Animation affichée à côté du curseur (thread principal uniquement).
struct Indicator {
    window: Option<OverlayWindow>,
    scale: f32,
    shown: Option<Animation>,
    since: Instant,
    level: f32,
}

impl Indicator {
    fn new(enabled: bool) -> Self {
        let mut indicator = Self {
            window: None,
            scale: platform::system_scale(),
            shown: None,
            since: Instant::now(),
            level: 0.0,
        };
        indicator.set_enabled(enabled);
        indicator
    }

    fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.window = None;
            self.shown = None;
            return;
        }
        if self.window.is_none() {
            let (w, h) = overlay::size(self.scale);
            self.window = OverlayWindow::new(w, h)
                .map_err(|e| log::warn!("indicateur indisponible : {e}"))
                .ok();
        }
    }

    fn update(&mut self, animation: Option<Animation>, mic_rms: f32) {
        let Some(window) = &mut self.window else {
            return;
        };
        let Some(animation) = animation else {
            if self.shown.take().is_some() {
                window.set_animating(false, overlay::FRAME_MS);
                window.hide();
            }
            return;
        };
        if self.shown != Some(animation) {
            self.shown = Some(animation);
            self.since = Instant::now();
            self.level = 0.0;
            window.set_animating(true, overlay::FRAME_MS);
        }
        // Montée immédiate, descente douce : les barres restent lisibles.
        let target = overlay::level_from_rms(mic_rms);
        self.level = if target > self.level {
            target
        } else {
            self.level * 0.85 + target * 0.15
        };
        let image = overlay::render(
            animation,
            self.since.elapsed().as_secs_f32(),
            self.level,
            self.scale,
        );
        let (x, y) = platform::text_cursor_or_pointer();
        let offset = |v: i32| (v as f32 * self.scale).round() as i32;
        window.show(
            x + offset(overlay::OFFSET.0),
            y + offset(overlay::OFFSET.1),
            &image,
        );
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
