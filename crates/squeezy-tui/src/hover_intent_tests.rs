//! Unit tests for the Mouse Hover Intent (§12.1.3) pure state machine. They
//! exercise the reveal/leave/suppress transitions, the settle window, and the
//! keyboard-focus degrade rule directly against [`HoverIntentState`], with an
//! injected clock, so the timing logic is verified without a terminal.

use std::time::{Duration, Instant};

use super::{HoverIntentState, SuppressReason};

#[test]
fn default_is_enabled_with_no_target_and_no_suppression() {
    let state = HoverIntentState::default();
    assert!(state.is_enabled(), "enabled by default");
    assert_eq!(state.hovered_target(), None);
    assert_eq!(state.suppression(), None);
    assert_eq!(state.pointer_cell(), None);
}

#[test]
fn hover_enter_reveals_the_target_and_records_the_pointer_cell() {
    let mut state = HoverIntentState::default();
    let now = Instant::now();
    let changed = state.on_hover_enter(42, 7, 3, now);
    assert!(changed, "first reveal is a change");
    assert_eq!(state.hovered_target(), Some(42));
    assert_eq!(state.pointer_cell(), Some((7, 3)));
    // The reveal target with no keyboard focus is the hovered id.
    assert_eq!(state.reveal_target(None), Some(42));
}

#[test]
fn re_enter_same_target_is_not_a_change_so_no_redraw_loops() {
    let mut state = HoverIntentState::default();
    let now = Instant::now();
    assert!(state.on_hover_enter(42, 7, 3, now));
    // A second enter on the SAME id reports no change: a settled hover that the
    // recognizer happens to re-emit must not loop the redraw gate.
    assert!(!state.on_hover_enter(42, 8, 3, now + Duration::from_millis(50)));
    assert_eq!(state.hovered_target(), Some(42));
}

#[test]
fn hover_enter_onto_a_new_target_moves_the_reveal() {
    let mut state = HoverIntentState::default();
    let now = Instant::now();
    assert!(state.on_hover_enter(1, 0, 0, now));
    assert!(
        state.on_hover_enter(2, 0, 1, now + Duration::from_millis(10)),
        "moving onto a different id changes the reveal"
    );
    assert_eq!(state.hovered_target(), Some(2));
}

#[test]
fn hover_leave_clears_a_revealed_target() {
    let mut state = HoverIntentState::default();
    let now = Instant::now();
    state.on_hover_enter(9, 4, 2, now);
    let changed = state.on_hover_leave(now + Duration::from_millis(5));
    assert!(changed, "leaving a revealed target is a change (erase it)");
    assert_eq!(state.hovered_target(), None);
    assert_eq!(state.pointer_cell(), None);
    // Leaving again with nothing revealed is a no-op.
    assert!(!state.on_hover_leave(now + Duration::from_millis(6)));
}

#[test]
fn scroll_suppression_hides_an_in_flight_reveal() {
    let mut state = HoverIntentState::default();
    let now = Instant::now();
    state.on_hover_enter(5, 1, 1, now);
    assert_eq!(state.hovered_target(), Some(5));
    let changed = state.set_suppression(Some(SuppressReason::Scroll));
    assert!(changed, "suppressing while revealed hides the affordance");
    assert_eq!(state.hovered_target(), None);
    assert_eq!(state.suppression(), Some(SuppressReason::Scroll));
    // While suppressed, a hover enter does NOT reveal.
    assert!(!state.on_hover_enter(5, 1, 1, now + Duration::from_millis(1)));
    assert_eq!(state.hovered_target(), None);
}

#[test]
fn drag_and_selection_and_capture_off_are_all_suppression_reasons() {
    // Every gesture the spec lists as a hover-suppressor maps to a reason note,
    // so a diagnostic / status line can name exactly what blocked the reveal.
    for (reason, note) in [
        (SuppressReason::Scroll, "scroll"),
        (SuppressReason::Drag, "drag"),
        (SuppressReason::Selection, "selection"),
        (SuppressReason::CaptureOff, "capture-off"),
    ] {
        let mut state = HoverIntentState::default();
        state.set_suppression(Some(reason));
        assert_eq!(state.suppression(), Some(reason));
        assert_eq!(reason.note(), note);
        // While suppressed nothing reveals, even with a hover present.
        state.on_hover_enter(1, 0, 0, Instant::now());
        assert_eq!(state.reveal_target(None), None);
    }
}

