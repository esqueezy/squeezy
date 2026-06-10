//! Horizontal navigation for wide blocks (§11.2 / backlog 11G.4).
//!
//! Wide code/diff blocks and long single-line command output are the one place
//! the soft-wrapping main transcript fights the content: a 200-column `cat` of a
//! minified file or a wide unified diff either wraps into an unreadable zig-zag
//! or (worse) gets visually truncated. The spec's fix is a per-view choice —
//! **soft-wrap toggle OR horizontal scroll inside the block** — and explicitly
//! forbids hiding long output. This module owns the pure state and math for
//! both: a single toggle that flips the whole main view between *wrap* (the
//! default, every line reflows to the column) and *no-wrap* (lines keep their
//! natural width and the viewport pans left/right over them), plus the clamped
//! horizontal offset the renderer feeds to `Paragraph::scroll`.
//!
//! It is deliberately pure: it knows nothing about ratatui, the row model, or
//! input. `lib.rs` owns one [`WideBlockView`], flips it from a keybinding
//! (`Alt+w`), pans it from keys (`Alt+h`/`Alt+l`) and Shift+wheel, measures the
//! widest painted line, and asks this module for the `(soft_wrap, h_offset)`
//! pair to drive the paragraph. Keeping the clamp/toggle/pan math here means it
//! is exhaustively unit-testable without a terminal, and the renderer stays a
//! thin consumer.
//!
//! Identity note: the offset is a *column* count, not tied to any entry id. It
//! is a view-local pan that resets to 0 whenever wrap is re-enabled (a wrapped
//! view has no horizontal axis to pan) or when the content/width shrinks the
//! pan out of range (handled by [`WideBlockView::clamp`]). No reflow can leave
//! it pointing past the content, so a stale offset can never hide output.

/// How far one `Alt+h`/`Alt+l` keypress (or one Shift+wheel notch) pans the
/// viewport horizontally. Matches the vertical wheel notch (3 lines) so the two
/// axes feel symmetric; small enough that a single keypress is a precise nudge,
/// large enough that panning a wide diff doesn't take dozens of presses.
pub(crate) const HORIZONTAL_STEP_COLUMNS: u16 = 8;

/// Pure horizontal-navigation state for the main transcript view.
///
/// Two fields, both view-local:
///   - `soft_wrap`: the master switch. `true` (default) is today's behaviour —
///     every transcript line reflows to the text column. `false` switches the
///     whole view to no-wrap: lines keep their natural width and the viewport
///     pans over them with `h_offset`.
///   - `h_offset`: the leftmost painted column when `soft_wrap` is `false`. Always
///     `0` while wrapping (a wrapped view has no horizontal axis), and always
///     clamped so it can never scroll past the widest line — output is panned,
///     never hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WideBlockView {
    soft_wrap: bool,
    h_offset: u16,
}

impl Default for WideBlockView {
    fn default() -> Self {
        // Soft-wrap on is the established main-view behaviour; horizontal nav is
        // strictly opt-in so an unchanged session paints exactly as before.
        Self {
            soft_wrap: true,
            h_offset: 0,
        }
    }
}

impl WideBlockView {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Whether the view soft-wraps (the default) or pans horizontally.
    pub(crate) fn soft_wrap(self) -> bool {
        self.soft_wrap
    }

    /// The raw stored horizontal offset, `0` while wrapping. Test-only: the
    /// renderer paints through [`painted_offset`](Self::painted_offset) (which
    /// also clamps against live geometry); this is the bare-state read the unit
    /// tests assert on, mirroring the `#[cfg(test)]` raw accessor pattern in
    /// `jump_marks.rs`.
    #[cfg(test)]
    pub(crate) fn horizontal_offset(self) -> u16 {
        if self.soft_wrap { 0 } else { self.h_offset }
    }

