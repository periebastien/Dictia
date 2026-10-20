//! Appels propres au système d'exploitation, isolés du reste du code.

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use self::windows::*;
