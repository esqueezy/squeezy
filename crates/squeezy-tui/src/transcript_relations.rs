//! Related-Entry Links (§12.5.3): a pure, in-memory **relation graph** over the
//! transcript model, keyed by **stable entry ids** (`TranscriptEntry::id`), that
//! surfaces the implicit links between entries — `user prompt -> assistant ->
//! tool call -> result -> error -> fix -> follow-up`, plus subagents — so the
//! user can jump between an entry and the things it relates to.
//!
//! The graph is derived from a small, ordered slice of classified entries
//! ([`RelationEntry`]) that the caller (`lib.rs`) builds from the live
//! transcript, reusing the same role / `LogKind` / `entry_is_error` predicates
//! the renderer and the Local Transcript Index (§12.5.1) use. This module owns
//! only the id/relation bookkeeping — nothing about geometry, rendering, or
//! input — so the derivation math stays testable without a terminal.
//!
//! **Typed relations with confidence + provenance.** Every edge is a
//! [`Relation`] carrying its [`RelationKind`] (what kind of link it is), a
//! [`Confidence`] (how sure we are), and a one-line provenance string (why it
//! was derived, surfaced in the overlay so a weak link explains itself). Direct
//! sequential links (a user prompt and the assistant reply that follows it) rank
//! high; derived links (two tool results that merely share a tool name) rank
//! low. The overlay lists a focused entry's related entries ranked by confidence
//! then transcript order, so the strongest link reads first.
//!
//! **Stable ids, never row offsets.** Like the index and the jump-mark stack
//! (`jump_marks.rs`), every key here is an entry id, never a width-/fold-
//! dependent row coordinate. An id survives reflow (resize, streaming, collapse,
//! coalescing), so a relation built before a reflow still resolves to the right
//! entry afterwards. Ids whose entry has since been dropped fall out on the next
//! rebuild.
//!
//! **Zero idle cost, incremental rebuild.** Like the index, the graph carries a
//! `fingerprint` folded over every live `(id, revision, kind, error, tool)`. The
//! caller feeds the same fingerprint each frame via
//! [`RelationGraph::rebuild_if_stale`]; when it matches the stored one the call
//! returns immediately and touches nothing. The graph is only re-walked when the
//! transcript actually changed (append, stream settle, revision bump, clear,
//! compaction, resume). An idle session pays one cheap `u64` comparison.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// The coarse role an entry plays for relation derivation. A small fixed set —
/// enough to recognize the turn flow `user -> assistant -> tool -> error` and
/// the subagent lane. Mirrors the categories the Local Transcript Index uses,
/// collapsed to what the relation rules actually branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RelationEntryKind {
    /// A user message — the start of a turn.
    User,
    /// An assistant message.
    Assistant,
    /// A tool-call result (any status).
    ToolCall,
    /// A finalized reasoning segment.
    Reasoning,
    /// A failure surface: an error/failure log or a message whose outcome failed.
    Error,
    /// A subagent lifecycle breadcrumb.
    Subagent,
    /// Anything else (notes, plan cards, diffs, echoes).
    Other,
}

/// One classified transcript entry, as the caller feeds it in. `id` is the
/// stable `TranscriptEntry::id`; `revision` is its content revision (folded into
/// the fingerprint so a mutation re-derives); `kind` is its role; `is_error`
/// flags a failed entry (a failed tool keeps `kind == ToolCall` but sets this);
/// `tool_name` is the tool name for a tool-call entry, `None` otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationEntry {
    pub(crate) id: u64,
    pub(crate) revision: u64,
    pub(crate) kind: RelationEntryKind,
    pub(crate) is_error: bool,
    pub(crate) tool_name: Option<String>,
}

/// What kind of link a [`Relation`] represents. Ordered loosely by the natural
/// turn flow so a debug dump reads top-to-bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RelationKind {
    /// A user prompt and the assistant reply that answered it.
    Response,
    /// An assistant turn and a tool call it triggered.
    ToolInvocation,
    /// A tool call (or assistant turn) and the error it produced.
    Caused,
    /// An error and the cause that produced it (the inverse of [`Self::Caused`]).
    CausedBy,
    /// An error and the follow-up user turn that addressed it.
    Followup,
    /// Two tool results that ran the same tool.
    SameTool,
    /// Adjacent subagent breadcrumbs in the same subagent lane.
    Subagent,
}

impl RelationKind {
    /// Short, screen-reader-friendly verb for the overlay row. ASCII only (no
    /// glyphs) to match the rest of Squeezy's chrome.
    pub(crate) fn label(self) -> &'static str {
        match self {
            RelationKind::Response => "reply to",
            RelationKind::ToolInvocation => "tool call from",
            RelationKind::Caused => "caused error",
            RelationKind::CausedBy => "caused by",
            RelationKind::Followup => "follow-up to",
            RelationKind::SameTool => "same tool",
            RelationKind::Subagent => "subagent",
        }
    }
}

