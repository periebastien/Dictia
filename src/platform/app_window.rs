//! Fenêtre principale (hôte de la WebView), fenêtre cachée de contact entre
//! instances, et services système utilisés par l'interface.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Result};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GlobalFree, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND, DWM_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteKeyValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowW,
    GetCursorPos, GetWindowThreadProcessId, PostMessageW, RegisterClassW, SetForegroundWindow,
    ShowWindow, SystemParametersInfoW, HTCAPTION, SPI_GETWORKAREA, SW_HIDE, SW_MINIMIZE,
    SW_RESTORE, SW_SHOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WM_APP, WM_CLOSE, WM_NCLBUTTONDOWN,
    WNDCLASSW, WS_EX_APPWINDOW, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU,
};

use super::WindowHandle;

// ---------------------------------------------------------------------------
// Écran haute densité

/// Rend le processus « DPI aware » : textes nets à 125 %, 150 %…
pub fn enable_dpi_awareness() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Facteur d'échelle de l'écran principal (1.0 = 100 %).
pub fn system_scale() -> f32 {
    let dpi = unsafe { GetDpiForSystem() };
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / 96.0
    }
}

// ---------------------------------------------------------------------------
// Demandes d'affichage de l'interface (icône, seconde instance, fermeture)

static SHOW_REQUESTED: AtomicBool = AtomicBool::new(false);
static CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiRequest {
    Show,
    Close,
}

/// Demande l'ouverture (ou la fermeture) de la fenêtre ; traitée par la
/// boucle principale au prochain message.
pub fn request_ui(request: UiRequest) {
    match request {
        UiRequest::Show => SHOW_REQUESTED.store(true, Ordering::SeqCst),
        UiRequest::Close => CLOSE_REQUESTED.store(true, Ordering::SeqCst),
    }
}

pub fn take_ui_request() -> Option<UiRequest> {
    if CLOSE_REQUESTED.swap(false, Ordering::SeqCst) {
        Some(UiRequest::Close)
    } else if SHOW_REQUESTED.swap(false, Ordering::SeqCst) {
        Some(UiRequest::Show)
    } else {
        None
    }
}

