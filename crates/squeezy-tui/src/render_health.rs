//! Stuck-Render Watchdog (§12.9.1).
//!
//! The fullscreen draw loop only repaints when a frame is *wanted*
//! (`needs_redraw` / a pending resize / an active animation) and the
//! frame-rate gate opens. That contract is what keeps an idle session silent,
//! but it also means a single wedged `draw` — a backend write that never
//! flushes, a renderer that quietly returns without committing cells, a
//! terminal that swallows a synchronized-update bracket — leaves the UI frozen
//! while the model keeps mutating underneath. There is no native signal for
//! "state moved but the screen did not."
//!
//! [`RenderHealth`] is that signal. It rides the timing the loop already
//! computes (one `Instant` per iteration) and tracks two revisions:
//!
//! - **state revision** — bumped every time the loop decides a frame is wanted
//!   (a visible state change), via [`RenderHealth::note_state_change`).
//! - **drawn revision** — snapped to the state revision after a frame actually
//!   commits (draw + flush), via [`RenderHealth::record_frame_committed`].
//!
//! When the drawn revision lags the state revision (frames are wanted but none
//! has committed) for longer than [`STALL_BUDGET`], the render is stuck. The
//! loop then self-heals: invalidate the ratatui buffer so the next `draw`
//! repaints every cell, force one clean full redraw, and surface a low-priority
//! recovery toast. A forced-redraw throttle ([`RECOVERY_THROTTLE`]) stops the
//! watchdog from re-firing recursively while the replacement frame is still
//! settling, so a genuinely dead terminal degrades to one quiet retry per
//! window rather than a redraw storm.
//!
//! ## Idle-redraw contract
//!
//! Everything here is driven from values the loop already has: when no frame is
//! wanted the watchdog clears its "wanted since" marker and does nothing, so an
//! idle session pays one `Option` reset per loop and never trips. The stall
//! clock only advances while a *wanted* frame is failing to commit — exactly
//! the condition the watchdog exists to catch.
//!
//! ## Windows clock trap
//!
//! Durations are always derived with [`Instant::saturating_duration_since`] and
//! any synthetic earlier instant in tests is built with
//! [`Instant::checked_sub`] plus a clock-safe fallback — never bare
//! `Instant - Duration`, which panics on fresh Windows CI runners whose
//! monotonic clock is younger than the offset.

#![cfg_attr(not(unix), allow(dead_code))]

use std::time::{Duration, Instant};

/// How long a wanted-but-uncommitted frame may persist before the watchdog
/// declares the render stuck. 2s is far longer than any legitimate frame (the
/// 60 FPS gate caps real frames at ~16ms) or a normal burst of coalesced
/// events, so a healthy session never approaches it; short enough that a real
/// freeze self-heals before the user reaches for the kill switch.
pub(crate) const STALL_BUDGET: Duration = Duration::from_secs(2);

/// Minimum spacing between forced recoveries. After the watchdog forces a
/// redraw it ignores further stalls for this long, so the replacement frame has
/// room to commit and the watchdog can never recurse into a redraw storm on a
/// terminal that stays wedged. One retry per window degrades gracefully.
pub(crate) const RECOVERY_THROTTLE: Duration = Duration::from_secs(5);

/// What [`RenderHealth::poll`] tells the loop to do this iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderHealthAction {
    /// Nothing to do: either no frame is wanted, a frame is wanted but still
    /// inside the stall budget, or a recovery is already throttled.
    Healthy,
    /// The render is stuck. The loop should invalidate the buffer, force one
    /// full redraw, and surface the recovery notice.
    Recover,
}

/// Watchdog state for the fullscreen draw loop. Cheap (`Copy`-sized fields, no
/// allocation) and fully deterministic given the instants the loop feeds it, so
/// the transition table is unit-testable without a real clock.
#[derive(Debug, Default)]
pub(crate) struct RenderHealth {
    /// Bumped on every visible state change (a frame becoming wanted).
    state_revision: u64,
    /// Snapped to `state_revision` when a frame actually commits.
    drawn_revision: u64,
    /// When the *currently pending* wanted frame first became wanted, i.e. the
    /// first loop iteration since the last commit on which a frame was wanted.
    /// `None` while the UI is up to date (drawn == state). The stall clock is
    /// `now - wanted_since`.
    wanted_since: Option<Instant>,
    /// When the last frame committed (draw + flush). Diagnostics only; lets the
    /// recovery notice report how long the screen was frozen.
    last_frame_at: Option<Instant>,
    /// Cheap signature of the last committed frame, used to recognise that a
    /// forced recovery actually changed the screen. Set by the caller from a
    /// per-frame value it already has (the frame ordinal); the watchdog only
    /// stores it for diagnostics and never recomputes a buffer hash.
    last_frame_signature: u64,
    /// How many times the watchdog has fired this session. Bounded counter for
    /// the diagnostics line / dogfood surface; never resets.
    stalled_count: u64,
    /// When the last forced recovery fired, gating [`RECOVERY_THROTTLE`].
    last_recovery_at: Option<Instant>,
}

