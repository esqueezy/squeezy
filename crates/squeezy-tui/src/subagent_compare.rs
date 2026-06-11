//! Compare Subagent Outputs (§12.8.3): mark two of the session's subagents and
//! view their findings/outputs side-by-side (or stacked, on a narrow terminal)
//! for comparison, with attribution, independent per-pane scroll, a focus model
//! so the keyboard/wheel target one pane, and a line-based clean-text diff
//! toggle.
//!
//! **Reuses the §12.2.3 Pinned Compare View machinery.** The geometry, scroll,
//! and diff primitives all live in [`crate::pinned_compare`] — the
//! `split_overlay_content` wide-split-vs-narrow-stack layout solver, the
//! `pane_inner` insets, the `clamp_pane_scroll`/`pane_max_scroll` clamp, the
//! half-open `rect_contains` hit-test, the `ComparePane` focus enum, the
//! `CompareMode` content/diff toggle, and the bounded `clean_text_diff`. This
//! module is the spec's "Compare Subagent Outputs" extension of that pinned
//! compare: instead of pinning *transcript entries* it pins two *subagents*
//! (addressed by their stable `SubagentRecord::id`), so the same two-pane focus
//! model and the same clean-text diff machinery compare two delegated workers.
//! Reusing the peer module's primitives means the two compare views behave
//! identically where they overlap and a reader of one understands the other.
//!
//! **Marking is a small two-slot set.** The spec says "mark multiple subagents
//! and compare". The user marks subagents from the Subagent Timeline Panel
//! (§12.8.1) by stable id; the marks live in [`SubagentCompareMarks`], a bounded
//! two-slot set (the spec's "cap visible columns" — two readable panes, never an
//! unbounded fan-out). Marking a *third* subagent rolls the oldest mark off so
//! the newest two are always the compared pair. When exactly two distinct
//! subagents are marked the compare view can open.
//!
//! **Addressed by stable id, healed when a subagent disappears.** Both panes are
//! addressed by `SubagentRecord::id` (never a `Vec` index), so a pruned/cleared
//! record never repoints a pane at the wrong subagent. When a marked id falls out
//! of the live record list the crate root closes the view (heals to `None`)
//! rather than painting an empty pane — exactly as the pinned compare heals a
//! vanished entry id.
//!
//! **Pure state/marking, no `TuiApp`.** Like its peer leaf modules
//! (`pinned_compare`, `subagent_timeline`, `subagent_preview`, `interaction`)
//! this file holds only the small compare-state struct, the two-slot mark set,
//! and pure predicates. The crate root owns the field, the keybinding, the
//! per-frame render call, and the subagent-output text extraction. Keeping the
//! state here lets the marking/focus/scroll math be unit-tested without a
//! terminal.

// Re-export the shared §12.2.3 compare primitives this view drives, so the crate
// root and the tests reach the focus enum / mode toggle / geometry through one
// module and the reuse is explicit.
pub(crate) use crate::pinned_compare::{CompareMode, ComparePane};

/// The bounded set of subagents marked for comparison (the spec's "mark multiple
/// subagents"). Capped at two slots — the spec's "cap visible columns" — so the
/// compared pair is always two readable panes, never an unbounded fan-out.
/// Marks are stable `SubagentRecord::id`s, oldest-first: marking a third subagent
/// rolls the oldest off so the newest two stay the pair.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SubagentCompareMarks {
    /// Marked subagent ids, oldest-first, length ≤ 2.
    ids: Vec<u64>,
}

impl SubagentCompareMarks {
    /// An empty mark set.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Whether `id` is currently marked.
    pub(crate) fn contains(&self, id: u64) -> bool {
        self.ids.contains(&id)
    }

