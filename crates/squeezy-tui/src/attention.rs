//! Attention Routing (§12.8.6): route the user's attention to the
//! subagents/tasks that need it. Quiet progress updates stay on the timeline
//! only; the loud events — failures, cap rejections, blockers, awaited
//! approvals, and selected/pinned completions — surface through a prioritized
//! status indicator + a quick-jump that lands on the single most-important one.
//!
//! **One classifier, one priority order.** Every subagent record is classified
//! into exactly one [`SubagentAttentionKind`] by [`classify`]. The kinds are
//! ordered loudest-first (failure → rejection → blocker → approval →
//! pinned-completion → quiet), and `Quiet` is the catch-all that the indicator
//! and the quick-jump both *skip* — a running subagent or an unpinned completion
//! is calm, exactly the spec's "quiet progress updates timeline only". Nothing
//! here suppresses a log/eval row: this module only decides what gets *surfaced*,
//! never what gets *recorded*, so the spec's "UI suppression must never suppress
//! logs/evals" holds by construction (the caller still feeds every raw event into
//! the timeline + transcript).
//!
//! **Reuses the timeline projection.** The caller already projects each live
//! subagent record into a [`crate::subagent_timeline::SubagentTimelineSource`]
//! for the Subagent Timeline Panel (§12.8.1); Attention Routing consumes that
//! same slice (plus a per-id `pinned` flag for the "selected/pinned completions"
//! opt-in) rather than re-deriving from `lib.rs`, so the two views never drift.
//! The blocker/approval signals are detected from the source's already-bounded,
//! secret-free `latest` activity line — the same string the timeline shows.
//!
//! **Stable ids, never row offsets.** Like the error-lens model (§12.5.6) and the
//! timeline, every attention item is keyed by its subagent `id`, never a
//! width-/filter-dependent row coordinate, so the quick-jump resolves correctly
//! after a reflow or a filter change.
//!
//! **Zero idle cost, incremental rebuild.** The model carries a `fingerprint`
//! folded over every source `(id, status, latest, pinned)`. The caller feeds the
//! same fingerprint each refresh via [`AttentionRoute::rebuild_if_stale`]; when it
//! matches the stored one the call returns immediately and touches nothing. The
//! route is only re-walked when a subagent event actually moved the fingerprint —
//! a lifecycle flip, a fresh activity line, or a pin toggle. An all-calm session
//! (everything running or quietly done) pays one cheap `u64` comparison per
//! refresh and surfaces nothing.

use std::hash::{Hash, Hasher};

use crate::subagent_timeline::{SubagentTimelineSource, SubagentTimelineStatus};

/// Why a subagent wants the user's attention — the routed event class the spec
/// (§12.8.6) calls out. Ordered loudest-first: a smaller discriminant is a more
/// urgent attention target, so the prioritized indicator and the quick-jump both
/// land on the lowest-ordered (most urgent) item. `Quiet` is the calm catch-all
/// (running progress, an unpinned completion) that never surfaces as attention —
/// it stays on the timeline only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum SubagentAttentionKind {
    /// The subagent ended in a failure — the loudest attention class.
    Failure,
    /// The subagent was refused before it ran (the concurrency cap was hit).
    Rejection,
    /// The subagent is blocked / stuck waiting on something (detected from its
    /// latest activity line: "blocked", "stuck", "waiting for", …).
    Blocker,
    /// The subagent is awaiting an approval / a decision (detected from its
    /// latest activity line: "awaiting approval", "needs approval", "permission
    /// required", "confirm", …).
    Approval,
    /// A completed subagent the user pinned/selected — an opt-in notification for
    /// a result they explicitly flagged to watch (the spec's "selected/pinned
    /// completions").
    PinnedCompletion,
    /// Nothing to surface: a running subagent's quiet progress, or an unpinned
    /// completion. Stays on the timeline only; the indicator and quick-jump skip
    /// it.
    Quiet,
}

impl SubagentAttentionKind {
    /// Every kind, loudest-first. Exhaustive on purpose: a new variant must be
    /// added here or it never appears in the summary / priority scan.
    pub(crate) const ALL: &'static [SubagentAttentionKind] = &[
        SubagentAttentionKind::Failure,
        SubagentAttentionKind::Rejection,
        SubagentAttentionKind::Blocker,
        SubagentAttentionKind::Approval,
        SubagentAttentionKind::PinnedCompletion,
        SubagentAttentionKind::Quiet,
    ];

    /// Short, screen-reader-friendly label (ASCII only, no glyphs) for the status
    /// readout.
    pub(crate) fn label(self) -> &'static str {
        match self {
            SubagentAttentionKind::Failure => "failed",
            SubagentAttentionKind::Rejection => "capped",
            SubagentAttentionKind::Blocker => "blocked",
            SubagentAttentionKind::Approval => "approval",
            SubagentAttentionKind::PinnedCompletion => "pinned done",
            SubagentAttentionKind::Quiet => "quiet",
        }
    }

    /// Whether this kind actually wants the user's attention (everything except
    /// [`SubagentAttentionKind::Quiet`]). The single predicate the indicator and
    /// the quick-jump filter on.
    pub(crate) fn is_attention(self) -> bool {
        !matches!(self, SubagentAttentionKind::Quiet)
    }
}

