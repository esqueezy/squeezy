//! Unit tests for the Terminal Restore Command (§12.9.2) policy: the pending
//! request state machine, the single-sourced status wording, and the recovery
//! sequence contract (teardown-then-re-enter, never a scrollback purge).

use super::*;

// ---------------------------------------------------------------------------
// Pending-request state machine.
// ---------------------------------------------------------------------------

#[test]
fn default_request_is_none_and_not_pending() {
    let request = TerminalRestoreRequest::default();
    assert_eq!(request, TerminalRestoreRequest::None);
    assert!(!request.is_pending(), "a fresh request is idle");
}

#[test]
fn pending_request_reports_pending() {
    let request = TerminalRestoreRequest::Pending;
    assert!(request.is_pending());
}

#[test]
fn take_drains_a_pending_request_exactly_once() {
    let mut request = TerminalRestoreRequest::Pending;
    assert!(
        request.take(),
        "the first drain consumes the queued restore"
    );
    assert_eq!(
        request,
        TerminalRestoreRequest::None,
        "the request is cleared after the drain",
    );
    assert!(
        !request.take(),
        "a second drain on a cleared request is a no-op",
    );
}

#[test]
fn take_on_an_idle_request_is_false() {
    let mut request = TerminalRestoreRequest::None;
    assert!(!request.take());
    assert_eq!(request, TerminalRestoreRequest::None);
}

// ---------------------------------------------------------------------------
// Status wording (single source for the verb + the slash twin).
// ---------------------------------------------------------------------------

#[test]
fn restore_status_reports_the_redraw_when_alt_screen_was_active() {
    let status = restore_status(true);
    assert_eq!(status, RESTORE_STATUS);
    assert!(
        status.contains("restored"),
        "a real recovery confirms the restore: {status}",
    );
}

#[test]
fn restore_status_reports_a_no_op_when_already_in_the_normal_screen() {
    let status = restore_status(false);
    assert_eq!(status, RESTORE_NOOP_STATUS);
    assert!(
        status.contains("nothing to restore"),
        "an already-normal screen reports the honest no-op: {status}",
    );
}

#[test]
fn real_and_noop_status_lines_are_distinct() {
    assert_ne!(
        restore_status(true),
        restore_status(false),
        "the recovery and no-op confirmations must read differently",
    );
}

// ---------------------------------------------------------------------------
// Recovery-sequence contract.
// ---------------------------------------------------------------------------

#[test]
fn restore_sequence_tears_down_before_re_entering() {
    let teardown = RESTORE_SEQUENCE
        .iter()
        .position(|s| *s == RestoreStep::EmergencyTeardown)
        .expect("the sequence leaves the alt-screen / restores modes");
    let enter = RESTORE_SEQUENCE
        .iter()
        .position(|s| *s == RestoreStep::EnterSetup)
        .expect("the sequence re-enters the fullscreen surface");
    assert!(
        teardown < enter,
        "the terminal is restored to a sane state BEFORE it is re-entered",
    );
}

#[test]
fn restore_sequence_cycles_raw_mode_around_the_re_enter() {
    let disable = RESTORE_SEQUENCE
        .iter()
        .position(|s| *s == RestoreStep::DisableRawMode)
        .expect("raw mode is disabled during the restore");
    let enable = RESTORE_SEQUENCE
        .iter()
        .position(|s| *s == RestoreStep::EnableRawMode)
        .expect("raw mode is re-enabled for the re-entered surface");
    let enter = RESTORE_SEQUENCE
        .iter()
        .position(|s| *s == RestoreStep::EnterSetup)
        .expect("the sequence re-enters the fullscreen surface");
    assert!(
        disable < enable,
        "raw mode is disabled before it is re-enabled",
    );
    assert!(
        enable < enter,
        "raw mode is back on before the enter-setup bytes go out",
    );
}

#[test]
fn restore_sequence_ends_with_a_forced_redraw() {
    assert_eq!(
        RESTORE_SEQUENCE.last(),
        Some(&RestoreStep::ForceFullRedraw),
        "the recovery lands the user on a freshly repainted surface",
    );
}

#[test]
fn restore_sequence_is_a_minimal_five_step_recovery() {
    // The recovery is exactly: teardown, disable raw, enable raw, enter, redraw.
    // Pinning the length guards against an accidental extra (e.g. a scrollback
    // purge) sneaking into the contract.
    assert_eq!(RESTORE_SEQUENCE.len(), 5);
}
