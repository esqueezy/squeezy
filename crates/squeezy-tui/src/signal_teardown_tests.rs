//! Headless tests for the Phase 9 crash-safety plumbing.
//!
//! The panic hook and signal handlers write to the real `stdout`, which a
//! headless test cannot capture. Instead these tests pin the load-bearing
//! contracts that ARE observable without a TTY:
//!
//!   * The emergency-teardown byte sequence the panic hook / signal handlers
//!     reuse ([`crate::emit_terminal_emergency_teardown`]) emits the disable
//!     sequence in the right order, and never the scrollback purge.
//!   * That emitter is best-effort: a writer that fails mid-stream does not
//!     panic (so it can never panic inside the panic hook).
//!   * The emitter is idempotent: running it twice (e.g. a clean exit then a
//!     late signal, or two crash paths racing) is safe and leaves the alternate
//!     screen exactly once when driven through the shared `alt_screen_active`
//!     flag.
//!   * The shared alt-screen flag gates the `LeaveAlternateScreen`.
//!
//! `super::*` is the `signal_teardown` module; lib internals are reached via
//! `crate::`.

use std::io::{self, Write};

use crate::{DISABLE_MOUSE_MODES, RESET_KEYBOARD_ENHANCEMENT_FLAGS};

/// A writer that fails every write after `ok_writes` successful ones, to prove
/// the teardown emitters are best-effort: a dying/closed stdout (SIGHUP, broken
/// SSH pipe) must never make the panic hook or a signal handler panic.
struct FailingWriter {
    ok_writes: usize,
    writes_done: usize,
}

impl FailingWriter {
    fn new(ok_writes: usize) -> Self {
        Self {
            ok_writes,
            writes_done: 0,
        }
    }
}

impl Write for FailingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.writes_done >= self.ok_writes {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "writer closed"));
        }
        self.writes_done += 1;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.writes_done >= self.ok_writes {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "writer closed"));
        }
        Ok(())
    }
}

/// The teardown sequence the panic hook / signal handlers reuse leaves the
/// alternate screen and restores terminal modes (disable mouse modes, bracketed
/// paste, focus reporting, keyboard-enhancement flags) — in the right order:
/// the mode restores land AFTER `LeaveAlternateScreen` so they apply to the
/// restored normal buffer. This is the exact sequence the crash paths emit.
#[test]
fn panic_hook_teardown_emits_disable_sequence_in_order() {
    let mut bytes = Vec::new();
    crate::emit_terminal_emergency_teardown(&mut bytes, /* alt_screen_active = */ true)
        .expect("emergency teardown emits");
    let ansi = String::from_utf8(bytes).expect("ansi");

    // Leaves the alternate screen.
    let leave_pos = ansi
        .find("\x1b[?1049l")
        .expect("teardown must leave the alternate screen");
    // Disables mouse modes, bracketed paste, focus reporting; resets keyboard
    // enhancement flags. (Focus reporting off is `\x1b[?1004l`; bracketed paste
    // off is `\x1b[?2004l`.)
    assert!(
        ansi.contains(DISABLE_MOUSE_MODES),
        "teardown must disable mouse modes"
    );
    assert!(
        ansi.contains("\x1b[?2004l"),
        "teardown must disable bracketed paste"
    );
    assert!(
        ansi.contains("\x1b[?1004l"),
        "teardown must disable focus reporting"
    );
    let reset_pos = ansi
        .find(RESET_KEYBOARD_ENHANCEMENT_FLAGS)
        .expect("teardown must reset keyboard enhancement flags");
    assert!(
        reset_pos > leave_pos,
        "mode restores must come AFTER LeaveAlternateScreen so they land on the \
         restored normal buffer"
    );

    // Crash-path teardown must NEVER purge the user's pre-launch scrollback.
    assert!(
        !ansi.contains("\x1b[3J"),
        "teardown must never purge scrollback (\\x1b[3J)"
    );
}

/// Best-effort: a writer that fails partway through the teardown must not panic.
/// `emit_terminal_emergency_teardown` returns an `Err` rather than unwinding, and
/// the panic hook / signal handler swallow it with `let _ = …`. Here we drive the
/// failing writer directly and assert no panic (the `Result` may be `Err`).
#[test]
fn panic_hook_teardown_is_best_effort_on_failing_writer() {
    // Fails on the very first write — the worst case for a panic hook.
    let mut writer = FailingWriter::new(0);
    let result = crate::emit_terminal_emergency_teardown(&mut writer, /* alt = */ true);
    // It must not panic; returning an error is fine (the caller discards it).
    assert!(
        result.is_err(),
        "a writer failing on the first write should surface an Err, not panic"
    );

    // Also exercise a mid-stream failure (some writes succeed, then the pipe
    // breaks). Still no panic.
    let mut writer = FailingWriter::new(3);
    let _ = crate::emit_terminal_emergency_teardown(&mut writer, /* alt = */ true);
}

