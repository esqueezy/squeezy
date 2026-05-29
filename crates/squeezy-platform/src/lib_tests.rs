use super::{ModifierShim, ProbedState, interpret_probe, shim_applicable};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn enter(modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(KeyCode::Enter, modifiers)
}

#[test]
fn shim_applicable_to_bare_enter() {
    assert!(shim_applicable(&enter(KeyModifiers::empty())));
}

#[test]
fn shim_applicable_when_only_shift_already_set() {
    // Shift may already be set when the terminal *did* deliver the
    // modifier (Kitty / modifyOtherKeys). We still inspect it so we
    // can no-op cleanly instead of regressing those terminals.
    assert!(shim_applicable(&enter(KeyModifiers::SHIFT)));
}

#[test]
fn shim_not_applicable_to_non_enter_key() {
    let event = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
    assert!(!shim_applicable(&event));
}

#[test]
fn shim_not_applicable_to_ctrl_enter() {
    assert!(!shim_applicable(&enter(KeyModifiers::CONTROL)));
}

#[test]
fn shim_not_applicable_to_alt_enter() {
    assert!(!shim_applicable(&enter(KeyModifiers::ALT)));
}

#[test]
fn shim_not_applicable_to_super_enter() {
    assert!(!shim_applicable(&enter(KeyModifiers::SUPER)));
}

#[test]
fn bare_enter_with_no_held_modifier_returns_none() {
    let event = enter(KeyModifiers::empty());
    assert_eq!(interpret_probe(&event, ProbedState::default()), None);
}

#[test]
fn bare_enter_with_held_shift_returns_shift_shim() {
    let event = enter(KeyModifiers::empty());
    let shim = interpret_probe(&event, ProbedState { shift: true })
        .expect("shift held should recover a shim");
    assert!(shim.shift());
    assert_eq!(shim.modifiers, KeyModifiers::SHIFT);
}

#[test]
fn enter_already_carrying_shift_is_skipped_even_when_held() {
    // Don't re-report a modifier the event already had — the caller
    // would otherwise OR `SHIFT` into a bitset that already contains
    // it, which is harmless but pointless and misleading in logs.
    let event = enter(KeyModifiers::SHIFT);
    assert_eq!(interpret_probe(&event, ProbedState { shift: true }), None);
}

#[test]
fn non_enter_key_with_held_shift_is_skipped() {
    // The shim only recovers modifiers for Enter; a held Shift on a
    // letter key already gets surfaced by the terminal via the
    // uppercase character.
    let event = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
    assert_eq!(interpret_probe(&event, ProbedState { shift: true }), None);
}

#[test]
fn ctrl_enter_with_held_shift_is_skipped() {
    // Ctrl+Enter arrives with the CONTROL modifier already set on
    // legacy terminals via the `\x00` byte (after squeezy's existing
    // control-byte normalisation). The shim leaves that path alone
    // so it doesn't accidentally promote Ctrl+Enter to
    // Ctrl+Shift+Enter when a user happens to be holding Shift.
    let event = enter(KeyModifiers::CONTROL);
    assert_eq!(interpret_probe(&event, ProbedState { shift: true }), None);
}

#[test]
fn modifier_shim_default_is_empty() {
    let shim = ModifierShim::default();
    assert!(!shim.shift());
    assert_eq!(shim.modifiers, KeyModifiers::empty());
}

#[test]
fn modifier_shim_apply_unions_bits() {
    let shim = ModifierShim {
        modifiers: KeyModifiers::SHIFT,
    };
    let combined = shim.apply(KeyModifiers::CONTROL);
    assert!(combined.contains(KeyModifiers::SHIFT));
    assert!(combined.contains(KeyModifiers::CONTROL));
}

#[test]
fn modifier_shim_apply_is_idempotent() {
    let shim = ModifierShim {
        modifiers: KeyModifiers::SHIFT,
    };
    let combined = shim.apply(KeyModifiers::SHIFT);
    assert_eq!(combined, KeyModifiers::SHIFT);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn non_macos_targets_have_zero_cost_noop() {
    // The public entry point should never report a shim on non-macOS
    // builds, regardless of the event shape, because the probe is a
    // compile-time stub returning `false`.
    let bare = enter(KeyModifiers::empty());
    assert_eq!(super::detect_modifier_state(&bare), None);

    let with_shift = enter(KeyModifiers::SHIFT);
    assert_eq!(super::detect_modifier_state(&with_shift), None);
}
