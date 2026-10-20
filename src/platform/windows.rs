//! Win32 : fenêtre active, injection clavier, boucle de messages, instance
//! unique et boîtes de dialogue.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, WPARAM,
};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::Threading::{AttachThreadInput, CreateMutexW, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RETURN,
    VK_RWIN, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
    IsIconic, IsWindow, MessageBoxW, PostThreadMessageW, SetForegroundWindow, ShowWindow,
    TranslateMessage, MB_ICONERROR, MB_ICONWARNING, MB_OK, MSG, SW_RESTORE, WM_APP,
};

use crate::inject::Keystroke;

/// Fenêtre Win32 transportable entre threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowHandle(isize);

impl WindowHandle {
    fn hwnd(self) -> HWND {
        HWND(self.0 as *mut _)
    }
}

/// Fenêtre au premier plan (celle où l'utilisateur tape).
pub fn foreground_window() -> Option<WindowHandle> {
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.is_invalid()).then_some(WindowHandle(hwnd.0 as isize))
}

/// Remet `target` au premier plan. Renvoie `true` si elle l'est.
pub fn focus_window(target: WindowHandle) -> bool {
    let hwnd = target.hwnd();
    unsafe {
        if GetForegroundWindow() == hwnd {
            return true;
        }
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if !SetForegroundWindow(hwnd).as_bool() {
            // Windows refuse le changement de premier plan aux processus
            // d'arrière-plan : on s'attache temporairement à la file
            // d'entrée de la fenêtre active pour en hériter le droit.
            let current = GetForegroundWindow();
            let fg_thread = GetWindowThreadProcessId(current, None);
            let me = GetCurrentThreadId();
            let attached = fg_thread != 0
                && fg_thread != me
                && AttachThreadInput(me, fg_thread, true).as_bool();
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            if attached {
                let _ = AttachThreadInput(me, fg_thread, false);
            }
        }
        // Le changement est asynchrone : on attend qu'il soit effectif.
        let deadline = Instant::now() + Duration::from_millis(200);
        while GetForegroundWindow() != hwnd {
            if Instant::now() > deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    true
}

/// Attend que Ctrl, Alt, Maj et Win soient relâchés (au plus `timeout`).
pub fn wait_modifiers_released(timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mods = [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN];
    while mods
        .iter()
        .any(|vk| unsafe { GetAsyncKeyState(i32::from(vk.0)) } < 0)
    {
        if Instant::now() > deadline {
            log::warn!("modificateurs toujours enfoncés, frappe quand même");
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn key_input(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn inputs_for(keys: &[Keystroke]) -> Vec<INPUT> {
    let mut inputs = Vec::with_capacity(keys.len() * 2);
    for key in keys {
        let (vk, scan, flags) = match *key {
            Keystroke::Unit(u) => (VIRTUAL_KEY(0), u, KEYEVENTF_UNICODE),
            Keystroke::Enter => (VK_RETURN, 0, KEYBD_EVENT_FLAGS(0)),
            Keystroke::Tab => (VK_TAB, 0, KEYBD_EVENT_FLAGS(0)),
        };
        inputs.push(key_input(vk, scan, flags));
        inputs.push(key_input(vk, scan, flags | KEYEVENTF_KEYUP));
    }
    inputs
}

/// Envoie les frappes à la fenêtre qui a le focus.
pub fn send_keystrokes(keys: &[Keystroke]) -> Result<()> {
    // Par lots : un seul appel reste atomique vis-à-vis des frappes de
    // l'utilisateur, mais des lots modérés évitent de saturer certaines
    // applications (Electron, terminaux).
    const BATCH: usize = 512;
    let inputs = inputs_for(keys);
    for batch in inputs.chunks(BATCH) {
        let sent = unsafe { SendInput(batch, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != batch.len() {
            bail!(
                "SendInput n'a envoyé que {sent}/{} événements (fenêtre administrateur ?)",
                batch.len()
            );
        }
    }
    Ok(())
}

/// Message de réveil de la boucle principale (état à rafraîchir).
const WM_DICTIA_WAKE: u32 = WM_APP + 1;
/// Demande d'arrêt de la boucle principale.
const WM_DICTIA_QUIT: u32 = WM_APP + 2;

pub fn current_thread_id() -> u32 {
    unsafe { GetCurrentThreadId() }
}

/// Réveille la boucle de messages du thread `thread_id`.
pub fn post_wake(thread_id: u32) {
    let _ = unsafe { PostThreadMessageW(thread_id, WM_DICTIA_WAKE, WPARAM(0), LPARAM(0)) };
}

/// Demande l'arrêt de la boucle de messages du thread `thread_id`.
pub fn post_quit(thread_id: u32) {
    let _ = unsafe { PostThreadMessageW(thread_id, WM_DICTIA_QUIT, WPARAM(0), LPARAM(0)) };
}

/// Boucle de messages Win32 du thread courant (indispensable au raccourci
/// global et à l'icône). `on_tick` est appelé après chaque message.
pub fn run_message_loop(mut on_tick: impl FnMut()) {
    let mut msg = MSG::default();
    loop {
        let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if ret.0 <= 0 {
            break; // WM_QUIT ou erreur
        }
        if msg.hwnd.is_invalid() && msg.message == WM_DICTIA_QUIT {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        on_tick();
    }
}

/// Verrou d'instance unique, libéré à la destruction.
pub struct SingleInstance(HANDLE);

impl Drop for SingleInstance {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// `None` si Dictia tourne déjà pour cet utilisateur.
pub fn acquire_single_instance() -> Option<SingleInstance> {
    let name = HSTRING::from("Local\\Dictia.SingleInstance");
    let handle = unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) }.ok()?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let _ = unsafe { CloseHandle(handle) };
        return None;
    }
    Some(SingleInstance(handle))
}

fn message_box(text: &str, style: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE) {
    let text = HSTRING::from(text);
    let title = HSTRING::from("Dictia");
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | style,
        );
    }
}

pub fn show_error(text: &str) {
    message_box(text, MB_ICONERROR);
}

pub fn show_warning(text: &str) {
    message_box(text, MB_ICONWARNING);
}

/// Son système d'erreur, pour signaler un échec sans fenêtre.
pub fn beep_error() {
    let _ = unsafe { MessageBeep(MB_ICONERROR) };
}