/// Detect a blocker / awaited-approval signal in a subagent's latest activity
/// line. Pure and case-insensitive on the substrings; the order is deliberate —
/// an explicit approval/permission phrase wins over a generic "waiting" so a
/// `waiting for approval` line routes to [`SubagentAttentionKind::Approval`]
/// rather than [`SubagentAttentionKind::Blocker`]. Returns `None` when the line
/// names neither, so the caller falls back to the lifecycle-derived kind.
fn detect_wait_signal(latest: &str) -> Option<SubagentAttentionKind> {
    let lower = latest.to_ascii_lowercase();

    // Approval / decision gates — the more specific signal, checked first.
    if lower.contains("awaiting approval")
        || lower.contains("needs approval")
        || lower.contains("waiting for approval")
        || lower.contains("approval required")
        || lower.contains("permission required")
        || lower.contains("permission denied")
        || lower.contains("awaiting input")
        || lower.contains("awaiting confirmation")
        || lower.contains("confirm to continue")
        || lower.contains("needs your input")
        || lower.contains("needs input")
    {
        return Some(SubagentAttentionKind::Approval);
    }

    // Generic blocked / stuck signals.
    if lower.contains("blocked")
        || lower.contains("is stuck")
        || lower.contains("stalled")
        || lower.contains("waiting for")
        || lower.contains("waiting on")
        || lower.contains("cannot proceed")
        || lower.contains("can't proceed")
    {
        return Some(SubagentAttentionKind::Blocker);
    }

    None
}

/// Classify one subagent source (plus its `pinned` flag) into the single
/// [`SubagentAttentionKind`] that should route the user's attention. Pure and
/// standalone so it is the unit-testable heart of the feature.
///
/// A terminal failure or a cap rejection is loud by its lifecycle alone. A
/// *running* subagent is otherwise quiet — except when its latest line reports a
/// blocker or an awaited approval, which is exactly the case the spec wants
/// surfaced even mid-run. A *completed* subagent is quiet unless the user pinned
/// it (an opt-in "watch this result" notification). Everything else is
/// [`SubagentAttentionKind::Quiet`].
pub(crate) fn classify(source: &SubagentTimelineSource, pinned: bool) -> SubagentAttentionKind {
    match source.status {
        // A hard failure is the loudest class regardless of its latest line.
        SubagentTimelineStatus::Failed => SubagentAttentionKind::Failure,
        // A cap rejection is always surfaced (it never ran).
        SubagentTimelineStatus::Rejected => SubagentAttentionKind::Rejection,
        // A running subagent is quiet unless its activity line reports a wait
        // (blocked / awaiting approval) the user needs to clear.
        SubagentTimelineStatus::Running => {
            detect_wait_signal(&source.latest).unwrap_or(SubagentAttentionKind::Quiet)
        }
        // A completion only surfaces when the user pinned it (opt-in).
        SubagentTimelineStatus::Completed => {
            if pinned {
                SubagentAttentionKind::PinnedCompletion
            } else {
                SubagentAttentionKind::Quiet
            }
        }
    }
}

/// One routed attention item (§12.8.6). `id` is the subagent's stable id (the
/// quick-jump target); `ordinal` is its 1-based source position (kept so the
/// caller can resolve a status hint without a second lookup); `agent` is its
/// role/name label; `kind` is the classified attention class (never
/// [`SubagentAttentionKind::Quiet`] — quiet items are not stored). The list is
/// ordered loudest-first so element 0 is always the single most-important target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttentionItem {
    pub(crate) id: u64,
    pub(crate) ordinal: u32,
    pub(crate) agent: String,
    pub(crate) kind: SubagentAttentionKind,
}

/// One subagent record as the caller projects it for attention routing: the
/// timeline source it already builds, plus whether the user pinned this subagent
/// (the "selected/pinned completions" opt-in — the compare-mark set in `lib.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttentionSource<'a> {
    pub(crate) source: &'a SubagentTimelineSource,
    pub(crate) pinned: bool,
}

/// The computed Attention Routing model over the session's subagent records
/// (§12.8.6).
///
/// `items` is the prioritized list of attention items (loudest-first; quiet
/// records are dropped). `fingerprint` is the staleness tag described in the
/// module docs; `built` distinguishes "empty set scanned" from "never built" so a
/// genuinely calm session is not re-scanned every refresh.
#[derive(Debug, Clone, Default)]
pub(crate) struct AttentionRoute {
    items: Vec<AttentionItem>,
    fingerprint: u64,
    built: bool,
}

