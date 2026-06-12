//! Run Selected Queued Next (§11G.9).
//!
//! While the prompt-queue overlay is open, `r` (and the mouse twin
//! `interaction::Action::QueueRunNext`) on the focused queued prompt promotes it
//! to the front of the queue so it runs before everything else still waiting:
//!
//! * **Idle** (no turn running) — the promoted prompt is moved to the front and
//!   the auto-drain pump is armed, so the very next event-loop tick pops it and
//!   starts a turn immediately.
//! * **Busy** (a turn is running) — the promoted prompt is moved to the front
//!   only; the existing drain-on-turn-finish path then runs it next, ahead of
//!   the rest of the queue, without interrupting the live turn.
//!
//! This module is the *pure-state* surface: it owns nothing and only computes
//! *what* the caller should do from three scalars (the focused row, the queue
//! length, and whether a turn is running). Keeping the decision here — pure and
//! terminal-free — means the keyboard (`r`) and mouse paths in `lib.rs` share one
//! source of truth and the math is pinned by unit tests without a live queue or a
//! running terminal. The actual mutation (move + id sidecar + undo record +
//! arming the pump) stays in `lib.rs`, which owns the model.

/// The decision for a "run selected next" request, computed by [`plan`].
///
/// `from` is the live index of the focused prompt the caller promotes to the
/// front; `run_now` is `true` exactly when no turn is running, telling the caller
/// to arm the auto-drain pump so the prompt starts immediately (rather than
/// waiting for the current turn to finish).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunNextPlan {
    /// Live index of the focused queued prompt to promote to the front.
    pub(crate) from: usize,
    /// Whether the promoted prompt should start running immediately. `true` when
    /// idle (arm the drain pump now); `false` when a turn is running (the
    /// drain-on-finish path runs it next).
    pub(crate) run_now: bool,
    /// Whether promoting actually changes the order. `false` when the focused row
    /// is already at the front (`from == 0`): the caller then skips the move and
    /// its undo record, but — when idle — still arms the pump so an already-front
    /// prompt runs immediately rather than silently doing nothing.
    pub(crate) moves: bool,
}

/// Decide what a "run selected next" request should do.
///
/// Returns `None` when there is nothing to act on — the queue is empty, or the
/// focused row is out of range (a transient stale cursor) — so the caller no-ops
/// rather than promoting a phantom row. Otherwise returns a [`RunNextPlan`]:
///
/// * `from` echoes the in-range `selected` index.
/// * `moves` is `true` unless the row is already at the front.
/// * `run_now` is `!turn_running`: idle requests arm an immediate drain; busy
///   ones only reorder and let the turn-finish drain pick the front prompt up.
///
/// Pure index/flag math over the three scalars, identical for the keyboard verb
/// and the mouse twin, so the two paths can never diverge and the tests pin it
/// without any `TuiApp`.
pub(crate) fn plan(selected: usize, queue_len: usize, turn_running: bool) -> Option<RunNextPlan> {
    if queue_len == 0 || selected >= queue_len {
        return None;
    }
    Some(RunNextPlan {
        from: selected,
        run_now: !turn_running,
        moves: selected != 0,
    })
}

#[cfg(test)]
#[path = "queue_run_next_tests.rs"]
mod tests;
