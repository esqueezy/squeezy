//! Unified logical scroll model for the transcript (§11G.11, plan §11.9).
//!
//! Before this module the two scrollable transcript surfaces each carried their
//! own scroll arithmetic:
//!
//! * the MAIN view stored a `from_bottom` distance in [`crate::scroll::ScrollState`]
//!   and resolved a top-line offset + scrollbar thumb in `scroll.rs`;
//! * the Ctrl+T OVERLAY stored an absolute top-line `scroll: usize` (with a
//!   `usize::MAX` "snap to bottom" sentinel) and resolved its own top-line
//!   offset + scrollbar thumb with a *separate, duplicated* copy of the same
//!   math in `lib.rs`.
//!
//! Two coordinate systems, two copies of the geometry — exactly the tech debt
//! §11.9 calls out. This module is the single, `usize`-backed logical scroll
//! model both surfaces now share. The two render-boundary conversions live here
//! once:
//!
//! * [`resolve_top_offset`] — turn a [`LogicalScroll`] into the absolute
//!   top-line offset the renderer scrolls the content by, clamped to the live
//!   `(content_len, viewport_h)` geometry. This is the *only* place a logical
//!   position becomes a concrete top row.
//! * [`thumb_geometry`] — the proportional scrollbar thumb (offset + length)
//!   for a resolved top-line offset. Both surfaces draw the identical thumb;
//!   they previously each re-derived it.
//!
//! Everything is `usize`. Narrowing to the `u16` that ratatui geometry consumes
//! stays funnelled through the one [`crate::scroll::to_u16_clamped`] helper at
//! the call sites, so this module never truncates.
//!
//! [`LogicalScroll`] is the canonical logical position: a top-line offset plus a
//! `follow_tail` intent flag. "Pinned to the tail" is expressed by the flag, not
//! by a sentinel value, so the two surfaces' historical sentinels
//! (`from_bottom == 0` for the main view, `usize::MAX` for the overlay) both map
//! onto the same well-typed state.

/// A `usize`-backed logical scroll position shared by the main transcript and
/// the Ctrl+T overlay.
///
/// `top` is the logical top-line offset *intent* — how far from the first row
/// the viewport's top edge sits. It is only clamped to real geometry at a
/// render boundary by [`resolve_top_offset`], so a stored value can outlive a
/// transient too-small viewport without losing the user's place.
///
/// `follow_tail` records the "stay pinned to the bottom as content grows"
/// intent. While set, [`resolve_top_offset`] ignores `top` and returns the live
/// `max_scroll`, matching how both legacy surfaces snapped to the tail
/// (`from_bottom == 0` and the `usize::MAX` sentinel respectively).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LogicalScroll {
    /// Logical top-line offset from the first content row.
    top: usize,
    /// Whether the view tracks the tail as content grows.
    follow_tail: bool,
}

impl Default for LogicalScroll {
    /// A fresh view follows the tail.
    fn default() -> Self {
        Self::pinned()
    }
}

impl LogicalScroll {
    /// A position pinned to the bottom (following the tail).
    #[must_use]
    pub(crate) fn pinned() -> Self {
        Self {
            top: 0,
            follow_tail: true,
        }
    }

    /// A position anchored at an absolute top-line offset, unpinned.
    ///
    /// Used by callers that already hold a concrete top row (the overlay, whose
    /// stored model *is* a top-line offset). A `top` of `0` does not auto-pin
    /// here — pinning is an explicit intent the caller sets via [`Self::pinned`]
    /// — so the overlay's "scrolled to row 0 (top)" state stays distinct from
    /// its "follow the tail" state.
    #[must_use]
    pub(crate) fn at_top_offset(top: usize) -> Self {
        Self {
            top,
            follow_tail: false,
        }
    }

    /// Whether the view is currently following the tail.
    ///
    /// Production resolves a concrete offset through [`resolve_top_offset`]
    /// rather than branching on this flag directly, so it is exercised by the
    /// unit tests; mirrors `scroll::ScrollState::is_following`.
    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn is_following(&self) -> bool {
        self.follow_tail
    }

