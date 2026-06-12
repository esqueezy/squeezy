//! Unit tests for the Attention Routing model (§12.8.6): the per-record
//! classifier (failure / rejection / blocker / approval / pinned-completion /
//! quiet), the loudest-first priority order + quick-jump target, the
//! latest-line blocker/approval detection (and approval-wins-over-blocker
//! tiebreak), the pinned-completion opt-in, the staleness fingerprint fast path,
//! the indicator readout, and the "quiet never surfaces" invariant. Pure over
//! `SubagentTimelineSource` slices — no terminal, no `TuiApp`. The end-to-end
//! keyboard/mouse/render coverage lives in `lib_tests.rs`.

use super::*;
use crate::subagent_timeline::{SubagentTimelineSource, SubagentTimelineStatus};

/// A `SubagentTimelineSource` builder with sensible defaults so each test states
/// only the fields it cares about.
fn source(id: u64, status: SubagentTimelineStatus, latest: &str) -> SubagentTimelineSource {
    SubagentTimelineSource {
        id,
        agent: format!("agent-{id}"),
        status,
        latest: latest.to_string(),
        elapsed_secs: Some(0),
        tool_count: 0,
        cost_micros: None,
    }
}

/// Wrap a source as an unpinned attention source.
fn unpinned(source: &SubagentTimelineSource) -> AttentionSource<'_> {
    AttentionSource {
        source,
        pinned: false,
    }
}

#[test]
fn all_kinds_have_distinct_nonempty_labels_and_are_exhaustive() {
    let mut seen = std::collections::HashSet::new();
    for kind in SubagentAttentionKind::ALL.iter().copied() {
        let label = kind.label();
        assert!(!label.is_empty(), "{kind:?} has an empty label");
        assert!(seen.insert(label), "duplicate label {label:?}");
    }
    // Every kind appears in ALL (exhaustiveness guard against a missed variant).
    const {
        assert!(SubagentAttentionKind::ALL.len() == 6);
    }
}

#[test]
fn quiet_is_the_only_non_attention_kind() {
    for kind in SubagentAttentionKind::ALL.iter().copied() {
        match kind {
            SubagentAttentionKind::Quiet => assert!(!kind.is_attention()),
            _ => assert!(kind.is_attention(), "{kind:?} should want attention"),
        }
    }
}

#[test]
fn priority_order_is_loudest_first() {
    // The Ord derive must rank failure ahead of every other class, and quiet last.
    assert!(SubagentAttentionKind::Failure < SubagentAttentionKind::Rejection);
    assert!(SubagentAttentionKind::Rejection < SubagentAttentionKind::Blocker);
    assert!(SubagentAttentionKind::Blocker < SubagentAttentionKind::Approval);
    assert!(SubagentAttentionKind::Approval < SubagentAttentionKind::PinnedCompletion);
    assert!(SubagentAttentionKind::PinnedCompletion < SubagentAttentionKind::Quiet);
}

#[test]
fn classify_failed_and_rejected_are_loud_regardless_of_latest() {
    let failed = source(1, SubagentTimelineStatus::Failed, "all good actually");
    assert_eq!(classify(&failed, false), SubagentAttentionKind::Failure);
    let rejected = source(2, SubagentTimelineStatus::Rejected, "");
    assert_eq!(classify(&rejected, false), SubagentAttentionKind::Rejection);
}

#[test]
fn classify_running_is_quiet_unless_a_wait_signal_is_present() {
    let calm = source(1, SubagentTimelineStatus::Running, "running cargo test");
    assert_eq!(classify(&calm, false), SubagentAttentionKind::Quiet);

    let blocked = source(2, SubagentTimelineStatus::Running, "blocked on a lock file");
    assert_eq!(classify(&blocked, false), SubagentAttentionKind::Blocker);

    let waiting = source(3, SubagentTimelineStatus::Running, "waiting for the build");
    assert_eq!(classify(&waiting, false), SubagentAttentionKind::Blocker);
}

#[test]
fn classify_running_routes_approval_signals() {
    for line in [
        "awaiting approval to run rm -rf",
        "needs approval for the edit",
        "permission required to write",
        "awaiting input from the user",
        "confirm to continue",
    ] {
        let src = source(1, SubagentTimelineStatus::Running, line);
        assert_eq!(
            classify(&src, false),
            SubagentAttentionKind::Approval,
            "line {line:?} should route to Approval",
        );
    }
}

#[test]
fn approval_wins_over_blocker_when_both_phrases_match() {
    // "waiting for approval" contains both a blocker substring ("waiting for")
    // and an approval one; the approval gate is checked first, so it wins.
    let src = source(1, SubagentTimelineStatus::Running, "waiting for approval");
    assert_eq!(classify(&src, false), SubagentAttentionKind::Approval);
}

#[test]
fn classify_completed_is_quiet_unless_pinned() {
    let done = source(1, SubagentTimelineStatus::Completed, "found the bug");
    assert_eq!(classify(&done, false), SubagentAttentionKind::Quiet);
    assert_eq!(
        classify(&done, true),
        SubagentAttentionKind::PinnedCompletion,
        "a pinned completion opts into a notification",
    );
}

