//! Enchaînement des états d'une dictée selon les raccourcis.
//!
//! En « maintenir pour parler », l'enregistrement continue un court instant
//! après le relâchement (`release_tail_ms`) pour ne pas couper la fin d'une
//! phrase quand les touches sont lâchées un peu tôt. Un nouvel appui pendant
//! ce délai prolonge la même dictée.

use std::time::Duration;

use crate::hotkey::Action;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Recording,
    /// Raccourci relâché : l'enregistrement se termine à l'échéance.
    Ending,
}

/// Ce que le contrôleur doit faire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Start(&'static str),
    Stop,
    StopAfter(Duration),
    /// Annule un arrêt programmé : la dictée continue.
    KeepRecording,
    /// Abandonne la dictée sans rien taper.
    Cancel,
    Nothing,
}

pub const MODE_TOGGLE: &str = "Bascule";
pub const MODE_HOLD: &str = "Maintien";
/// Dictée tapée telle quelle, quel que soit le mode de transformation.
pub const MODE_RAW: &str = "Brut";

/// Réaction à une action du raccourci.
pub fn on_action(phase: Phase, action: Action, tail: Duration) -> (Phase, Effect) {
    use Action::*;
    use Phase::*;
    match (phase, action) {
        (Idle, Toggle) => (Recording, Effect::Start(MODE_TOGGLE)),
        (Idle, ToggleRaw) => (Recording, Effect::Start(MODE_RAW)),
        (Idle, Start) => (Recording, Effect::Start(MODE_HOLD)),
        (Idle, Stop | Cancel) => (Idle, Effect::Nothing),
        (Recording | Ending, Cancel) => (Idle, Effect::Cancel),
        // Un second appui en mode bascule est délibéré : arrêt immédiat.
        (Recording | Ending, Toggle | ToggleRaw) => (Idle, Effect::Stop),
        (Recording, Stop) if tail.is_zero() => (Idle, Effect::Stop),
        (Recording, Stop) => (Ending, Effect::StopAfter(tail)),
        (Recording, Start) => (Recording, Effect::Nothing),
        (Ending, Start) => (Recording, Effect::KeepRecording),
        (Ending, Stop) => (Ending, Effect::Nothing),
    }
}

/// Échéance du prolongement atteinte.
pub fn on_tail_elapsed(phase: Phase) -> (Phase, Effect) {
    match phase {
        Phase::Ending => (Phase::Idle, Effect::Stop),
        other => (other, Effect::Nothing),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAIL: Duration = Duration::from_millis(300);

    #[test]
    fn toggle_starts_and_stops_immediately() {
        let (p, e) = on_action(Phase::Idle, Action::Toggle, TAIL);
        assert_eq!((p, e), (Phase::Recording, Effect::Start(MODE_TOGGLE)));
        assert_eq!(
            on_action(p, Action::Toggle, TAIL),
            (Phase::Idle, Effect::Stop)
        );
    }

    #[test]
    fn hold_release_waits_for_the_tail() {
        let (p, e) = on_action(Phase::Idle, Action::Start, TAIL);
        assert_eq!(e, Effect::Start(MODE_HOLD));
        let (p, e) = on_action(p, Action::Stop, TAIL);
        assert_eq!((p, e), (Phase::Ending, Effect::StopAfter(TAIL)));
        // Relâchement en double : ignoré.
        assert_eq!(
            on_action(p, Action::Stop, TAIL),
            (Phase::Ending, Effect::Nothing)
        );
        assert_eq!(on_tail_elapsed(p), (Phase::Idle, Effect::Stop));
    }

    #[test]
    fn pressing_again_during_the_tail_continues_the_same_dictation() {
        let (p, _) = on_action(Phase::Recording, Action::Stop, TAIL);
        let (p, e) = on_action(p, Action::Start, TAIL);
        assert_eq!((p, e), (Phase::Recording, Effect::KeepRecording));
        // L'échéance périmée ne coupe plus rien.
        assert_eq!(on_tail_elapsed(p), (Phase::Recording, Effect::Nothing));
    }

    #[test]
    fn zero_tail_stops_on_release() {
        assert_eq!(
            on_action(Phase::Recording, Action::Stop, Duration::ZERO),
            (Phase::Idle, Effect::Stop)
        );
    }

    #[test]
    fn toggle_during_the_tail_stops_now() {
        assert_eq!(
            on_action(Phase::Ending, Action::Toggle, TAIL),
            (Phase::Idle, Effect::Stop)
        );
    }

    #[test]
    fn raw_toggle_and_cancel() {
        let (p, e) = on_action(Phase::Idle, Action::ToggleRaw, TAIL);
        assert_eq!((p, e), (Phase::Recording, Effect::Start(MODE_RAW)));
        // L'un ou l'autre raccourci arrête la dictée en cours.
        assert_eq!(
            on_action(p, Action::Toggle, TAIL),
            (Phase::Idle, Effect::Stop)
        );
        assert_eq!(
            on_action(Phase::Ending, Action::Cancel, TAIL),
            (Phase::Idle, Effect::Cancel)
        );
        assert_eq!(
            on_action(Phase::Idle, Action::Cancel, TAIL),
            (Phase::Idle, Effect::Nothing)
        );
    }

    #[test]
    fn stray_events_do_nothing() {
        assert_eq!(
            on_action(Phase::Idle, Action::Stop, TAIL),
            (Phase::Idle, Effect::Nothing)
        );
        assert_eq!(on_tail_elapsed(Phase::Idle), (Phase::Idle, Effect::Nothing));
        assert_eq!(
            on_action(Phase::Recording, Action::Start, TAIL),
            (Phase::Recording, Effect::Nothing)
        );
    }
}