/// Idempotence: running the teardown twice is safe, and when the alt-screen flag
/// is cleared between calls (as the shared flag does), the second call does NOT
/// re-leave the alternate screen — it only re-emits the idempotent mode restores.
/// This models a clean exit (which clears the flag) followed by a late signal.
#[test]
fn teardown_is_idempotent_and_leaves_alt_screen_once() {
    // First teardown with the alternate screen active: leaves it.
    let mut first = Vec::new();
    crate::emit_terminal_emergency_teardown(&mut first, /* alt = */ true).expect("first teardown");
    let first = String::from_utf8(first).expect("ansi");
    assert!(
        first.contains("\x1b[?1049l"),
        "first teardown (alt active) must leave the alternate screen"
    );

    // Second teardown with the flag now cleared (as the clean exit / a prior
    // teardown would have done): must NOT leave the alternate screen again, but
    // still restores modes harmlessly.
    let mut second = Vec::new();
    crate::emit_terminal_emergency_teardown(&mut second, /* alt = */ false)
        .expect("second teardown");
    let second = String::from_utf8(second).expect("ansi");
    assert!(
        !second.contains("\x1b[?1049l"),
        "second teardown (alt already left) must NOT re-leave the alternate screen"
    );
    assert!(
        second.contains(DISABLE_MOUSE_MODES),
        "second teardown still re-emits the idempotent mode restores"
    );
}

/// The shared alt-screen flag round-trips and read-and-clears via the public
/// module API, which is the contract the panic hook / signal handlers rely on to
/// leave the alternate screen exactly once.
#[test]
fn set_alt_screen_active_flag_round_trips() {
    // Save and restore the process-global flag so this test does not perturb a
    // concurrently running fullscreen guard's expectation in the same process.
    let saved = super::ALT_SCREEN_ACTIVE.load(std::sync::atomic::Ordering::SeqCst);

    super::set_alt_screen_active(true);
    assert!(super::ALT_SCREEN_ACTIVE.load(std::sync::atomic::Ordering::SeqCst));
    // `run_emergency_teardown` read-and-clears, so a second crash-path call sees
    // `false`. Emulate just the swap (the real fn also writes to stdout).
    let was = super::ALT_SCREEN_ACTIVE.swap(false, std::sync::atomic::Ordering::SeqCst);
    assert!(was, "swap returns the previous (active) value");
    assert!(
        !super::ALT_SCREEN_ACTIVE.load(std::sync::atomic::Ordering::SeqCst),
        "flag is cleared after the read-and-clear swap"
    );

    super::set_alt_screen_active(saved);
}

/// Unix-only: registering the SIGTERM/SIGHUP/SIGTSTP handlers inside a tokio
/// runtime must not panic and must not block (the listeners are spawned, no
/// signal is sent).
#[cfg(unix)]
#[tokio::test]
async fn install_signal_handlers_registers_without_blocking() {
    // Just installing must be infallible and non-blocking; we never deliver a
    // signal, so the spawned listeners stay parked and the test returns promptly.
    super::install_signal_handlers();
}

/// Unix-only: the SIGTSTP suspend request is a read-and-clear flag. The handler
/// arms it (here via the test setter, since raising a real SIGTSTP would stop the
/// test process), the loop drains it exactly once, and a second drain sees
/// nothing — so suspend runs once per Ctrl+Z, never on a stale flag.
#[cfg(unix)]
#[test]
fn suspend_request_is_read_and_cleared_once() {
    // Clear any state a concurrent test might have left, then arm and drain.
    let _ = super::take_suspend_request();
    super::request_suspend_for_test();
    assert!(
        super::take_suspend_request(),
        "an armed SIGTSTP request must be observed by the loop exactly once"
    );
    assert!(
        !super::take_suspend_request(),
        "a drained suspend request must not fire again (no stale re-suspend)"
    );
}

/// On non-Unix there is no job-control suspend, so the request drain is always
/// `false` — the loop's call site stays `cfg`-free and never suspends.
#[cfg(not(unix))]
#[test]
fn suspend_request_is_always_false_off_unix() {
    assert!(
        !super::take_suspend_request(),
        "no SIGTSTP suspend exists off Unix; the drain must always be false"
    );
}
