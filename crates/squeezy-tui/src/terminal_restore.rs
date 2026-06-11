//! Terminal Restore Command (§12.9.2): a user-driven recovery verb that forcibly
//! returns the terminal to a sane state when it is wedged — hidden cursor, stuck
//! raw mode, live mouse / bracketed-paste / focus reporting, alternate-scroll, a
//! lingering alternate screen, keyboard-enhancement flags, or a stale title.
//!
//! Unlike the crash paths in [`crate::signal_teardown`] (panic hook, SIGTERM /
//! SIGHUP), this is invoked from a *live* session: the user presses the restore
//! chord (or types `/terminal-reset`) because the screen looks corrupted but the
//! app is still running. So the recovery is restore-then-RE-ENTER: it replays the
//! exact single-sourced emergency-teardown bytes (leave the alternate screen,
//! disable every mode, show the hardware cursor, reset attrs + title) and disables
//! raw mode, then immediately re-enables raw mode, replays the enter-setup bytes,
//! and forces a full repaint from model state — so the user lands back in a clean
//! fullscreen surface rather than at a bare shell.
//!
//! This module owns only the dependency-free *policy*: the request flag plumbing,
//! the status text, and a pure description of the restore sequence so a
//! capture-sink test can assert the exact bytes with no real TTY. The byte work
//! and the raw-mode toggles live on the terminal guard in `lib.rs` (it owns the
//! writer); this keeps the recovery logic testable in isolation and the guard's
//! method a thin orchestrator that reuses the proven emit machinery.
//!
//! Deliberately NEVER purges scrollback (`\x1b[3J`): a recovery must repair the
//! live terminal without destroying the user's pre-launch history, exactly like
//! the clean-exit / emergency-teardown contract. The restore is idempotent and
//! best-effort — running it twice, or on a terminal that filters some sequences
//! (tmux / screen), simply re-emits the harmless mode resets.

/// The status line shown after a Terminal Restore Command completes. A single
/// source so the keyboard verb, the `/terminal-reset` slash command, and the
/// tests agree on the user-facing confirmation.
pub(crate) const RESTORE_STATUS: &str = "\u{2713} terminal restored — modes reset, screen redrawn";

/// The status line shown when the restore is requested but there is nothing to
/// restore (the alternate screen has already been left — e.g. a clean exit raced
/// the request). The flag is still consumed; this just reports the no-op honestly
/// instead of claiming a redraw that did not happen.
pub(crate) const RESTORE_NOOP_STATUS: &str =
    "terminal already in normal screen — nothing to restore";

/// The short toast shown the instant the restore is requested, so the verb has a
/// visible, on-surface acknowledgement (the status line is not the transient-notice
/// surface in the main view). Kept terse so it survives the toast width clamp.
pub(crate) const RESTORE_REQUESTED_TOAST: &str = "restoring terminal\u{2026}";

/// The short toast shown after a real restore completes, the on-surface twin of
/// [`RESTORE_STATUS`]. Kept terse so it survives the toast width clamp.
pub(crate) const RESTORE_DONE_TOAST: &str = "terminal restored";

/// One logical step of the Terminal Restore sequence, in emission order. A pure,
/// dependency-free description so a test can assert the *shape* of the recovery
/// (teardown precedes re-enter; the screen is never purged) without a real TTY or
/// the crossterm command types. The guard's `run_pending_terminal_restore` emits
/// the real bytes for each step by reusing the single-sourced emit functions.
///
/// `cfg(test)`-only: this is the executable spec of the recovery order that the
/// `run_pending_terminal_restore` body must follow, asserted by the unit tests; it
/// carries no runtime behavior, so it does not exist in a shipping build.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestoreStep {
    /// Leave the alternate screen + disable mouse / paste / focus / alt-scroll,
    /// reset keyboard-enhancement flags + title, and show the hardware cursor —
    /// the single-sourced emergency-teardown bytes.
    EmergencyTeardown,
    /// Disable raw mode (a crossterm mode call, emitted after the teardown bytes
    /// so the preceding restores are unaffected).
    DisableRawMode,
    /// Re-enable raw mode for the re-entered fullscreen surface.
    EnableRawMode,
    /// Replay the enter-setup bytes: re-enter the alternate screen, clear, home,
    /// re-arm bracketed paste / focus / mouse capture, hide the cursor.
    EnterSetup,
    /// Force a full repaint from model state on the next frame (clear ratatui's
    /// diff baseline; the freshly re-entered alternate screen is blank).
    ForceFullRedraw,
}

/// The ordered restore-then-re-enter recovery sequence. Exposed as a pure
/// constant so a test can pin the contract: the teardown (leave alt-screen +
/// restore modes + show cursor) comes first, raw mode is cycled around the
/// re-enter, and a forced redraw lands last. There is intentionally NO
/// scrollback-purge step. `cfg(test)`-only — see [`RestoreStep`].
#[cfg(test)]
pub(crate) const RESTORE_SEQUENCE: &[RestoreStep] = &[
    RestoreStep::EmergencyTeardown,
    RestoreStep::DisableRawMode,
    RestoreStep::EnableRawMode,
    RestoreStep::EnterSetup,
    RestoreStep::ForceFullRedraw,
];

/// A pending Terminal Restore request. The keymap dispatch / slash command set
/// this on the app model (side-effect-light); the run loop, which owns the
/// terminal guard, drains it and performs the byte work. Modeled as a small enum
/// rather than a bare `bool` so a future variant (e.g. a dry-run that only
/// reports) can be added without churning the call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TerminalRestoreRequest {
    /// No restore pending (the common idle state).
    #[default]
    None,
    /// A full restore-then-re-enter recovery is queued for the next loop turn.
    Pending,
}

impl TerminalRestoreRequest {
    /// Whether a restore is queued. The run loop checks this each turn; `false`
    /// at idle costs a single discriminant comparison.
    pub(crate) fn is_pending(self) -> bool {
        matches!(self, TerminalRestoreRequest::Pending)
    }

    /// Read-and-clear the request, returning `true` exactly once per queued
    /// restore so the loop runs the recovery a single time. Idempotent: a second
    /// drain on an already-cleared request returns `false`.
    pub(crate) fn take(&mut self) -> bool {
        let pending = self.is_pending();
        *self = TerminalRestoreRequest::None;
        pending
    }
}

/// Resolve the status line for a completed restore. `alt_screen_active` is the
/// guard's view of whether the alternate screen was live when the restore ran: a
/// real recovery (`true`) reports the redraw; an already-normal screen (`false`)
/// reports the honest no-op. Pure so the wording is asserted without a TTY.
pub(crate) fn restore_status(alt_screen_active: bool) -> &'static str {
    if alt_screen_active {
        RESTORE_STATUS
    } else {
        RESTORE_NOOP_STATUS
    }
}

#[cfg(test)]
#[path = "terminal_restore_tests.rs"]
mod tests;
