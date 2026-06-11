//! Unit tests for the Focus-Preserving Resize (§12.4.3) resolver: capturing the
//! top-visible entry as a stable scroll anchor before a reflow, re-pinning that
//! anchor's `from_bottom` against the post-reflow geometry, preserving
//! tail-following, gracefully clamping when an anchored entry is gone, and
//! re-validating a keyboard-focus index so it never dangles.

use super::*;

// ---------------------------------------------------------------------------
// Capturing the scroll anchor before the reflow.
// ---------------------------------------------------------------------------

#[test]
fn capture_of_following_view_is_following() {
    // A live (tail-following) view captures `Following` regardless of geometry —
    // a reflow keeps it pinned to the tail, never to a frozen anchor.
    let offsets = [0_usize, 4, 9];
    let ids = [10_u64, 20, 30];
    assert_eq!(
        ScrollAnchor::capture(true, 0, &offsets, &ids),
        ScrollAnchor::Following,
    );
}

#[test]
fn capture_anchors_on_the_top_visible_entry_with_intra_entry_offset() {
    // Entries begin at rows 0, 4, and 9. A view whose top row is 5 is looking at
    // the second entry (offset 4 is the greatest header <= 5), one row into it,
    // so it anchors on that id with delta = 5 - 4 = 1.
    let offsets = [0_usize, 4, 9];
    let ids = [10_u64, 20, 30];
    assert_eq!(
        ScrollAnchor::capture(false, 5, &offsets, &ids),
        ScrollAnchor::Anchored { id: 20, delta: 1 },
    );
}

#[test]
fn capture_records_negative_delta_when_scrolled_above_the_first_header() {
    // Scrolled into the blank region above the first header (top row 0, first
    // header at row 2): anchor on the first entry with a NEGATIVE delta, so the
    // re-pin can restore that above-the-header position rather than snapping down.
    let offsets = [2_usize, 6, 11];
    let ids = [10_u64, 20, 30];
    assert_eq!(
        ScrollAnchor::capture(false, 0, &offsets, &ids),
        ScrollAnchor::Anchored { id: 10, delta: -2 },
    );
}

#[test]
fn capture_of_empty_transcript_falls_back_to_following() {
    // No entries to anchor on → `Following`, so the re-pin step degrades to the
    // plain clamp instead of dangling.
    assert_eq!(
        ScrollAnchor::capture(false, 0, &[], &[]),
        ScrollAnchor::Following,
    );
}

// ---------------------------------------------------------------------------
// Re-pinning the anchor after the reflow.
// ---------------------------------------------------------------------------

#[test]
fn reanchor_following_pins_to_tail() {
    // `Following` always resolves to `from_bottom == 0` (the tail).
    let offsets = [0_usize, 4, 9];
    let ids = [10_u64, 20, 30];
    assert_eq!(
        reanchor_from_bottom(ScrollAnchor::Following, &offsets, &ids, 14, 6),
        Some(0),
    );
}

#[test]
fn reanchor_keeps_the_anchored_entry_at_top_after_reflow() {
    // The anchored entry (id 20) sat exactly at its header (delta 0). After the
    // reflow it wraps to MORE rows and now begins at row 7 of a taller content
    // block. The re-pin must compute the `from_bottom` that puts row 7 at the
    // top: total_rows 20, viewport 6 ⇒ max_scroll 14 ⇒ from_bottom 14 - 7 = 7.
    let after_offsets = [0_usize, 7, 15];
    let after_ids = [10_u64, 20, 30];
    assert_eq!(
        reanchor_from_bottom(
            ScrollAnchor::Anchored { id: 20, delta: 0 },
            &after_offsets,
            &after_ids,
            20,
            6
        ),
        Some(7),
        "the same entry id maps to its new top-pinning from_bottom",
    );
}

#[test]
fn reanchor_preserves_intra_entry_offset_across_reflow() {
    // The view sat 2 rows into the anchored entry (delta 2). After the reflow the
    // entry's header moved to row 7, so the same intra-entry position is row 9 ⇒
    // from_bottom 14 - 9 = 5.
    let after_offsets = [0_usize, 7, 15];
    let after_ids = [10_u64, 20, 30];
    assert_eq!(
        reanchor_from_bottom(
            ScrollAnchor::Anchored { id: 20, delta: 2 },
            &after_offsets,
            &after_ids,
            20,
            6
        ),
        Some(5),
        "the re-pin restores the same line within the entry, not just its header",
    );
}