/// How confident the derivation is in a relation. Direct sequential links rank
/// `High`; structural-but-inferred links `Medium`; loose heuristics `Low`. The
/// overlay ranks a focused entry's relations by this (then transcript order), so
/// the strongest link reads first and weak links are not lost but demoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    /// Short tag for the overlay's provenance column (debug-mode honesty: a weak
    /// link says so).
    pub(crate) fn label(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "med",
            Confidence::Low => "low",
        }
    }
}

/// One directed edge from a source entry to a related `target` entry, with its
/// kind, confidence, and a one-line provenance explaining why it was derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Relation {
    /// The related entry id this edge points at.
    pub(crate) target: u64,
    /// What kind of link this is.
    pub(crate) kind: RelationKind,
    /// How sure the derivation is.
    pub(crate) confidence: Confidence,
    /// Why this link was derived (surfaced in the overlay, debug-honest).
    pub(crate) provenance: &'static str,
}

/// In-memory relation graph keyed by stable entry id.
///
/// `edges` maps each entry id to its ranked list of related entries (strongest
/// first). `order` records each id's transcript position so the ranking can
/// tie-break deterministically. `fingerprint` is the staleness tag described in
/// the module docs.
#[derive(Debug, Clone, Default)]
pub(crate) struct RelationGraph {
    edges: HashMap<u64, Vec<Relation>>,
    order: HashMap<u64, usize>,
    fingerprint: u64,
    built: bool,
}