    /// Number of subagents currently marked (0, 1, or 2).
    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }

    /// Toggle `id`'s mark. If it was marked, unmark it; otherwise mark it,
    /// rolling the oldest mark off when a third would exceed the two-slot cap.
    /// Returns `true` when `id` ended up marked, `false` when it ended up
    /// unmarked — the caller uses this for the status line ("marked"/"unmarked").
    pub(crate) fn toggle(&mut self, id: u64) -> bool {
        if let Some(pos) = self.ids.iter().position(|&existing| existing == id) {
            self.ids.remove(pos);
            false
        } else {
            self.ids.push(id);
            if self.ids.len() > 2 {
                self.ids.remove(0);
            }
            true
        }
    }

    /// Drop the mark on `id` if present (used when a subagent record disappears),
    /// returning `true` when a mark was actually removed.
    pub(crate) fn remove(&mut self, id: u64) -> bool {
        if let Some(pos) = self.ids.iter().position(|&existing| existing == id) {
            self.ids.remove(pos);
            true
        } else {
            false
        }
    }

    /// Forget every mark.
    pub(crate) fn clear(&mut self) {
        self.ids.clear();
    }

    /// The marked ids, oldest-first.
    pub(crate) fn ids(&self) -> &[u64] {
        &self.ids
    }

    /// The two marked ids as `(left, right)` when *exactly two distinct* are
    /// marked — the precondition for opening the compare view. The older mark is
    /// `left`, the newer is `right` (so a fresh second mark lands on the right,
    /// the conventional "new" side that the diff treats as additions). `None`
    /// when fewer than two are marked.
    pub(crate) fn pair(&self) -> Option<(u64, u64)> {
        match self.ids.as_slice() {
            [left, right] => Some((*left, *right)),
            _ => None,
        }
    }
}

/// Pinned state for the Compare Subagent Outputs view (§12.8.3). `left_id` /
/// `right_id` are the two compared subagents' stable `SubagentRecord::id`s (never
/// `Vec` indices). `focus` routes the keyboard / wheel to one pane; `mode` is
/// verbatim content vs. a line-based clean-text diff; `left_scroll` /
/// `right_scroll` are the two INDEPENDENT logical row offsets (clamped at render
/// time). [`ComparePane::Pinned`] maps to the left/older subagent and
/// [`ComparePane::Compare`] to the right/newer one, so the shared §12.2.3 focus
/// enum drives this view unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SubagentCompareState {
    /// `SubagentRecord::id` of the left (older-marked) subagent.
    pub(crate) left_id: u64,
    /// `SubagentRecord::id` of the right (newer-marked) subagent.
    pub(crate) right_id: u64,
    /// Which pane the keyboard / wheel drives.
    pub(crate) focus: ComparePane,
    /// Verbatim content vs. line-based clean-text diff.
    pub(crate) mode: CompareMode,
    /// Independent scroll offset for the left pane.
    pub(crate) left_scroll: usize,
    /// Independent scroll offset for the right pane.
    pub(crate) right_scroll: usize,
}

impl SubagentCompareState {
    /// Open the compare view over `(left_id, right_id)`: focus the left pane,
    /// verbatim content mode, both panes scrolled to the top.
    pub(crate) fn new(left_id: u64, right_id: u64) -> Self {
        Self {
            left_id,
            right_id,
            focus: ComparePane::Pinned,
            mode: CompareMode::Content,
            left_scroll: 0,
            right_scroll: 0,
        }
    }

    /// The subagent id behind a pane (left = `Pinned`, right = `Compare`).
    pub(crate) fn id_for(&self, pane: ComparePane) -> u64 {
        match pane {
            ComparePane::Pinned => self.left_id,
            ComparePane::Compare => self.right_id,
        }
    }

    /// The scroll offset of the currently-focused pane.
    pub(crate) fn focused_scroll(&self) -> usize {
        match self.focus {
            ComparePane::Pinned => self.left_scroll,
            ComparePane::Compare => self.right_scroll,
        }
    }

    /// Set the scroll offset of the currently-focused pane.
    pub(crate) fn set_focused_scroll(&mut self, scroll: usize) {
        match self.focus {
            ComparePane::Pinned => self.left_scroll = scroll,
            ComparePane::Compare => self.right_scroll = scroll,
        }
    }

    /// The scroll offset of a specific pane (used by the renderer).
    pub(crate) fn scroll_for(&self, pane: ComparePane) -> usize {
        match pane {
            ComparePane::Pinned => self.left_scroll,
            ComparePane::Compare => self.right_scroll,
        }
    }
}

#[cfg(test)]
#[path = "subagent_compare_tests.rs"]
mod tests;
