//! Unit tests for the Related-Entry Links (§12.5.3) pure relation graph.

use super::*;

fn e(id: u64, kind: RelationEntryKind) -> RelationEntry {
    RelationEntry {
        id,
        revision: 0,
        kind,
        is_error: false,
        tool_name: None,
    }
}

fn tool(id: u64, name: &str, is_error: bool) -> RelationEntry {
    RelationEntry {
        id,
        revision: 0,
        kind: RelationEntryKind::ToolCall,
        is_error,
        tool_name: Some(name.to_string()),
    }
}

fn err(id: u64) -> RelationEntry {
    RelationEntry {
        id,
        revision: 0,
        kind: RelationEntryKind::Error,
        is_error: true,
        tool_name: None,
    }
}

fn build(entries: &[RelationEntry]) -> RelationGraph {
    let mut graph = RelationGraph::new();
    let fp = RelationGraph::fingerprint_of(entries.iter());
    graph.rebuild_if_stale(fp, entries);
    graph
}

/// The set of target ids related to `id`, ignoring rank — handy for membership
/// assertions where order is tested separately.
fn targets(graph: &RelationGraph, id: u64) -> Vec<u64> {
    graph.relations(id).iter().map(|r| r.target).collect()
}

#[test]
fn empty_graph_has_no_relations() {
    let graph = build(&[]);
    assert!(graph.relations(1).is_empty());
    assert!(!graph.has_relations(1));
    assert_eq!(graph.count(1), 0);
    assert_eq!(graph.target_at(1, 0), None);
}

#[test]
fn user_links_to_following_assistant_reply() {
    let entries = [
        e(1, RelationEntryKind::User),
        e(2, RelationEntryKind::Assistant),
    ];
    let graph = build(&entries);
    // Both directions exist (the jump is reversible).
    assert_eq!(targets(&graph, 1), vec![2]);
    assert_eq!(targets(&graph, 2), vec![1]);
    assert_eq!(graph.relations(1)[0].kind, RelationKind::Response);
    assert_eq!(graph.relations(1)[0].confidence, Confidence::High);
}

#[test]
fn assistant_links_to_tool_calls_in_its_turn() {
    let entries = [
        e(1, RelationEntryKind::User),
        e(2, RelationEntryKind::Assistant),
        tool(3, "shell", false),
        tool(4, "edit", false),
        // A new user turn closes the assistant's tool window.
        e(5, RelationEntryKind::User),
        tool(6, "shell", false),
    ];
    let graph = build(&entries);
    // The assistant relates to the two tools in its own turn, not the tool in
    // the later turn.
    let assistant = targets(&graph, 2);
    assert!(assistant.contains(&3), "tool 3 in turn: {assistant:?}");
    assert!(assistant.contains(&4), "tool 4 in turn: {assistant:?}");
    assert!(
        !assistant.contains(&6),
        "tool 6 is a later turn: {assistant:?}"
    );
    // Reverse direction exists too.
    assert!(targets(&graph, 3).contains(&2));
    assert_eq!(graph.relations(3)[0].kind, RelationKind::ToolInvocation);
}

#[test]
fn error_links_back_to_cause_and_forward_to_followup() {
    let entries = [
        e(1, RelationEntryKind::Assistant),
        tool(2, "shell", false),
        err(3),
        e(4, RelationEntryKind::User),
    ];
    let graph = build(&entries);
    let from_error = targets(&graph, 3);
    // CausedBy the preceding tool call.
    assert!(from_error.contains(&2), "caused by tool: {from_error:?}");
    // Followup user turn.
    assert!(from_error.contains(&4), "follow-up turn: {from_error:?}");
    // The cause is ranked above the weak follow-up (medium > low).
    let kinds: Vec<_> = graph
        .relations(3)
        .iter()
        .map(|r| (r.kind, r.confidence))
        .collect();
    let cause_pos = kinds.iter().position(|(k, _)| *k == RelationKind::CausedBy);
    let followup_pos = kinds.iter().position(|(k, _)| *k == RelationKind::Followup);
    assert!(
        cause_pos < followup_pos,
        "cause (medium) ranks above follow-up (low): {kinds:?}",
    );
}