#[test]
fn clearing_suppression_does_not_re_reveal_until_a_fresh_dwell() {
    let mut state = HoverIntentState::default();
    let now = Instant::now();
    state.on_hover_enter(7, 2, 2, now);
    state.set_suppression(Some(SuppressReason::Drag));
    assert_eq!(state.hovered_target(), None);
    // Clearing the suppression reports no change (nothing re-reveals on its own).
    assert!(!state.set_suppression(None));
    assert_eq!(state.hovered_target(), None, "no automatic re-reveal");
    // A fresh dwell after the drag reveals honestly.
    assert!(state.on_hover_enter(7, 2, 2, now + Duration::from_millis(20)));
    assert_eq!(state.hovered_target(), Some(7));
}

#[test]
fn reveal_target_degrades_to_keyboard_focus_when_no_hover() {
    let state = HoverIntentState::default();
    // No mouse hover at all: the reveal falls back to the focused entry, so the
    // affordance still appears on keyboard-only terminals (the `?1000h` case).
    assert_eq!(state.reveal_target(Some(99)), Some(99));
    assert_eq!(state.reveal_target(None), None);
}

#[test]
fn live_hover_wins_over_keyboard_focus() {
    let mut state = HoverIntentState::default();
    state.on_hover_enter(3, 0, 0, Instant::now());
    // A live, unsuppressed hover takes precedence over the focused id.
    assert_eq!(state.reveal_target(Some(99)), Some(3));
}

#[test]
fn suppressed_hover_falls_back_to_focus_not_the_hovered_id() {
    let mut state = HoverIntentState::default();
    state.on_hover_enter(3, 0, 0, Instant::now());
    state.set_suppression(Some(SuppressReason::Selection));
    // Suppressed: the hovered id is hidden, but keyboard focus still reveals.
    assert_eq!(state.reveal_target(Some(99)), Some(99));
}

#[test]
fn disabling_reveals_nothing_and_clears_state() {
    let mut state = HoverIntentState::default();
    state.on_hover_enter(8, 1, 1, Instant::now());
    assert!(state.is_enabled());
    let now_on = state.toggle();
    assert!(!now_on, "toggle off");
    assert!(!state.is_enabled());
    // Off ⇒ neither hover nor focus reveals anything, and the prior target is gone.
    assert_eq!(state.reveal_target(Some(99)), None);
    assert_eq!(state.hovered_target(), None);
    // While off, a hover enter reveals nothing.
    assert!(!state.on_hover_enter(8, 1, 1, Instant::now()));
    // Toggling back on restores the keyboard-focus reveal.
    assert!(state.toggle());
    assert_eq!(state.reveal_target(Some(99)), Some(99));
}

#[test]
fn reveal_pending_is_true_briefly_then_settles_so_no_redraw_loop() {
    let mut state = HoverIntentState::default();
    let now = Instant::now();
    state.on_hover_enter(4, 0, 0, now);
    // Right after revealing, the settle window is open (schedule one redraw).
    assert!(
        state.reveal_pending(now),
        "a fresh reveal is pending settling"
    );
    assert!(state.reveal_pending(now + Duration::from_millis(50)));
    // After the short settle window the state goes quiet: an idle, settled hover
    // schedules no further tick (the zero-idle-cost contract).
    assert!(
        !state.reveal_pending(now + Duration::from_millis(500)),
        "a long-settled hover schedules nothing"
    );
}

#[test]
fn reveal_pending_is_false_with_no_target() {
    let state = HoverIntentState::default();
    assert!(
        !state.reveal_pending(Instant::now()),
        "nothing hovered ⇒ never pending ⇒ zero idle tick"
    );
}

#[test]
fn clear_resets_every_in_flight_field() {
    let mut state = HoverIntentState::default();
    state.on_hover_enter(11, 5, 5, Instant::now());
    state.set_suppression(Some(SuppressReason::Scroll));
    state.clear();
    assert_eq!(state.hovered_target(), None);
    assert_eq!(state.suppression(), None);
    assert_eq!(state.pointer_cell(), None);
    // Cleared but still enabled, so focus reveal works again.
    assert!(state.is_enabled());
    assert_eq!(state.reveal_target(Some(1)), Some(1));
}
