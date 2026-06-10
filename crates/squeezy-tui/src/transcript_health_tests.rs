//! Unit tests for the Transcript Health Markers model (§12.5.7). Pure: no
//! terminal, no rendering — they exercise the per-kind detection from measured
//! facts, severity ordering, the empty/healthy case, the staleness fast path,
//! navigation, and the summary. They prove the detector classifies from *facts*
//! only and never re-reads output text (so it cannot leak a hidden secret).

use super::*;

/// A healthy candidate the tests mutate one fact at a time. `revision` 0 by
/// default; `title` a short, secret-free label.
fn healthy(id: u64) -> HealthCandidate {
    HealthCandidate {
        id,
        revision: 0,
        title: "shell".to_string(),
        tool_failed: false,
        subagent_failed: false,
        turn_failed: false,
        elided: false,
        hidden_lines: 0,
        output_bytes: 0,
    }
}

#[test]
fn detects_tool_failed_marker() {
    let mut c = healthy(7);
    c.tool_failed = true;
    let markers = detect_for_candidate(&c);
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].entry_id, 7);
    assert_eq!(markers[0].kind, HealthKind::ToolFailed);
    assert_eq!(markers[0].severity, HealthSeverity::Important);
    assert!(markers[0].message.contains("shell"), "{:?}", markers[0]);
    assert!(markers[0].message.contains("failed"));
}

#[test]
fn detects_subagent_failed_marker() {
    let mut c = healthy(3);
    c.title = "subagent".to_string();
    c.subagent_failed = true;
    let markers = detect_for_candidate(&c);
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].kind, HealthKind::SubagentFailed);
    assert_eq!(markers[0].severity, HealthSeverity::Important);
}

#[test]
fn detects_turn_failed_marker() {
    let mut c = healthy(9);
    c.title = "turn".to_string();
    c.turn_failed = true;
    let markers = detect_for_candidate(&c);
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].kind, HealthKind::TurnFailed);
}

#[test]
fn detects_output_elided_with_hidden_count() {
    let mut c = healthy(2);
    c.elided = true;
    c.hidden_lines = 42;
    let markers = detect_for_candidate(&c);
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].kind, HealthKind::OutputElided);
    assert_eq!(markers[0].severity, HealthSeverity::Minor);
    assert!(markers[0].message.contains("+42 lines"), "{:?}", markers[0]);
}

#[test]
fn detects_large_output_when_not_elided() {
    let mut c = healthy(4);
    c.output_bytes = LARGE_OUTPUT_BYTES;
    let markers = detect_for_candidate(&c);
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].kind, HealthKind::LargeOutput);
    assert_eq!(markers[0].severity, HealthSeverity::Minor);
    // The human byte-size appears in the message.
    assert!(markers[0].message.contains("KB"), "{:?}", markers[0]);
}

#[test]
fn large_output_just_below_threshold_emits_nothing() {
    let mut c = healthy(4);
    c.output_bytes = LARGE_OUTPUT_BYTES - 1;
    assert!(detect_for_candidate(&c).is_empty());
}

#[test]
fn elided_wins_over_large_size_heads_up() {
    // An entry that is BOTH elided and large gets exactly one hidden-content
    // marker — the more actionable "elided", not a duplicate size badge.
    let mut c = healthy(5);
    c.elided = true;
    c.hidden_lines = 10;
    c.output_bytes = LARGE_OUTPUT_BYTES * 4;
    let markers = detect_for_candidate(&c);
    let kinds: Vec<_> = markers.iter().map(|m| m.kind).collect();
    assert!(kinds.contains(&HealthKind::OutputElided), "{kinds:?}");
    assert!(!kinds.contains(&HealthKind::LargeOutput), "{kinds:?}");
}

#[test]
fn failed_and_elided_emit_both_markers_severity_ordered() {
    // A failed tool whose output is ALSO elided produces an important failure
    // marker followed by a minor elision marker — the important one first.
    let mut c = healthy(6);
    c.tool_failed = true;
    c.elided = true;
    c.hidden_lines = 3;
    let markers = detect_for_candidate(&c);
    assert_eq!(markers.len(), 2);
    assert_eq!(markers[0].kind, HealthKind::ToolFailed);
    assert_eq!(markers[0].severity, HealthSeverity::Important);
    assert_eq!(markers[1].kind, HealthKind::OutputElided);
    assert_eq!(markers[1].severity, HealthSeverity::Minor);
    assert!(markers[0].severity < markers[1].severity);
}

#[test]
fn healthy_candidate_emits_nothing() {
    assert!(detect_for_candidate(&healthy(1)).is_empty());
}