#[test]
fn failed_tool_links_to_error_via_cause_chain() {
    let entries = [tool(1, "edit", true), err(2)];
    let graph = build(&entries);
    // The error's CausedBy points at the failed tool; the reverse (Caused) is on
    // the tool side.
    assert!(targets(&graph, 2).contains(&1));
    assert!(targets(&graph, 1).contains(&2));
    assert_eq!(graph.relations(2)[0].kind, RelationKind::CausedBy);
    assert_eq!(graph.relations(1)[0].kind, RelationKind::Caused);
}

#[test]
fn same_tool_calls_chain_low_confidence() {
    let entries = [
        tool(1, "shell", false),
        e(2, RelationEntryKind::Assistant),
        tool(3, "shell", false),
        tool(4, "edit", false),
    ];
    let graph = build(&entries);
    // shell@1 chains forward to shell@3 (not edit@4).
    let from_1 = graph.relations(1);
    assert!(
        from_1
            .iter()
            .any(|r| r.target == 3 && r.kind == RelationKind::SameTool),
        "{from_1:?}",
    );
    assert!(
        !targets(&graph, 1).contains(&4),
        "different tool not linked"
    );
    // It is a weak link.
    let same = from_1.iter().find(|r| r.target == 3).unwrap();
    assert_eq!(same.confidence, Confidence::Low);
}

#[test]
fn subagent_breadcrumbs_chain() {
    let entries = [
        e(1, RelationEntryKind::Subagent),
        e(2, RelationEntryKind::Subagent),
        e(3, RelationEntryKind::Subagent),
    ];
    let graph = build(&entries);
    assert!(targets(&graph, 1).contains(&2));
    assert!(targets(&graph, 2).contains(&3));
    assert_eq!(graph.relations(1)[0].kind, RelationKind::Subagent);
}

#[test]
fn relations_are_ranked_strongest_first() {
    // Assistant has a high tool link and (via its error) is reachable from a
    // medium cause; the high link must sort first.
    let entries = [
        e(1, RelationEntryKind::User),
        e(2, RelationEntryKind::Assistant),
        tool(3, "shell", true),
        err(4),
    ];
    let graph = build(&entries);
    // The assistant relates to: user(1, Response high), tool(3, ToolInvocation
    // high), and error(4, CausedBy medium). High links rank before the medium.
    let confidences: Vec<_> = graph.relations(2).iter().map(|r| r.confidence).collect();
    assert!(
        confidences.windows(2).all(|w| w[0] >= w[1]),
        "relations descend by confidence: {confidences:?}",
    );
    assert_eq!(confidences.first().copied(), Some(Confidence::High));
}

#[test]
fn target_at_walks_the_ranked_list() {
    let entries = [
        e(1, RelationEntryKind::User),
        e(2, RelationEntryKind::Assistant),
        tool(3, "shell", false),
    ];
    let graph = build(&entries);
    let count = graph.count(2);
    assert!(count >= 2, "assistant relates to user + tool");
    assert_eq!(graph.target_at(2, 0), Some(graph.relations(2)[0].target));
    assert_eq!(graph.target_at(2, count), None, "out of range is None");
}

#[test]
fn duplicate_targets_are_deduped_keeping_strongest() {
    // A failed tool both invokes (high) and causes (caused) the same error path
    // is contrived, but two rules can name the same target; the dedupe must keep
    // exactly one edge per target.
    let entries = [
        e(1, RelationEntryKind::Assistant),
        tool(2, "shell", false),
        tool(3, "shell", true),
        err(4),
    ];
    let graph = build(&entries);
    for id in [1u64, 2, 3, 4] {
        let mut seen = std::collections::HashSet::new();
        for relation in graph.relations(id) {
            assert!(
                seen.insert(relation.target),
                "duplicate target {} for {id}",
                relation.target,
            );
        }
    }
}