impl RenderHealth {
    /// Record that the app's visible state changed (a frame is now wanted).
    /// Idempotent within one pending-frame window: the state revision advances
    /// every call, but `wanted_since` is stamped only on the first change since
    /// the last commit, so the stall clock measures the age of the *oldest*
    /// uncommitted change rather than resetting on every new one.
    pub(crate) fn note_state_change(&mut self, now: Instant) {
        self.state_revision = self.state_revision.wrapping_add(1);
        if self.wanted_since.is_none() {
            self.wanted_since = Some(now);
        }
    }

    /// Record that a frame committed (draw + flush) at `now` with the given
    /// cheap signature. Clears the pending-frame marker and snaps the drawn
    /// revision up to the state revision, so the UI is considered up to date.
    pub(crate) fn record_frame_committed(&mut self, now: Instant, signature: u64) {
        self.drawn_revision = self.state_revision;
        self.last_frame_at = Some(now);
        self.last_frame_signature = signature;
        self.wanted_since = None;
    }

    /// True when the screen is behind the model: at least one visible state
    /// change has not yet committed to a frame.
    // Exercised by the watchdog unit tests and available to diagnostics
    // consumers; the loop drives recovery off `poll`, not this accessor.
    #[allow(dead_code)]
    pub(crate) fn is_behind(&self) -> bool {
        self.drawn_revision != self.state_revision
    }

    /// Decide what the loop should do this iteration.
    ///
    /// `wants_draw` is the loop's own gate (`needs_redraw || pending_resize ||
    /// has_active_animation`). When it is `false` the watchdog resets its
    /// pending-frame marker and reports healthy — an idle frame can never be
    /// stuck. When a frame is wanted, the watchdog reports [`Recover`] only if
    /// the pending frame has been outstanding for at least [`STALL_BUDGET`] AND
    /// no recovery fired within [`RECOVERY_THROTTLE`].
    ///
    /// [`Recover`]: RenderHealthAction::Recover
    pub(crate) fn poll(&mut self, now: Instant, wants_draw: bool) -> RenderHealthAction {
        if !wants_draw {
            // Nothing pending: the screen is (or will be) up to date. Drop the
            // stall marker so a future wanted frame measures from its own start.
            self.wanted_since = None;
            return RenderHealthAction::Healthy;
        }
        // A frame is wanted. Ensure the stall clock is running even if the
        // caller routed the want through `pending_resize` / animation rather
        // than `note_state_change` (those paths set `wants_draw` without bumping
        // the revision). This keeps the watchdog honest about *any* wanted
        // frame, not only state-revision-driven ones.
        let since = *self.wanted_since.get_or_insert(now);
        if now.saturating_duration_since(since) < STALL_BUDGET {
            return RenderHealthAction::Healthy;
        }
        if let Some(last) = self.last_recovery_at
            && now.saturating_duration_since(last) < RECOVERY_THROTTLE
        {
            // A recovery is still settling; do not recurse.
            return RenderHealthAction::Healthy;
        }
        RenderHealthAction::Recover
    }

    /// Mark that a forced recovery just fired at `now`: bump the stall counter,
    /// arm the throttle, and reset the pending-frame marker so the replacement
    /// frame is measured fresh. The caller invokes this immediately after it
    /// invalidates the buffer and forces the redraw.
    pub(crate) fn note_recovery(&mut self, now: Instant) {
        self.stalled_count = self.stalled_count.wrapping_add(1);
        self.last_recovery_at = Some(now);
        // The forced redraw is itself the pending frame; restart its clock so
        // the throttle, not a stale `wanted_since`, governs the next decision.
        self.wanted_since = Some(now);
    }

    /// Total times the watchdog has fired this session.
    // Surfaced through `diagnostics`; exposed directly for tests and future
    // dogfood/status counters.
    #[allow(dead_code)]
    pub(crate) fn stalled_count(&self) -> u64 {
        self.stalled_count
    }

    /// The last committed frame's cheap signature (diagnostics / tests).
    // Stored for diagnostics parity with the spec's "last frame signature";
    // read by the unit tests today.
    #[allow(dead_code)]
    pub(crate) fn last_frame_signature(&self) -> u64 {
        self.last_frame_signature
    }

    /// How long the current pending frame has been outstanding at `now`, or
    /// `None` when the UI is up to date. Diagnostics for the recovery notice.
    pub(crate) fn pending_for(&self, now: Instant) -> Option<Duration> {
        self.wanted_since
            .map(|since| now.saturating_duration_since(since))
    }

    /// One-line, allocation-light diagnostics string written when the watchdog
    /// fires. Reports the revision gap, how long the screen was frozen, and the
    /// running stall count — everything a `tracing` reader needs to confirm the
    /// watchdog is the thing that unwedged the UI.
    pub(crate) fn diagnostics(&self, now: Instant) -> String {
        let frozen_ms = self
            .pending_for(now)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        format!(
            "stuck-render watchdog: state_rev={} drawn_rev={} frozen={}ms stalls={}",
            self.state_revision, self.drawn_revision, frozen_ms, self.stalled_count
        )
    }
}

#[cfg(test)]
#[path = "render_health_tests.rs"]
mod tests;