#[test]
fn reanchor_restores_above_header_position_to_absolute_top() {
    // A view scrolled into the blank region above the first header (delta -2 on
    // the first entry whose header is now at row 2) must re-pin to the absolute
    // top (target row max(0, 2-2)=0 ⇒ from_bottom = max_scroll), NOT snap down to
    // the header. This is the over-scrolled clamp-to-top behavior preserved.
    let offsets = [2_usize, 6, 11];
    let ids = [10_u64, 20, 30];
    // total 20, viewport 6 ⇒ max_scroll 14; target_top = clamp(2 + (-2)) = 0.
    assert_eq!(
        reanchor_from_bottom(
            ScrollAnchor::Anchored { id: 10, delta: -2 },
            &offsets,
            &ids,
            20,
            6
        ),
        Some(14),
    );
}

#[test]
fn reanchor_clamps_when_entry_now_sits_past_the_last_screen() {
    // The anchored entry's new offset is below `max_scroll`: clamp to the top
    // (from_bottom 0 — the tail) rather than producing a negative value.
    let offsets = [0_usize, 4, 18];
    let ids = [10_u64, 20, 30];
    // id 30 at offset 18, total 20, viewport 6 ⇒ max_scroll 14; clamp(18)=14
    // ⇒ from_bottom 14 - 14 = 0.
    assert_eq!(
        reanchor_from_bottom(
            ScrollAnchor::Anchored { id: 30, delta: 0 },
            &offsets,
            &ids,
            20,
            6
        ),
        Some(0),
    );
}

#[test]
fn reanchor_returns_none_when_anchored_entry_is_gone() {
    // The anchored entry was pruned/coalesced away; signal the caller to keep its
    // clamp fallback rather than guessing — no hidden/dangling anchor.
    let offsets = [0_usize, 4];
    let ids = [10_u64, 30]; // id 20 vanished
    assert_eq!(
        reanchor_from_bottom(
            ScrollAnchor::Anchored { id: 20, delta: 0 },
            &offsets,
            &ids,
            8,
            6
        ),
        None,
    );
}

#[test]
fn reanchor_capture_then_repin_is_stable_for_unchanged_geometry() {
    // Round-trip: capture against a geometry, then re-pin against the SAME
    // geometry, must reproduce the original top row (a resize that did not change
    // the wrap is a no-op for the anchor).
    let offsets = [0_usize, 4, 9, 13];
    let ids = [10_u64, 20, 30, 40];
    let total_rows = 18;
    let viewport_h = 6;
    let max_scroll = total_rows - viewport_h; // 12
    let original_from_bottom = 8; // looking partway up
    let top_row = max_scroll - original_from_bottom; // 4 ⇒ entry id 20 at its header

    let anchor = ScrollAnchor::capture(false, top_row, &offsets, &ids);
    assert_eq!(anchor, ScrollAnchor::Anchored { id: 20, delta: 0 });
    let repinned = reanchor_from_bottom(anchor, &offsets, &ids, total_rows, viewport_h);
    assert_eq!(
        repinned,
        Some(original_from_bottom),
        "an unchanged reflow reproduces the same scroll position",
    );
}

// ---------------------------------------------------------------------------
// Re-validating the keyboard focus index.
// ---------------------------------------------------------------------------

#[test]
fn resolve_focus_keeps_in_bounds_index() {
    assert_eq!(resolve_focus_index(Some(2), 5), Some(2));
    assert_eq!(resolve_focus_index(Some(0), 1), Some(0));
}

#[test]
fn resolve_focus_unfocused_stays_unfocused() {
    // A resize never conjures a selection out of an unfocused view.
    assert_eq!(resolve_focus_index(None, 5), None);
}

#[test]
fn resolve_focus_clamps_a_dangling_index_to_last_row() {
    // The transcript shrank under the focus index (a prune between a resize-storm
    // burst); clamp to a visible row instead of dangling past the end.
    assert_eq!(resolve_focus_index(Some(9), 4), Some(3));
}

#[test]
fn resolve_focus_clears_on_empty_transcript() {
    // No visible row remains, so focus clears — never a hidden index.
    assert_eq!(resolve_focus_index(Some(0), 0), None);
}
