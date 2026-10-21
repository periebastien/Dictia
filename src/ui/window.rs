//! Hôte WebView2 de la fenêtre Dictia.

use std::num::NonZeroIsize;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use anyhow::{Context, Result};
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use wry::{WebContext, WebView, WebViewBuilder};

use super::{Command, Message, HEIGHT, HTML, WIDTH};
use crate::platform::{self, AppWindow};

struct Parent(isize);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        // SAFETY : la fenêtre vit aussi longtemps que `UiWindow`.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

/// Fenêtre ouverte. La détruire libère WebView2 (Dictia reste léger quand
/// la fenêtre est fermée).
pub struct UiWindow {
    // Ordre de destruction : la WebView avant sa fenêtre.
    webview: WebView,
    window: AppWindow,
    _context: WebContext,
}

impl UiWindow {
    /// Crée la fenêtre ; chaque commande de la page est transmise à
    /// `commands` puis la boucle principale est réveillée.
    pub fn open(data_dir: PathBuf, commands: Sender<Command>, main_thread: u32) -> Result<Self> {
        let scale = platform::system_scale();
        let window = AppWindow::new(
            (WIDTH as f32 * scale).round() as i32,
            (HEIGHT as f32 * scale).round() as i32,
        )?;
        let mut context = WebContext::new(Some(data_dir));
        let webview = WebViewBuilder::new_with_web_context(&mut context)
            .with_html(HTML)
            .with_background_color((0x1B, 0x1B, 0x21, 0xFF))
            .with_devtools(cfg!(debug_assertions))
            .with_hotkeys_zoom(false)
            .with_navigation_handler(|url| !url.starts_with("http"))
            .with_ipc_handler(move |request| match Command::parse(request.body()) {
                Some(command) => {
                    let _ = commands.send(command);
                    platform::post_wake(main_thread);
                }
                None => log::warn!("message de l'interface ignoré : {}", request.body()),
            })
            .build(&Parent(window.raw()))
            .context(
                "impossible d'afficher la fenêtre : le composant Microsoft Edge WebView2 \
                 est-il installé ?",
            )?;
        window.show();
        let _ = webview.focus();
        Ok(Self {
            webview,
            window,
            _context: context,
        })
    }

    pub fn send(&self, message: &Message) {
        if let Err(e) = self.webview.evaluate_script(&message.to_script()) {
            log::warn!("envoi à l'interface impossible : {e}");
        }
    }

    pub fn show(&self) {
        self.window.show();
        let _ = self.webview.focus();
    }

    pub fn hide(&self) {
        self.window.hide();
    }

    pub fn minimize(&self) {
        self.window.minimize();
    }

    pub fn begin_drag(&self) {
        self.window.begin_drag();
    }
}
