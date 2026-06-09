//! Unit tests for the UX latency-budget primitives (§12.10.1): the
//! interaction-kind table, the bounded sample ring + percentiles, the budget
//! comparison / last-violation tracking, and the overlay formatting.
//!
//! These exercise the module-local logic directly. The end-to-end wiring (that
//! the event loop tags a frame and `draw_app` records a sample, and that the
//! overlay only paints when toggled on) is covered by the integration tests in
//! `lib_tests.rs` against the capture-sink guard.

use super::*;
use std::time::Duration;

#[test]
fn interaction_kinds_have_unique_padded_labels() {
    // The overlay lines up columns, so every label must be the same display
    // width and distinct (a collision would conflate two interactions' rows).
    let width = InteractionKind::ALL[0].label().chars().count();
    let mut seen = std::collections::BTreeSet::new();
    for kind in InteractionKind::ALL {
        let label = kind.label();
        assert_eq!(
            label.chars().count(),
            width,
            "labels must be fixed-width for column alignment: {label:?}"
        );
        assert!(seen.insert(label), "duplicate label {label:?}");
    }
    // All eight spec interactions are represented.
    assert_eq!(InteractionKind::ALL.len(), 8);
}

#[test]
fn every_budget_orders_p99_at_or_above_p95() {
    // A mis-ordered budget would let p99 flag "better than p95", which is
    // nonsense; the constructor must clamp it.
    for kind in InteractionKind::ALL {
        let b = kind.budget();
        assert!(
            b.p99 >= b.p95,
            "{}: p99 {:?} must be >= p95 {:?}",
            kind.label().trim_end(),
            b.p99,
            b.p95
        );
    }
}

#[test]
fn budget_constructor_clamps_inverted_pair() {
    let b = TuiLatencyBudget::new(Duration::from_millis(20), Duration::from_millis(5));
    assert_eq!(b.p95, Duration::from_millis(20));
    assert_eq!(
        b.p99,
        Duration::from_millis(20),
        "p99 clamps up to p95 when supplied smaller"
    );
}

#[test]
fn tracker_starts_empty_and_records_nothing_for_unobserved_kinds() {
    let tracker = LatencyTracker::default();
    assert_eq!(tracker.observed_kinds(), 0);
    assert!(tracker.last_violation().is_none());
    assert!(tracker.percentiles(InteractionKind::Scroll).is_none());
    // An empty tracker paints no overlay panel — the zero-idle-cost contract.
    assert!(tracker.overlay_lines().is_empty());
}

#[test]
fn within_budget_samples_record_but_do_not_violate() {
    let mut tracker = LatencyTracker::default();
    // KeypressEcho budget is p95=8ms / p99=16ms; 1ms frames are comfortably in.
    for _ in 0..50 {
        let v = tracker.record(InteractionKind::KeypressEcho, Duration::from_millis(1), 1);
        assert!(v.is_none(), "an in-budget sample never violates");
    }
    assert_eq!(tracker.observed_kinds(), 1);
    assert!(tracker.last_violation().is_none());
    let (p95, p99) = tracker
        .percentiles(InteractionKind::KeypressEcho)
        .expect("samples recorded");
    assert_eq!(p95, Duration::from_millis(1));
    assert_eq!(p99, Duration::from_millis(1));
}

#[test]
fn an_over_budget_p99_is_detected_and_remembered() {
    let mut tracker = LatencyTracker::default();
    // 99 fast frames + 1 very slow one: the slow one is the p99 with 100
    // samples (but the window caps at 64, so fill with fewer to keep the slow
    // one in-window and at/above the 99th rank).
    for _ in 0..63 {
        assert!(
            tracker
                .record(InteractionKind::KeypressEcho, Duration::from_millis(1), 1)
                .is_none()
        );
    }
    // One frame far over the p99=16ms keypress budget.
    let violation = tracker
        .record(InteractionKind::KeypressEcho, Duration::from_millis(40), 7)
        .expect("a 40ms keypress frame blows the 16ms p99 budget");
    assert_eq!(violation.kind, InteractionKind::KeypressEcho);
    assert_eq!(violation.percentile, 99, "p99 is the stronger signal");
    assert_eq!(violation.observed, Duration::from_millis(40));
    assert_eq!(violation.budget, Duration::from_millis(16));
    assert_eq!(
        violation.frame, 7,
        "the detecting frame ordinal is recorded"
    );
    // The violation persists even after in-budget frames push it toward the
    // window edge — it is remembered, not recomputed each call.
    assert_eq!(tracker.last_violation(), Some(violation));
}