impl RelationGraph {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Fold a fingerprint over a sequence of classified entries. Order- and
    /// content-sensitive: id, revision, kind, error, and tool name all
    /// participate, so an append, a revision bump, a reorder, a kind change, or
    /// a drop all move the value. Pure and standalone so the caller can compute
    /// it cheaply each frame and compare against the stored one before deciding
    /// to rebuild.
    pub(crate) fn fingerprint_of<'a>(entries: impl IntoIterator<Item = &'a RelationEntry>) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for entry in entries {
            entry.id.hash(&mut hasher);
            entry.revision.hash(&mut hasher);
            entry.kind.hash(&mut hasher);
            entry.is_error.hash(&mut hasher);
            entry.tool_name.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Rebuild the graph from `entries` **only if** `fingerprint` differs from
    /// the one captured at the last rebuild (or this is the first build).
    /// Returns `true` when a rebuild actually ran, `false` when the cached graph
    /// was already current (the zero-idle-cost fast path).
    pub(crate) fn rebuild_if_stale(&mut self, fingerprint: u64, entries: &[RelationEntry]) -> bool {
        if self.built && fingerprint == self.fingerprint {
            return false;
        }
        self.rebuild(entries);
        self.fingerprint = fingerprint;
        self.built = true;
        true
    }

    /// Derive every relation from the ordered entry slice. Symmetric edges are
    /// added in both directions (so jumping is reversible), then each adjacency
    /// list is sorted strongest-first and de-duplicated.
    fn rebuild(&mut self, entries: &[RelationEntry]) {
        self.edges.clear();
        self.order.clear();
        for (i, entry) in entries.iter().enumerate() {
            self.order.insert(entry.id, i);
        }

        for (i, entry) in entries.iter().enumerate() {
            match entry.kind {
                RelationEntryKind::User => {
                    // user prompt -> the next assistant reply (Response, high).
                    if let Some(reply) = next_matching(entries, i, |e| {
                        matches!(e.kind, RelationEntryKind::Assistant)
                    }) {
                        self.link(
                            entry.id,
                            reply.id,
                            RelationKind::Response,
                            Confidence::High,
                            "assistant reply that followed this prompt",
                        );
                    }
                }
                RelationEntryKind::Assistant => {
                    // assistant -> each tool call it triggered, until the next
                    // user/assistant boundary (ToolInvocation, high).
                    for tool in following_tools(entries, i) {
                        self.link(
                            entry.id,
                            tool.id,
                            RelationKind::ToolInvocation,
                            Confidence::High,
                            "tool call invoked in this assistant turn",
                        );
                    }
                }
                _ => {}
            }

            if entry.is_error {
                // error -> the preceding tool call / assistant that caused it
                // (CausedBy, medium).
                if let Some(cause) = prev_matching(entries, i, |e| {
                    matches!(
                        e.kind,
                        RelationEntryKind::ToolCall | RelationEntryKind::Assistant
                    )
                }) {
                    self.link(
                        entry.id,
                        cause.id,
                        RelationKind::CausedBy,
                        Confidence::Medium,
                        "tool call / turn that preceded this failure",
                    );
                }
                // error -> the next user turn that addressed it (Followup, low).
                if let Some(fix) =
                    next_matching(entries, i, |e| matches!(e.kind, RelationEntryKind::User))
                {
                    self.link(
                        entry.id,
                        fix.id,
                        RelationKind::Followup,
                        Confidence::Low,
                        "user turn that followed this failure",
                    );
                }
            }

            // Same-tool links: a tool result relates to every other call of the
            // same tool (SameTool, low). Only the forward neighbour is linked so
            // a tool used N times produces a chain, not an N^2 fan-out; both
            // directions come from the symmetric `link`.
            if entry.kind == RelationEntryKind::ToolCall
                && let Some(name) = &entry.tool_name
                && let Some(same) = next_matching(entries, i, |e| {
                    e.kind == RelationEntryKind::ToolCall
                        && e.tool_name.as_deref() == Some(name.as_str())
                })
            {
                self.link(
                    entry.id,
                    same.id,
                    RelationKind::SameTool,
                    Confidence::Low,
                    "next call of the same tool",
                );
            }

            // Subagent lane: adjacent subagent breadcrumbs chain together
            // (Subagent, medium).
            if entry.kind == RelationEntryKind::Subagent
                && let Some(next) =
                    next_matching(entries, i, |e| e.kind == RelationEntryKind::Subagent)
            {
                self.link(
                    entry.id,
                    next.id,
                    RelationKind::Subagent,
                    Confidence::Medium,
                    "adjacent subagent breadcrumb",
                );
            }
        }

        self.rank();
    }

    /// Record a relation in both directions so a jump is reversible. The reverse
    /// of [`RelationKind::Caused`] is [`RelationKind::CausedBy`] and vice versa;
    /// every other kind is its own inverse.
    fn link(
        &mut self,
        from: u64,
        to: u64,
        kind: RelationKind,
        confidence: Confidence,
        provenance: &'static str,
    ) {
        if from == to {
            return;
        }
        self.edges.entry(from).or_default().push(Relation {
            target: to,
            kind,
            confidence,
            provenance,
        });
        let reverse_kind = match kind {
            RelationKind::Caused => RelationKind::CausedBy,
            RelationKind::CausedBy => RelationKind::Caused,
            other => other,
        };
        self.edges.entry(to).or_default().push(Relation {
            target: from,
            kind: reverse_kind,
            confidence,
            provenance,
        });
    }

    /// Sort each adjacency list strongest-first (confidence desc, then transcript
    /// order asc) and drop duplicate targets, keeping the strongest edge per
    /// target. Deterministic so the overlay order is stable across rebuilds.
    fn rank(&mut self) {
        let order = &self.order;
        for relations in self.edges.values_mut() {
            relations.sort_by(|a, b| {
                b.confidence
                    .cmp(&a.confidence)
                    .then_with(|| {
                        order
                            .get(&a.target)
                            .copied()
                            .unwrap_or(usize::MAX)
                            .cmp(&order.get(&b.target).copied().unwrap_or(usize::MAX))
                    })
                    .then_with(|| a.target.cmp(&b.target))
            });
            // Keep the first (strongest) edge per target.
            let mut seen = std::collections::HashSet::new();
            relations.retain(|relation| seen.insert(relation.target));
        }
    }

    /// The stored fingerprint from the last rebuild. Test/diagnostic accessor.
    #[cfg(test)]
    pub(crate) fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// The ranked relations for entry `id` (strongest first), or an empty slice
    /// when the id has no relations (or is not present).
    pub(crate) fn relations(&self, id: u64) -> &[Relation] {
        self.edges.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Whether entry `id` has at least one related entry. The overlay reads the
    /// `count`/`relations` accessors directly today, so this convenience is
    /// exercised by the unit suite until a "does this entry have links?" badge
    /// affordance consumes it.
    #[cfg(test)]
    pub(crate) fn has_relations(&self, id: u64) -> bool {
        self.edges
            .get(&id)
            .is_some_and(|relations| !relations.is_empty())
    }

    /// The number of related entries for `id`.
    pub(crate) fn count(&self, id: u64) -> usize {
        self.edges.get(&id).map_or(0, Vec::len)
    }

    /// The target id of the related entry at `index` in `id`'s ranked list, or
    /// `None` when out of range. The overlay walks this with a cursor.
    pub(crate) fn target_at(&self, id: u64, index: usize) -> Option<u64> {
        self.edges
            .get(&id)
            .and_then(|relations| relations.get(index).map(|relation| relation.target))
    }
}

/// The first entry strictly after `from` matching `pred`, or `None`.
fn next_matching(
    entries: &[RelationEntry],
    from: usize,
    pred: impl Fn(&RelationEntry) -> bool,
) -> Option<&RelationEntry> {
    entries.get(from + 1..)?.iter().find(|e| pred(e))
}

/// The first entry strictly before `from` matching `pred`, or `None`.
fn prev_matching(
    entries: &[RelationEntry],
    from: usize,
    pred: impl Fn(&RelationEntry) -> bool,
) -> Option<&RelationEntry> {
    entries.get(..from)?.iter().rev().find(|e| pred(e))
}

/// Every tool-call entry that follows the assistant at `from`, up to (but not
/// including) the next user or assistant boundary — i.e. the tools that turn
/// triggered.
fn following_tools(entries: &[RelationEntry], from: usize) -> Vec<&RelationEntry> {
    let mut tools = Vec::new();
    for entry in entries.get(from + 1..).into_iter().flatten() {
        match entry.kind {
            RelationEntryKind::User | RelationEntryKind::Assistant => break,
            RelationEntryKind::ToolCall => tools.push(entry),
            _ => {}
        }
    }
    tools
}

#[cfg(test)]
#[path = "transcript_relations_tests.rs"]
mod tests;
