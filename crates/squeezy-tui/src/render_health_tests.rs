//! Unit tests for the Stuck-Render Watchdog state machine (§12.9.1).
//!
//! These drive [`RenderHealth`] with synthetic instants so the transition
//! table is exercised deterministically, with no real clock and no sleeps.

use super::*;

/// Build an instant `offset` in the past relative to `now`, clock-safely.
///
/// `Instant - Duration` panics on platforms whose monotonic clock is younger
/// than the offset (fresh Windows CI runners boot with a tiny QPC value), so we
/// subtract via `checked_sub` and fall back to ever-smaller safe offsets,
/// finally to `now` itself. Tests that need a *specific* gap should keep the
/// offset modest so the first `checked_sub` succeeds on every platform.
fn earlier(now: Instant, offset: Duration) -> Instant {
    now.checked_sub(offset)
        .or_else(|| now.checked_sub(Duration::from_millis(1)))
        .unwrap_or(now)
}

#[test]
fn fresh_health_is_not_behind_and_polls_healthy() {
    let mut health = RenderHealth::default();
    let now = Instant::now();
    assert!(!health.is_behind());
    assert_eq!(health.stalled_count(), 0);
    // No frame wanted ⇒ never stuck.
    assert_eq!(health.poll(now, false), RenderHealthAction::Healthy);
}

#[test]
fn state_change_marks_behind_until_commit() {
    let mut health = RenderHealth::default();
    let now = Instant::now();
    health.note_state_change(now);
    assert!(health.is_behind(), "a pending state change is behind");
    health.record_frame_committed(now, 7);
    assert!(
        !health.is_behind(),
        "a committed frame catches the screen up"
    );
    assert_eq!(health.last_frame_signature(), 7);
}

#[test]
fn wanted_frame_within_budget_stays_healthy() {
    let mut health = RenderHealth::default();
    let now = Instant::now();
    health.note_state_change(now);
    // Still inside the stall budget ⇒ not yet stuck.
    assert_eq!(health.poll(now, true), RenderHealthAction::Healthy);
}

#[test]
fn wanted_frame_past_budget_triggers_recovery() {
    let mut health = RenderHealth::default();
    let now = Instant::now();
    // Stamp the want well past the stall budget.
    let wanted = earlier(now, STALL_BUDGET + Duration::from_millis(500));
    health.note_state_change(wanted);
    assert_eq!(health.poll(now, true), RenderHealthAction::Recover);
}

#[test]
fn idle_iteration_resets_the_stall_clock() {
    let mut health = RenderHealth::default();
    let now = Instant::now();
    let wanted = earlier(now, STALL_BUDGET + Duration::from_millis(500));
    health.note_state_change(wanted);
    // An idle iteration (no frame wanted) clears the pending marker, so a
    // later wanted frame measures from its own start rather than tripping.
    assert_eq!(health.poll(now, false), RenderHealthAction::Healthy);
    assert!(health.pending_for(now).is_none());
    // A brand-new want at `now` is well inside the budget.
    health.note_state_change(now);
    assert_eq!(health.poll(now, true), RenderHealthAction::Healthy);
}

#[test]
fn commit_before_budget_prevents_recovery() {
    let mut health = RenderHealth::default();
    let now = Instant::now();
    let wanted = earlier(now, STALL_BUDGET - Duration::from_millis(200));
    health.note_state_change(wanted);
    // The frame commits before the budget elapses, so no stall is ever seen.
    health.record_frame_committed(now, 1);
    assert_eq!(health.poll(now, false), RenderHealthAction::Healthy);
    assert!(!health.is_behind());
    assert_eq!(health.stalled_count(), 0);
}

#[test]
fn recovery_is_throttled_against_recursion() {
    let mut health = RenderHealth::default();
    let now = Instant::now();
    let wanted = earlier(now, STALL_BUDGET + Duration::from_secs(1));
    health.note_state_change(wanted);
    assert_eq!(health.poll(now, true), RenderHealthAction::Recover);
    health.note_recovery(now);
    assert_eq!(health.stalled_count(), 1);
    // Immediately after a recovery the watchdog must not fire again, even
    // though a frame is still wanted — the replacement frame needs room to
    // settle. This is what prevents a redraw storm on a dead terminal.
    assert_eq!(health.poll(now, true), RenderHealthAction::Healthy);
}

#[test]
fn recovery_re_arms_after_throttle_window() {
    let mut health = RenderHealth::default();
    let now = Instant::now();
    // First recovery at an instant comfortably in the past.
    let first = earlier(now, RECOVERY_THROTTLE + Duration::from_secs(1));
    health.note_recovery(first);
    assert_eq!(health.stalled_count(), 1);
    // A still-wanted frame whose want predates everything is now past both the
    // stall budget and the throttle window ⇒ the watchdog re-arms.
    health.note_state_change(earlier(now, STALL_BUDGET + Duration::from_secs(2)));
    assert_eq!(health.poll(now, true), RenderHealthAction::Recover);
}

#[test]
fn note_state_change_does_not_reset_existing_stall_clock() {
    let mut health = RenderHealth::default();
    let now = Instant::now();
    let first_want = earlier(now, STALL_BUDGET + Duration::from_millis(800));
    health.note_state_change(first_want);
    // A second, newer state change must NOT push the stall clock forward — the
    // watchdog measures the age of the OLDEST uncommitted change.
    let newer = earlier(now, Duration::from_millis(10));
    health.note_state_change(newer);
    assert_eq!(health.poll(now, true), RenderHealthAction::Recover);
}

#[test]
fn poll_arms_clock_for_resize_or_animation_wants() {
    // A want routed purely through `pending_resize` / animation sets
    // `wants_draw` without a `note_state_change`. `poll` must still arm the
    // stall clock so such frames are watched too.
    let mut health = RenderHealth::default();
    let now = Instant::now();
    // First poll arms the clock at `now`; inside the budget ⇒ healthy.
    assert_eq!(health.poll(now, true), RenderHealthAction::Healthy);
    assert!(health.pending_for(now).is_some());
}

#[test]
fn diagnostics_reports_revisions_and_stall_count() {
    let mut health = RenderHealth::default();
    let now = Instant::now();
    health.note_state_change(earlier(now, STALL_BUDGET + Duration::from_millis(100)));
    let line = health.diagnostics(now);
    assert!(line.contains("stuck-render watchdog"), "{line}");
    assert!(line.contains("state_rev=1"), "{line}");
    assert!(line.contains("drawn_rev=0"), "{line}");
    assert!(line.contains("stalls=0"), "{line}");
    // After a recovery the stall count is reflected.
    health.note_recovery(now);
    assert!(health.diagnostics(now).contains("stalls=1"));
}

#[test]
fn pending_for_is_none_when_up_to_date() {
    let mut health = RenderHealth::default();
    let now = Instant::now();
    assert!(health.pending_for(now).is_none());
    health.note_state_change(now);
    health.record_frame_committed(now, 3);
    assert!(health.pending_for(now).is_none());
}
