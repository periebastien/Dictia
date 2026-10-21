//! Appels propres au système d'exploitation, isolés du reste du code.

#[cfg(windows)]
mod app_window;
#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use self::app_window::*;
#[cfg(windows)]
pub use self::windows::*;