    /// The raw stored top-line offset (unclamped). `0` while following the tail.
    ///
    /// The render path reads the geometry-clamped [`resolve_top_offset`]; this
    /// raw accessor lets the unit tests assert on the stored intent.
    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn raw_top(&self) -> usize {
        self.top
    }
}

/// The largest top-line offset that still shows content: `content_len -
/// viewport_h`, saturating at `0` when the content fits.
///
/// This is the shared definition of "max scroll" both surfaces used (the main
/// view's `ScrollState::max_scroll` and the overlay's
/// `transcript_overlay_max_scroll_for_content`).
#[must_use]
pub(crate) fn max_top_offset(content_len: usize, viewport_h: usize) -> usize {
    content_len.saturating_sub(viewport_h)
}

/// Resolve a [`LogicalScroll`] into the absolute top-line offset to render at,
/// clamped to `[0, max_top_offset]`.
///
/// This is the single render-boundary conversion from the logical model to a
/// concrete top row. Following the tail resolves to `max_scroll` (the bottom);
/// otherwise the stored `top` is clamped down to `max_scroll` so a shrunken
/// viewport or trimmed content never scrolls past the last row.
#[must_use]
pub(crate) fn resolve_top_offset(
    scroll: LogicalScroll,
    content_len: usize,
    viewport_h: usize,
) -> usize {
    let max_scroll = max_top_offset(content_len, viewport_h);
    if scroll.follow_tail {
        max_scroll
    } else {
        scroll.top.min(max_scroll)
    }
}

/// Pure scrollbar thumb geometry for a vertical track of `viewport_h` rows
/// showing `content_len` rows of content scrolled to top-line offset `top`.
///
/// Returns `None` when no scrollbar should be drawn: a zero-height track, or
/// content that fits entirely within the viewport (`content_len <= viewport_h`),
/// or a degenerate zero `max_scroll`.
///
/// All fields are `usize`; callers narrow to the `u16` ratatui geometry through
/// [`crate::scroll::to_u16_clamped`]. The thumb length is the proportional
/// `track * track / content`, clamped to `[1, track]`, and the thumb top is
/// `top * travel / max_scroll`. The `top * travel` product is widened to `u128`
/// so very large transcripts (the point of the `usize` migration) cannot
/// overflow; the quotient is bounded by `travel < track`, so narrowing back to
/// `usize` is lossless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThumbGeometry {
    /// Offset of the thumb's top edge from the top of the track, in rows.
    pub(crate) thumb_offset: usize,
    /// Length of the thumb, in rows (always `>= 1` when present).
    pub(crate) thumb_len: usize,
    /// The `max_top_offset` this geometry was computed against. Callers that
    /// hit-test a click/drag back into a scroll position need it, and recomputing
    /// it would re-derive `content_len - viewport_h` for no reason.
    pub(crate) max_scroll: usize,
}

/// Compute the [`ThumbGeometry`] for the given content/viewport and resolved
/// top-line offset. See [`ThumbGeometry`] for the `None` cases.
#[must_use]
pub(crate) fn thumb_geometry(
    content_len: usize,
    viewport_h: usize,
    top: usize,
) -> Option<ThumbGeometry> {
    let track_height = viewport_h;
    if track_height == 0 || content_len <= track_height {
        return None;
    }
    let max_scroll = content_len.saturating_sub(track_height);
    if max_scroll == 0 {
        return None;
    }
    let thumb_len = ((track_height * track_height) / content_len).clamp(1, track_height);
    let travel = track_height.saturating_sub(thumb_len);
    let top = top.min(max_scroll);
    let thumb_offset = if travel == 0 {
        0
    } else {
        ((top as u128 * travel as u128) / max_scroll as u128) as usize
    };
    Some(ThumbGeometry {
        thumb_offset,
        thumb_len,
        max_scroll,
    })
}

#[cfg(test)]
#[path = "logical_scroll_tests.rs"]
mod tests;