#[test]
fn rebuild_drops_quiet_and_orders_loudest_first() {
    let sources = [
        source(1, SubagentTimelineStatus::Completed, "done"), // quiet, dropped
        source(2, SubagentTimelineStatus::Running, "awaiting approval"), // approval
        source(3, SubagentTimelineStatus::Failed, "boom"),    // failure
        source(4, SubagentTimelineStatus::Running, "cargo test"), // quiet, dropped
        source(5, SubagentTimelineStatus::Rejected, ""),      // rejection
    ];
    let attn: Vec<AttentionSource<'_>> = sources.iter().map(unpinned).collect();
    let mut route = AttentionRoute::new();
    let fp = AttentionRoute::fingerprint_of(attn.iter());
    assert!(route.rebuild_if_stale(fp, &attn));

    // Three loud items (failure, rejection, approval), loudest-first.
    assert_eq!(route.len(), 3);
    let kinds: Vec<_> = route.items().iter().map(|i| i.kind).collect();
    assert_eq!(
        kinds,
        vec![
            SubagentAttentionKind::Failure,
            SubagentAttentionKind::Rejection,
            SubagentAttentionKind::Approval,
        ],
    );
    // The top (quick-jump) target is the failure.
    assert_eq!(route.top().map(|i| i.id), Some(3));
}

#[test]
fn same_kind_keeps_record_order_tiebreak() {
    let sources = [
        source(7, SubagentTimelineStatus::Failed, "second-but-id-7"),
        source(3, SubagentTimelineStatus::Failed, "first-by-record-order"),
    ];
    let attn: Vec<AttentionSource<'_>> = sources.iter().map(unpinned).collect();
    let mut route = AttentionRoute::new();
    let fp = AttentionRoute::fingerprint_of(attn.iter());
    route.rebuild_if_stale(fp, &attn);
    // Both failures: the one earlier in record order (ordinal 1, id 7) sorts first
    // even though its id is larger — the tiebreak is source position, not id.
    assert_eq!(route.items()[0].id, 7);
    assert_eq!(route.items()[1].id, 3);
}

#[test]
fn empty_when_everything_is_calm() {
    let sources = [
        source(1, SubagentTimelineStatus::Running, "thinking"),
        source(2, SubagentTimelineStatus::Completed, "done"),
    ];
    let attn: Vec<AttentionSource<'_>> = sources.iter().map(unpinned).collect();
    let mut route = AttentionRoute::new();
    let fp = AttentionRoute::fingerprint_of(attn.iter());
    route.rebuild_if_stale(fp, &attn);
    assert!(route.is_empty());
    assert!(route.top().is_none());
    assert!(route.indicator().is_empty());
}

#[test]
fn fingerprint_fast_path_skips_unchanged_rebuild() {
    let sources = [source(1, SubagentTimelineStatus::Failed, "boom")];
    let attn: Vec<AttentionSource<'_>> = sources.iter().map(unpinned).collect();
    let mut route = AttentionRoute::new();
    let fp = AttentionRoute::fingerprint_of(attn.iter());
    assert!(route.rebuild_if_stale(fp, &attn), "first build runs");
    assert!(
        !route.rebuild_if_stale(fp, &attn),
        "same fingerprint short-circuits",
    );
    assert_eq!(route.fingerprint(), fp);
}

#[test]
fn fingerprint_moves_on_status_pin_and_latest_change() {
    let base = source(1, SubagentTimelineStatus::Running, "thinking");
    let fp_base = AttentionRoute::fingerprint_of([unpinned(&base)].iter());

    let failed = source(1, SubagentTimelineStatus::Failed, "thinking");
    let fp_status = AttentionRoute::fingerprint_of([unpinned(&failed)].iter());
    assert_ne!(fp_base, fp_status, "a status flip moves the fingerprint");

    let pinned = AttentionSource {
        source: &base,
        pinned: true,
    };
    let fp_pin = AttentionRoute::fingerprint_of([pinned].iter());
    assert_ne!(fp_base, fp_pin, "a pin toggle moves the fingerprint");

    let new_line = source(1, SubagentTimelineStatus::Running, "blocked on lock");
    let fp_line = AttentionRoute::fingerprint_of([unpinned(&new_line)].iter());
    assert_ne!(
        fp_base, fp_line,
        "a fresh activity line moves the fingerprint"
    );
}

#[test]
fn indicator_leads_with_count_and_breaks_down_by_kind() {
    let sources = [
        source(1, SubagentTimelineStatus::Failed, "boom"),
        source(2, SubagentTimelineStatus::Failed, "boom2"),
        source(3, SubagentTimelineStatus::Running, "awaiting approval"),
    ];
    let attn: Vec<AttentionSource<'_>> = sources.iter().map(unpinned).collect();
    let mut route = AttentionRoute::new();
    let fp = AttentionRoute::fingerprint_of(attn.iter());
    route.rebuild_if_stale(fp, &attn);
    let indicator = route.indicator();
    assert!(indicator.starts_with("!3 attention"), "got {indicator:?}");
    assert!(indicator.contains("2 failed"), "got {indicator:?}");
    assert!(indicator.contains("1 approval"), "got {indicator:?}");
    assert_eq!(route.count_of(SubagentAttentionKind::Failure), 2);
}

#[test]
fn next_index_wraps_over_the_routed_list() {
    let sources = [
        source(1, SubagentTimelineStatus::Failed, "a"),
        source(2, SubagentTimelineStatus::Rejected, ""),
    ];
    let attn: Vec<AttentionSource<'_>> = sources.iter().map(unpinned).collect();
    let mut route = AttentionRoute::new();
    let fp = AttentionRoute::fingerprint_of(attn.iter());
    route.rebuild_if_stale(fp, &attn);
    assert_eq!(route.next_index(None), Some(0));
    assert_eq!(route.next_index(Some(0)), Some(1));
    assert_eq!(route.next_index(Some(1)), Some(0), "wraps at the end");
}

#[test]
fn next_index_is_none_when_empty() {
    let route = AttentionRoute::new();
    assert_eq!(route.next_index(None), None);
    assert_eq!(route.next_index(Some(0)), None);
}