#[test]
fn rebuild_is_fast_path_when_fingerprint_unchanged() {
    let mut model = HealthMarkers::new();
    let mut c = healthy(1);
    c.tool_failed = true;
    let candidates = vec![c];
    let fp = HealthMarkers::fingerprint_of(candidates.iter());

    assert!(
        model.rebuild_if_stale(fp, &candidates),
        "first build recomputes"
    );
    assert_eq!(model.len(), 1);
    assert_eq!(model.fingerprint(), fp);

    // Same fingerprint: the fast path returns without recomputing.
    assert!(
        !model.rebuild_if_stale(fp, &candidates),
        "unchanged fingerprint is the zero-cost fast path"
    );
    assert_eq!(model.len(), 1);
}

#[test]
fn rebuild_recomputes_when_revision_moves() {
    let mut model = HealthMarkers::new();
    let mut c = healthy(1);
    c.tool_failed = true;
    let first = vec![c.clone()];
    let fp1 = HealthMarkers::fingerprint_of(first.iter());
    assert!(model.rebuild_if_stale(fp1, &first));

    // A revision bump moves the fingerprint, forcing a recompute.
    c.revision = 1;
    let second = vec![c];
    let fp2 = HealthMarkers::fingerprint_of(second.iter());
    assert_ne!(fp1, fp2, "a revision bump moves the fingerprint");
    assert!(model.rebuild_if_stale(fp2, &second));
}

#[test]
fn empty_transcript_builds_without_re_scanning() {
    let mut model = HealthMarkers::new();
    let candidates: Vec<HealthCandidate> = Vec::new();
    let fp = HealthMarkers::fingerprint_of(candidates.iter());
    assert!(model.rebuild_if_stale(fp, &candidates), "first build runs");
    assert!(model.is_empty());
    // A genuinely empty/healthy transcript is not re-scanned every refresh.
    assert!(
        !model.rebuild_if_stale(fp, &candidates),
        "empty is built, not re-scanned"
    );
}

#[test]
fn navigation_wraps_both_directions() {
    let mut model = HealthMarkers::new();
    let mut a = healthy(1);
    a.tool_failed = true;
    let mut b = healthy(2);
    b.turn_failed = true;
    b.title = "turn".to_string();
    let candidates = vec![a, b];
    let fp = HealthMarkers::fingerprint_of(candidates.iter());
    model.rebuild_if_stale(fp, &candidates);
    assert_eq!(model.len(), 2);

    assert_eq!(model.next_index(None), Some(0));
    assert_eq!(model.next_index(Some(0)), Some(1));
    assert_eq!(model.next_index(Some(1)), Some(0), "wraps forward");

    assert_eq!(model.prev_index(None), Some(1));
    assert_eq!(model.prev_index(Some(0)), Some(1), "wraps backward");
    assert_eq!(model.prev_index(Some(1)), Some(0));
}

#[test]
fn navigation_on_empty_is_none() {
    let model = HealthMarkers::new();
    assert_eq!(model.next_index(None), None);
    assert_eq!(model.prev_index(None), None);
}

#[test]
fn summary_lists_total_and_per_kind_counts() {
    let mut model = HealthMarkers::new();
    let mut a = healthy(1);
    a.tool_failed = true;
    let mut b = healthy(2);
    b.tool_failed = true;
    let mut c = healthy(3);
    c.elided = true;
    c.hidden_lines = 5;
    let candidates = vec![a, b, c];
    let fp = HealthMarkers::fingerprint_of(candidates.iter());
    model.rebuild_if_stale(fp, &candidates);

    let summary = model.summary();
    assert!(summary.contains("3 markers"), "{summary}");
    assert!(summary.contains("2 tool failed"), "{summary}");
    assert!(summary.contains("1 output elided"), "{summary}");
    assert_eq!(model.count_of(HealthKind::ToolFailed), 2);
    assert_eq!(model.count_of(HealthKind::OutputElided), 1);
    assert_eq!(model.count_of(HealthKind::LargeOutput), 0);
}

#[test]
fn summary_is_empty_when_no_markers() {
    let model = HealthMarkers::new();
    assert!(model.summary().is_empty());
}

#[test]
fn message_is_bounded() {
    // A pathologically long title cannot blow up the overlay row.
    let mut c = healthy(1);
    c.tool_failed = true;
    c.title = "x".repeat(10_000);
    let markers = detect_for_candidate(&c);
    assert_eq!(markers.len(), 1);
    assert!(
        markers[0].message.chars().count() <= MESSAGE_CAP + 1,
        "message capped: {} chars",
        markers[0].message.chars().count()
    );
}

#[test]
fn elided_without_hidden_count_still_marks() {
    // Elision is known but the exact hidden count is not — still a marker, with a
    // generic message (no bogus "+0 lines").
    let mut c = healthy(8);
    c.elided = true;
    c.hidden_lines = 0;
    let markers = detect_for_candidate(&c);
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].kind, HealthKind::OutputElided);
    assert!(!markers[0].message.contains("+0"), "{:?}", markers[0]);
}