    /// The offset to actually PAINT this frame, clamped against the live
    /// content/viewport widths WITHOUT mutating stored state. The renderer holds
    /// `&TuiApp` (no `&mut`), so it cannot re-clamp the stored offset on a resize;
    /// instead it paints this locally-clamped value, and the next keyboard/wheel
    /// pan re-clamps the stored offset through [`scroll_right`](Self::scroll_right)
    /// / [`clamp`](Self::clamp). Always `0` while wrapping.
    pub(crate) fn painted_offset(self, content_width: u16, viewport_width: u16) -> u16 {
        if self.soft_wrap {
            return 0;
        }
        self.h_offset
            .min(Self::max_offset(content_width, viewport_width))
    }

    /// Flip soft-wrap on/off. Re-enabling wrap resets the pan to `0` because the
    /// wrapped view has no horizontal axis to preserve; turning wrap *off* keeps
    /// the offset at `0` so the no-wrap view always starts flush-left. Returns
    /// the new `soft_wrap` value so the caller can word the status line.
    pub(crate) fn toggle_wrap(&mut self) -> bool {
        self.soft_wrap = !self.soft_wrap;
        self.h_offset = 0;
        self.soft_wrap
    }

    /// Pan left by `cols`, saturating at the flush-left edge (offset `0`).
    /// A no-op while wrapping (there is no horizontal axis). Returns `true` when
    /// the offset actually moved, so the caller can decide whether to redraw and
    /// avoid an idle repaint on a no-op pan.
    pub(crate) fn scroll_left(&mut self, cols: u16) -> bool {
        if self.soft_wrap {
            return false;
        }
        let next = self.h_offset.saturating_sub(cols);
        let moved = next != self.h_offset;
        self.h_offset = next;
        moved
    }

    /// Pan right by `cols`, clamped so the offset never exceeds
    /// [`max_offset`](Self::max_offset) for the given content/viewport widths —
    /// the widest line's last column always stays reachable but the view never
    /// scrolls past it into emptiness. A no-op while wrapping. Returns `true`
    /// when the offset actually moved.
    pub(crate) fn scroll_right(
        &mut self,
        cols: u16,
        content_width: u16,
        viewport_width: u16,
    ) -> bool {
        if self.soft_wrap {
            return false;
        }
        let max = Self::max_offset(content_width, viewport_width);
        let next = self.h_offset.saturating_add(cols).min(max);
        let moved = next != self.h_offset;
        self.h_offset = next;
        moved
    }

    /// Re-clamp the offset against the current content/viewport widths. Called
    /// after a resize or a content change shrinks the widest line: an offset that
    /// was valid for a wide diff must snap back in-range when the diff scrolls
    /// off, so the no-wrap view can never come to rest showing only blank columns
    /// to the right of all content. Returns `true` when the clamp moved the
    /// offset.
    pub(crate) fn clamp(&mut self, content_width: u16, viewport_width: u16) -> bool {
        if self.soft_wrap {
            // Defensive: wrapping always pins the offset to 0.
            let moved = self.h_offset != 0;
            self.h_offset = 0;
            return moved;
        }
        let max = Self::max_offset(content_width, viewport_width);
        if self.h_offset > max {
            self.h_offset = max;
            true
        } else {
            false
        }
    }

    /// The largest in-range horizontal offset: how far the viewport can pan
    /// before the widest line's final column sits at the right edge. `0` when the
    /// content already fits (nothing to pan).
    pub(crate) fn max_offset(content_width: u16, viewport_width: u16) -> u16 {
        content_width.saturating_sub(viewport_width)
    }

    /// One-line status hint describing the current mode, for the status line.
    /// Wrapping reports the toggle; no-wrap reports the pan position so the user
    /// knows there is more content to the right (or that they are flush-left).
    pub(crate) fn status_hint(self) -> &'static str {
        if self.soft_wrap {
            "soft-wrap on — Alt+w for horizontal scroll"
        } else if self.h_offset == 0 {
            "soft-wrap off — Alt+l / Shift+wheel to scroll right"
        } else {
            "soft-wrap off — Alt+h / Alt+l to pan, Alt+w to wrap"
        }
    }
}

#[cfg(test)]
#[path = "wide_block_tests.rs"]
mod tests;