impl AttentionRoute {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Fold a staleness fingerprint over the sources. Order- and
    /// content-sensitive: id, status, the pinned flag, and the latest line all
    /// participate, so a new subagent, a status flip, a pin toggle, or a fresh
    /// activity line (which can move a running subagent into/out of a blocker/
    /// approval signal) all move the value. Pure and standalone so the caller can
    /// compute it cheaply each refresh and compare before deciding to recompute.
    pub(crate) fn fingerprint_of<'a>(
        sources: impl IntoIterator<Item = &'a AttentionSource<'a>>,
    ) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for source in sources {
            source.source.id.hash(&mut hasher);
            source.source.status.hash(&mut hasher);
            source.source.latest.hash(&mut hasher);
            source.pinned.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Recompute the routed list from `sources` **only if** `fingerprint` differs
    /// from the one captured at the last rebuild (or this is the first build).
    /// Returns `true` when a recompute actually ran, `false` when the cached model
    /// was already current (the zero-idle-cost fast path).
    ///
    /// Each source is classified; quiet sources are dropped; the survivors are
    /// sorted loudest-first (by kind, then by source ordinal so two items of the
    /// same kind keep a stable, record-order tiebreak). The caller computes
    /// `fingerprint` via [`Self::fingerprint_of`] over the same slice.
    pub(crate) fn rebuild_if_stale(
        &mut self,
        fingerprint: u64,
        sources: &[AttentionSource<'_>],
    ) -> bool {
        if self.built && fingerprint == self.fingerprint {
            return false;
        }
        self.items.clear();
        for (index, src) in sources.iter().enumerate() {
            let kind = classify(src.source, src.pinned);
            if !kind.is_attention() {
                continue;
            }
            self.items.push(AttentionItem {
                id: src.source.id,
                ordinal: index as u32 + 1,
                agent: src.source.agent.clone(),
                kind,
            });
        }
        // Loudest-first, then record order within a kind (a stable tiebreak so the
        // quick-jump target is deterministic across refreshes).
        self.items
            .sort_by(|a, b| a.kind.cmp(&b.kind).then(a.ordinal.cmp(&b.ordinal)));
        self.fingerprint = fingerprint;
        self.built = true;
        true
    }

    /// The stored staleness fingerprint from the last rebuild. Test/diagnostic
    /// accessor; production compares inside `rebuild_if_stale`.
    #[cfg(test)]
    pub(crate) fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// The routed attention items, loudest-first. Read by the unit suite to
    /// assert priority order; production reads [`Self::top`] (the single jump
    /// target) and [`Self::indicator`] (the status readout).
    #[cfg(test)]
    pub(crate) fn items(&self) -> &[AttentionItem] {
        &self.items
    }

    /// Number of routed attention items.
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether anything wants attention. Read by the unit suite; production reads
    /// [`Self::top`] (which is `None` exactly when this is empty).
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The single most-important attention target (loudest, then record order),
    /// or `None` when nothing wants attention. The quick-jump lands here.
    pub(crate) fn top(&self) -> Option<&AttentionItem> {
        self.items.first()
    }

    /// Number of items of `kind`.
    pub(crate) fn count_of(&self, kind: SubagentAttentionKind) -> usize {
        self.items.iter().filter(|i| i.kind == kind).count()
    }

    /// The list index of the next item strictly after `after` (wrapping to the
    /// first when `after` is the last or `None`). `None` only when there are no
    /// items. The quick-jump lands on [`Self::top`] today; this drives forward
    /// navigation across multiple attention targets and is exercised by the unit
    /// suite until a "next attention" verb lands.
    #[cfg(test)]
    pub(crate) fn next_index(&self, after: Option<usize>) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        match after {
            Some(i) if i + 1 < self.items.len() => Some(i + 1),
            Some(_) => Some(0),
            None => Some(0),
        }
    }

    /// A compact indicator string for the status line, e.g.
    /// `"!2 attention \u{00b7} 1 failed \u{00b7} 1 approval"`. The leading `!N`
    /// is the at-a-glance badge; the per-kind breakdown follows. Empty string when
    /// nothing wants attention (so the status line paints nothing extra at idle).
    pub(crate) fn indicator(&self) -> String {
        if self.items.is_empty() {
            return String::new();
        }
        let total = self.items.len();
        let mut parts = vec![format!("!{total} attention")];
        for kind in SubagentAttentionKind::ALL.iter().copied() {
            if !kind.is_attention() {
                continue;
            }
            let n = self.count_of(kind);
            if n > 0 {
                parts.push(format!("{n} {}", kind.label()));
            }
        }
        parts.join(" \u{00b7} ")
    }
}

#[cfg(test)]
#[path = "attention_tests.rs"]
mod tests;