const HOST_CLASS: PCWSTR = w!("DictiaHost");
const MAIN_CLASS: PCWSTR = w!("DictiaMain");
/// Envoyé par une seconde instance pour afficher la fenêtre.
const WM_DICTIA_SHOW: u32 = WM_APP + 3;

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_DICTIA_SHOW {
        request_ui(UiRequest::Show);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_CLOSE {
        request_ui(UiRequest::Close);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn instance() -> Result<HINSTANCE> {
    Ok(unsafe { GetModuleHandleW(None)? }.into())
}

fn register_class(
    name: PCWSTR,
    proc_: windows::Win32::UI::WindowsAndMessaging::WNDPROC,
) -> Result<HINSTANCE> {
    let hinstance = instance()?;
    let wc = WNDCLASSW {
        lpfnWndProc: proc_,
        hInstance: hinstance,
        lpszClassName: name,
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };
    Ok(hinstance)
}

/// Fenêtre invisible qui permet à une seconde instance de joindre celle-ci.
pub struct HostWindow(HWND);

impl HostWindow {
    pub fn new() -> Result<Self> {
        let hinstance = register_class(HOST_CLASS, Some(host_proc))?;
        let hwnd = unsafe {
            CreateWindowExW(
                Default::default(),
                HOST_CLASS,
                w!("Dictia"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinstance),
                None,
            )?
        };
        Ok(Self(hwnd))
    }
}

impl Drop for HostWindow {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

/// Demande à l'instance déjà lancée d'afficher sa fenêtre. `true` si elle a
/// été trouvée.
pub fn signal_running_instance() -> bool {
    unsafe {
        let Ok(hwnd) = FindWindowW(HOST_CLASS, None) else {
            return false;
        };
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        // L'utilisateur vient de lancer cette instance : elle a le droit de
        // passer la main au premier plan à l'autre.
        let _ = AllowSetForegroundWindow(pid);
        PostMessageW(Some(hwnd), WM_DICTIA_SHOW, WPARAM(0), LPARAM(0)).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Fenêtre principale

/// Fenêtre sans bordure système (la page dessine sa barre de titre), placée
/// au-dessus de la zone de notification.
pub struct AppWindow {
    hwnd: HWND,
}

impl AppWindow {
    /// `width`×`height` en pixels physiques.
    pub fn new(width: i32, height: i32) -> Result<Self> {
        let hinstance = register_class(MAIN_CLASS, Some(main_proc))?;
        let mut work = RECT::default();
        unsafe {
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some((&mut work as *mut RECT).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
        }
        let margin = 12;
        let x = (work.right - width - margin).max(work.left);
        let y = (work.bottom - height - margin).max(work.top);
        let title = HSTRING::from("Dictia");
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_APPWINDOW,
                MAIN_CLASS,
                PCWSTR(title.as_ptr()),
                WS_POPUP | WS_SYSMENU | WS_MINIMIZEBOX,
                x,
                y,
                width,
                height,
                None,
                None,
                Some(hinstance),
                None,
            )?
        };
        unsafe {
            // Windows 11 : coins arrondis et cadre sombre (sans effet ailleurs).
            let round: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&round as *const DWM_WINDOW_CORNER_PREFERENCE).cast(),
                std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            );
            let dark: i32 = 1;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&dark as *const i32).cast(),
                4,
            );
        }
        Ok(Self { hwnd })
    }

    pub fn raw(&self) -> isize {
        self.hwnd.0 as isize
    }

    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_RESTORE);
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    pub fn hide(&self) {
        let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }

    pub fn minimize(&self) {
        let _ = unsafe { ShowWindow(self.hwnd, SW_MINIMIZE) };
    }

    /// Déplace la fenêtre avec la souris, comme une barre de titre.
    pub fn begin_drag(&self) {
        unsafe {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = ReleaseCapture();
            let packed = ((pt.y as u32 & 0xFFFF) << 16) | (pt.x as u32 & 0xFFFF);
            let _ = PostMessageW(
                Some(self.hwnd),
                WM_NCLBUTTONDOWN,
                WPARAM(HTCAPTION as usize),
                LPARAM(packed as isize),
            );
        }
    }
}

impl Drop for AppWindow {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

// ---------------------------------------------------------------------------
// Presse-papier (bouton « Copier » de l'historique uniquement)

pub fn set_clipboard_text(text: &str) -> Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    unsafe {
        OpenClipboard(None)?;
        let result = (|| -> Result<()> {
            EmptyClipboard()?;
            let mem: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, bytes)?;
            let ptr = GlobalLock(mem);
            if ptr.is_null() {
                let _ = GlobalFree(Some(mem));
                bail!("mémoire du presse-papier indisponible");
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr().cast::<u8>(), ptr.cast::<u8>(), bytes);
            let _ = GlobalUnlock(mem);
            if let Err(e) = SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(mem.0))) {
                let _ = GlobalFree(Some(mem));
                return Err(e.into());
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

// ---------------------------------------------------------------------------
// Lancement au démarrage (HKCU\…\Run)

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Dictia");

pub fn run_at_startup() -> bool {
    let mut size = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
        .is_ok()
    }
}

pub fn set_run_at_startup(enabled: bool) -> Result<()> {
    unsafe {
        if !enabled {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
            return if r.is_ok() || !run_at_startup() {
                Ok(())
            } else {
                Err(windows::core::Error::from(r.to_hresult()).into())
            };
        }
        let exe = std::env::current_exe()?;
        let command = format!("\"{}\"", exe.display());
        let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2);
        let mut key = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key).ok()?;
        let r = RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(bytes));
        let _ = RegCloseKey(key);
        r.ok()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Application d'une fenêtre

/// Chemin de l'exécutable qui possède la fenêtre.
pub fn window_exe(window: WindowHandle) -> Option<String> {
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(window.hwnd(), Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(process);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}
