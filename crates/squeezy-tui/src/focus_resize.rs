//! Focus-Preserving Resize (§12.4.3): the user's scroll anchor and keyboard
//! focus survive a terminal resize / relayout instead of jumping.
//!
//! The transcript scrolls by a bare `from_bottom` line count (see
//! [`crate::scroll::ScrollState`]). That count is meaningful only against a
//! fixed wrap geometry: when a resize reflows the transcript to a new width,
//! every entry occupies a *different* number of wrapped rows, so the *same*
//! `from_bottom` now lands on a *different entry*. The view appears to jump — the
//! user was reading entry N at the top and, after the drag, is suddenly looking
//! at entry N±k. The existing resize hook only clamps `from_bottom` into the new
//! `[0, max_scroll]`; it does not keep the anchored *content* in place.
//!
//! **Anchor as an entry id, not a row.** This module is the single place that
//! re-pins the scroll position to a *logical* anchor across a reflow. Before the
//! relayout the caller snapshots the entry currently at the top of the viewport
//! as a stable [`crate::TranscriptEntry::id`] (the spec's "scroll anchor as
//! row/entry id"); after the reflow [`reanchor_from_bottom`] recomputes the
//! `from_bottom` that puts that same entry back at the top. Following the tail is
//! preserved verbatim (a live view re-pins to the tail, never to a stale
//! anchor), and an anchor whose entry was pruned/coalesced falls back to clamping
//! rather than dangling — the spec's "no hidden focus without fallback", in one
//! resolver.
//!
//! **Zero idle cost.** Everything here is pure, allocation-free index/row
//! arithmetic with no clock, no I/O, and no state to invalidate. The caller
//! invokes it only on the `Event::Resize` reflow (and the suspend/resume
//! re-anchor that mirrors it) — an idle session that paints nothing calls it
//! zero times, so the feature adds no idle redraw and no background work.

/// The logical scroll anchor captured before a reflow: the stable id of the
/// entry at (or nearest above) the top of the viewport.
///
/// `Following` records that the view was pinned to the tail, which a reflow must
/// keep pinned regardless of any entry-row churn (a live view tracks new output,
/// not a frozen anchor). `Anchored(id)` is the top-visible entry's stable id,
/// which outlives the wrap-width change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScrollAnchor {
    /// The view was following the tail; keep it pinned to the bottom.
    Following,
    /// The view was scrolled up, anchored on the entry with this stable id, plus
    /// the signed `delta` = `top_row - entry_offset` (how far the viewport top
    /// sat *into* the entry, negative when scrolled into the blank region above
    /// its header). Carrying the offset, not just the id, lets the re-pin land
    /// on the same line *within* the entry after a reflow (the spec's "scroll
    /// anchor as row/entry id **plus offset**") — and keeps an over-scrolled
    /// "above the first header" view pinned to the absolute top, not snapped down
    /// to the header.
    Anchored { id: u64, delta: isize },
}

impl ScrollAnchor {
    /// Capture the anchor from the pre-reflow geometry.
    ///
    /// * `following` — whether the scroll state currently tracks the tail.
    /// * `top_row` — the wrapped-row index currently at the top of the viewport.
    /// * `entry_offsets` — each entry's first wrapped-row offset, ascending
    ///   (`entry_offsets[i]` is where entry `i`'s header begins).
    ///
    /// A following view captures [`ScrollAnchor::Following`]. Otherwise the
    /// anchor is the entry whose header is the greatest offset still at or above
    /// `top_row` — the entry occupying the top row. When scrolled above every
    /// header (or the transcript is empty) it anchors on the first entry, or
    /// falls back to [`ScrollAnchor::Following`] when there are no entries at all.
    #[must_use]
    pub(crate) fn capture(
        following: bool,
        top_row: usize,
        entry_offsets: &[usize],
        entry_ids: &[u64],
    ) -> Self {
        if following {
            return Self::Following;
        }
        let index = entry_offsets
            .iter()
            .enumerate()
            .filter(|&(_, &off)| off <= top_row)
            .map(|(i, _)| i)
            .next_back()
            .unwrap_or(0);
        match (entry_ids.get(index), entry_offsets.get(index)) {
            (Some(&id), Some(&offset)) => {
                let delta = top_row as isize - offset as isize;
                Self::Anchored { id, delta }
            }
            _ => Self::Following,
        }
    }
}

/// The `from_bottom` (lines scrolled up from the tail) that re-pins the captured
/// [`ScrollAnchor`] to the top of the viewport against the *post-reflow*
/// geometry.
///
/// * `Following` → `0` (the tail).
/// * `Anchored { id, .. }` with `id` present in `entry_ids` → the `from_bottom`
///   that restores that entry's captured line to the top, clamped to
///   `[0, max_scroll]`.
/// * `Anchored { id, .. }` with `id` absent (pruned/coalesced) → `None`,
///   signalling the caller to keep its existing clamp-based fallback (no
///   hidden/dangling anchor).
///
/// `total_rows` / `viewport_h` are the post-reflow wrapped-row total and visible
/// height; `entry_offsets[i]` is entry `i`'s post-reflow header row.
#[must_use]
pub(crate) fn reanchor_from_bottom(
    anchor: ScrollAnchor,
    entry_offsets: &[usize],
    entry_ids: &[u64],
    total_rows: usize,
    viewport_h: usize,
) -> Option<usize> {
    let (id, delta) = match anchor {
        ScrollAnchor::Following => return Some(0),
        ScrollAnchor::Anchored { id, delta } => (id, delta),
    };
    let index = entry_ids.iter().position(|candidate| *candidate == id)?;
    let offset = *entry_offsets.get(index)?;
    let max_scroll = total_rows.saturating_sub(viewport_h);
    // Restore the same line *within* the entry: target top row = the entry's new
    // header offset plus the captured intra-entry delta, clamped into the valid
    // `[0, max_scroll]` scroll range. Then `from_bottom = max_scroll - top_row`,
    // mirroring the jump-navigation conversion, so the entry sits where it did.
    let target_top = (offset as isize + delta).clamp(0, max_scroll as isize) as usize;
    Some(max_scroll - target_top)
}

/// Re-resolve a keyboard-focused entry's row index to a still-valid, visible row
/// after a relayout (§12.4.3) — the spec's `resolve_focus_after_layout` for the
/// focus highlight, kept distinct from the scroll anchor above.
///
/// * `None` (unfocused) stays `None`: a resize never conjures a selection.
/// * An in-bounds index is returned unchanged: the entry list does not change at
///   resize, so a valid index still names the same entry.
/// * An out-of-bounds index (the transcript shrank under it, e.g. a prune
///   between a resize-storm burst) clamps to the last row, or clears to `None`
///   on an empty transcript — never a dangling/hidden focus.
#[must_use]
pub(crate) fn resolve_focus_index(selected: Option<usize>, entry_count: usize) -> Option<usize> {
    let index = selected?;
    if entry_count == 0 {
        return None;
    }
    Some(index.min(entry_count - 1))
}

#[cfg(test)]
#[path = "focus_resize_tests.rs"]
mod tests;