#[test]
fn ring_is_bounded_and_overwrites_oldest() {
    let mut tracker = LatencyTracker::default();
    // Fill the window with one huge sample then flood with tiny ones until the
    // huge sample is evicted; the percentile must then drop back into budget.
    tracker.record(InteractionKind::Scroll, Duration::from_millis(100), 1);
    for _ in 0..WINDOW {
        tracker.record(InteractionKind::Scroll, Duration::from_micros(200), 2);
    }
    let (p95, p99) = tracker
        .percentiles(InteractionKind::Scroll)
        .expect("samples recorded");
    assert_eq!(
        p95,
        Duration::from_micros(200),
        "the 100ms outlier was evicted once the ring wrapped a full window"
    );
    assert_eq!(p99, Duration::from_micros(200));
}

#[test]
fn percentile_uses_nearest_rank() {
    // 100 samples 1..=100 ms: nearest-rank p95 = rank ceil(0.95*100)=95 → 95ms,
    // p99 = rank 99 → 99ms. Window caps at 64, so use exactly 64 samples and
    // recompute the expected ranks for n=64.
    let mut tracker = LatencyTracker::default();
    for ms in 1..=WINDOW as u64 {
        tracker.record(InteractionKind::PageJump, Duration::from_millis(ms), 1);
    }
    let (p95, p99) = tracker
        .percentiles(InteractionKind::PageJump)
        .expect("samples recorded");
    // n=64: rank95 = ceil(0.95*64)=61 → value 61ms; rank99 = ceil(0.99*64)=64 → 64ms.
    assert_eq!(p95, Duration::from_millis(61));
    assert_eq!(p99, Duration::from_millis(64));
}

#[test]
fn overlay_lines_mark_violations_and_show_last_violation() {
    let mut tracker = LatencyTracker::default();
    // One in-budget interaction and one over-budget interaction.
    tracker.record(InteractionKind::Scroll, Duration::from_micros(500), 1);
    tracker.record(InteractionKind::ResizeRedraw, Duration::from_millis(200), 9);
    let lines = tracker.overlay_lines();
    let joined = lines.join("\n");
    assert!(joined.contains("latency p95/p99 (budget)"), "{joined}");
    // The in-budget scroll row carries no `!` marker.
    let scroll_row = lines
        .iter()
        .find(|l| l.contains("scroll"))
        .expect("scroll row present");
    assert!(
        scroll_row.trim_start().starts_with("scroll"),
        "in-budget row has no leading bang: {scroll_row:?}"
    );
    // The over-budget resize row is flagged.
    let resize_row = lines
        .iter()
        .find(|l| l.contains("resize"))
        .expect("resize row present");
    assert!(
        resize_row.starts_with('!'),
        "over-budget row is flagged with `!`: {resize_row:?}"
    );
    // The last-violation summary names the resize interaction and frame 9.
    assert!(joined.contains("last !resize"), "{joined}");
    assert!(joined.contains("@f9"), "{joined}");
}

#[test]
fn fmt_dur_matches_metrics_formatting() {
    assert_eq!(fmt_dur(Duration::from_micros(0)), "0µs");
    assert_eq!(fmt_dur(Duration::from_micros(999)), "999µs");
    assert_eq!(fmt_dur(Duration::from_micros(1000)), "1.0ms");
    assert_eq!(
        fmt_dur(Duration::from_millis(12) + Duration::from_micros(345)),
        "12.3ms"
    );
}

#[test]
fn distinct_interactions_track_independently() {
    let mut tracker = LatencyTracker::default();
    tracker.record(InteractionKind::CopyAck, Duration::from_millis(2), 1);
    tracker.record(InteractionKind::SearchJump, Duration::from_millis(3), 2);
    assert_eq!(tracker.observed_kinds(), 2);
    assert_eq!(
        tracker.percentiles(InteractionKind::CopyAck),
        Some((Duration::from_millis(2), Duration::from_millis(2)))
    );
    assert_eq!(
        tracker.percentiles(InteractionKind::SearchJump),
        Some((Duration::from_millis(3), Duration::from_millis(3)))
    );
    // A kind never sampled stays absent.
    assert!(tracker.percentiles(InteractionKind::QueueDrag).is_none());
}
