//! Win32 : fenêtre active, injection clavier, boucle de messages, instance
//! unique, boîtes de dialogue et fenêtre transparente de l'indicateur.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, COLORREF, ERROR_ALREADY_EXISTS, HANDLE, HINSTANCE, HWND, LPARAM,
    LRESULT, POINT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
    DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, CreateMutexW, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_LWIN, VK_MENU,
    VK_RETURN, VK_RWIN, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetCursorPos, GetForegroundWindow, GetGUIThreadInfo, GetMessageW, GetWindowThreadProcessId,
    IsIconic, IsWindow, KillTimer, MessageBoxW, PostThreadMessageW, RegisterClassW,
    SetForegroundWindow, SetTimer, ShowWindow, TranslateMessage, UpdateLayeredWindow,
    GUITHREADINFO, MB_ICONERROR, MB_ICONWARNING, MB_OK, MSG, SW_HIDE, SW_RESTORE,
    SW_SHOWNOACTIVATE, ULW_ALPHA, WM_APP, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::inject::Keystroke;

/// Fenêtre Win32 transportable entre threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowHandle(isize);

impl WindowHandle {
    pub(super) fn hwnd(self) -> HWND {
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

/// Ctrl, Alt ou Win enfoncé : une frappe serait prise pour un raccourci.
pub fn shortcut_modifiers_down() -> bool {
    [VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        .any(|vk| unsafe { GetAsyncKeyState(i32::from(vk.0)) } < 0)
}

/// Attend que Ctrl, Alt, Maj et Win soient relâchés (au plus `timeout`).
/// Renvoie le temps attendu.
pub fn wait_modifiers_released(timeout: Duration) -> Duration {
    let start = Instant::now();
    let deadline = start + timeout;
    let mods = [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN];
    while mods
        .iter()
        .any(|vk| unsafe { GetAsyncKeyState(i32::from(vk.0)) } < 0)
    {
        if Instant::now() > deadline {
            log::warn!("modificateurs toujours enfoncés, frappe quand même");
            return start.elapsed();
        }
        thread::sleep(Duration::from_millis(10));
    }
    start.elapsed()
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
            Keystroke::Backspace => (VK_BACK, 0, KEYBD_EVENT_FLAGS(0)),
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

/// Position du point d'insertion de texte de la fenêtre active (bas du
/// curseur clignotant), ou à défaut celle du pointeur de la souris.
/// Beaucoup d'applications (navigateurs, Electron) n'exposent pas leur
/// point d'insertion : c'est alors la souris qui sert de repère.
pub fn text_cursor_or_pointer() -> (i32, i32) {
    unsafe {
        let fg = GetForegroundWindow();
        if !fg.is_invalid() {
            let thread = GetWindowThreadProcessId(fg, None);
            let mut info = GUITHREADINFO {
                cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            if thread != 0
                && GetGUIThreadInfo(thread, &mut info).is_ok()
                && !info.hwndCaret.is_invalid()
            {
                let mut pt = POINT {
                    x: info.rcCaret.left,
                    y: info.rcCaret.bottom,
                };
                if ClientToScreen(info.hwndCaret, &mut pt).as_bool() {
                    return (pt.x, pt.y);
                }
            }
        }
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        (pt.x, pt.y)
    }
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Petite fenêtre transparente, toujours au premier plan, qui laisse passer
/// les clics et ne prend jamais le focus : idéale pour un indicateur.
pub struct OverlayWindow {
    hwnd: HWND,
    mem_dc: HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    bits: *mut u8,
    width: u32,
    height: u32,
    visible: bool,
    animating: bool,
}

const OVERLAY_TIMER: usize = 1;

impl OverlayWindow {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None)?.into();
            let class = w!("DictiaOverlay");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(overlay_proc),
                hInstance: instance,
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE,
                class,
                w!("Dictia"),
                WS_POPUP,
                0,
                0,
                width as i32,
                height as i32,
                None,
                None,
                Some(instance),
                None,
            )?;

            let screen = GetDC(None);
            let mem_dc = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32), // lignes de haut en bas
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(Some(mem_dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let old_bitmap = SelectObject(mem_dc, bitmap.into());
            Ok(Self {
                hwnd,
                mem_dc,
                bitmap,
                old_bitmap,
                bits: bits.cast(),
                width,
                height,
                visible: false,
                animating: false,
            })
        }
    }

    /// Affiche `bgra` (BGRA prémultiplié, `width`×`height`) en (`x`, `y`).
    pub fn show(&mut self, x: i32, y: i32, bgra: &[u8]) {
        let len = (self.width * self.height * 4) as usize;
        if bgra.len() != len {
            return;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bgra.as_ptr(), self.bits, len);
            let dst = POINT { x, y };
            let size = SIZE {
                cx: self.width as i32,
                cy: self.height as i32,
            };
            let src = POINT::default();
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&dst),
                Some(&size),
                Some(self.mem_dc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            if !self.visible {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                self.visible = true;
            }
        }
    }

    pub fn hide(&mut self) {
        if self.visible {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
        }
    }

    /// Réveille la boucle de messages toutes les `interval_ms` tant que
    /// l'animation tourne.
    pub fn set_animating(&mut self, on: bool, interval_ms: u32) {
        if on == self.animating {
            return;
        }
        unsafe {
            if on {
                SetTimer(Some(self.hwnd), OVERLAY_TIMER, interval_ms, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), OVERLAY_TIMER);
            }
        }
        self.animating = on;
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.mem_dc, self.old_bitmap);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.mem_dc);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