#[test]
fn no_self_links() {
    let entries = [
        e(1, RelationEntryKind::User),
        e(2, RelationEntryKind::Assistant),
    ];
    let graph = build(&entries);
    for id in [1u64, 2] {
        assert!(
            graph.relations(id).iter().all(|r| r.target != id),
            "{id} must not relate to itself",
        );
    }
}

#[test]
fn rebuild_is_skipped_when_fingerprint_unchanged() {
    let entries = [
        e(1, RelationEntryKind::User),
        e(2, RelationEntryKind::Assistant),
    ];
    let mut graph = RelationGraph::new();
    let fp = RelationGraph::fingerprint_of(entries.iter());
    assert!(graph.rebuild_if_stale(fp, &entries));
    assert_eq!(graph.fingerprint(), fp);
    assert!(!graph.rebuild_if_stale(fp, &entries));
    assert!(!graph.rebuild_if_stale(fp, &entries));
}

#[test]
fn empty_transcript_builds_once_then_skips() {
    let mut graph = RelationGraph::new();
    let fp = RelationGraph::fingerprint_of(std::iter::empty());
    assert!(graph.rebuild_if_stale(fp, &[]));
    assert!(!graph.rebuild_if_stale(fp, &[]));
}

#[test]
fn revision_bump_changes_fingerprint() {
    let v0 = [tool(1, "shell", false)];
    let mut bumped = v0.clone();
    bumped[0].revision = 1;
    assert_ne!(
        RelationGraph::fingerprint_of(v0.iter()),
        RelationGraph::fingerprint_of(bumped.iter()),
    );
}

#[test]
fn append_changes_fingerprint() {
    let v0 = [e(1, RelationEntryKind::User)];
    let v1 = [
        e(1, RelationEntryKind::User),
        e(2, RelationEntryKind::Assistant),
    ];
    assert_ne!(
        RelationGraph::fingerprint_of(v0.iter()),
        RelationGraph::fingerprint_of(v1.iter()),
    );
}

#[test]
fn stale_ids_are_dropped_on_rebuild() {
    let v0 = [
        e(1, RelationEntryKind::User),
        e(2, RelationEntryKind::Assistant),
    ];
    // Entry 2 is gone in the next revision; entry 1 is alone.
    let v1 = [e(1, RelationEntryKind::User)];
    let mut graph = RelationGraph::new();
    graph.rebuild_if_stale(RelationGraph::fingerprint_of(v0.iter()), &v0);
    assert!(graph.has_relations(1));
    graph.rebuild_if_stale(RelationGraph::fingerprint_of(v1.iter()), &v1);
    // With the assistant gone, the lone user has nothing to relate to.
    assert!(!graph.has_relations(1));
    assert!(graph.relations(2).is_empty());
}

#[test]
fn large_transcript_derives_in_order() {
    // Perf/scale smoke: a long alternating user/assistant transcript derives and
    // answers lookups deterministically without an N^2 blow-up.
    let mut entries = Vec::with_capacity(4_000);
    for i in 0..2_000u64 {
        entries.push(e(i * 2, RelationEntryKind::User));
        entries.push(e(i * 2 + 1, RelationEntryKind::Assistant));
    }
    let graph = build(&entries);
    // Each user relates to its own assistant reply.
    assert!(targets(&graph, 0).contains(&1));
    assert!(targets(&graph, 3998).contains(&3999));
    // No relation list grows pathologically large.
    assert!(graph.count(0) <= 4, "bounded fan-out: {}", graph.count(0));
}

#[test]
fn confidence_orders_low_below_high() {
    // Guard the Ord derive the ranking relies on.
    assert!(Confidence::High > Confidence::Medium);
    assert!(Confidence::Medium > Confidence::Low);
}
