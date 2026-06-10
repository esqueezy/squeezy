//! Minimap turn rail (§11.2 / backlog 11G.3): a compact vertical rail that
//! shows where the user turns, tool calls, and errors sit in the whole session
//! plus the slice the viewport is currently looking at — and, when terminal
//! mouse capture is on, lets a click on a rail cell jump straight to that turn.
//!
//! This module is deliberately PURE. It owns only the geometry math:
//!   - classify each transcript entry into a [`RailMarker`] kind (user turn,
//!     tool call, error, or other),
//!   - lay those markers out onto a fixed number of rail rows by mapping each
//!     entry's wrapped-row offset through the same total-row span the transcript
//!     scroll uses, and
//!   - compute which rail rows the current viewport covers.
//!
//! It does NOT touch a terminal, a `Frame`, or `TuiApp`: `lib.rs` feeds it the
//! per-entry kinds + row offsets it already computes for the scroll/jump math
//! (`transcript_lines_and_entry_offsets`), gets back a [`RailLayout`], paints
//! the cells, and registers one frame-local hit target per occupied rail cell
//! keyed by the cell's entry id. Keeping the layout here means the marker
//! placement, the viewport band, and the cell→entry resolution are all
//! unit-testable without a screen, exactly like `jump_marks`/`scroll`.
//!
//! The rail is OFF by default and toggled by a keybinding, so an idle session
//! paints nothing extra and the accessibility audit surfaces (which never
//! toggle it on) are unaffected.

/// Semantic category of a transcript entry as it appears on the rail. The
/// renderer maps each to a distinct, screen-reader-safe chrome glyph + color so
/// the rail conveys structure by shape, not color alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RailMarker {
    /// A user turn — the spine of the session; the most prominent marker.
    UserTurn,
    /// A tool call (a tool-result lead).
    ToolCall,
    /// A failure surface: a failed tool, a failure log, or a failed message.
    Error,
    /// Anything else worth a tick (assistant prose, reasoning, notes). Drawn as
    /// a quiet dot so the structural markers stand out.
    Other,
}

impl RailMarker {
    /// The chrome glyph for this marker. All four are in the accessibility
    /// gate's `ALLOWED_CHROME_GLYPHS` set, so the rail never trips the
    /// minimal-glyph gate. Distinct shapes (not just colors) so the rail is
    /// legible without color.
    pub(crate) fn glyph(self) -> &'static str {
        match self {
            RailMarker::UserTurn => "●",
            RailMarker::ToolCall => "◦",
            RailMarker::Error => "▲",
            RailMarker::Other => "·",
        }
    }

    /// Relative drawing priority when two markers collapse onto the same rail
    /// row: an error beats a user turn beats a tool call beats other. Higher
    /// wins. Keeps a single-row rail from hiding an error behind a quieter tick.
    fn priority(self) -> u8 {
        match self {
            RailMarker::Error => 3,
            RailMarker::UserTurn => 2,
            RailMarker::ToolCall => 1,
            RailMarker::Other => 0,
        }
    }
}

/// One entry's contribution to the rail: its stable entry id, its semantic
/// marker kind, and the wrapped-row offset of its header (the same unit
/// `transcript_lines_and_entry_offsets` returns), used to place it vertically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RailEntry {
    pub(crate) entry_id: u64,
    pub(crate) marker: RailMarker,
    pub(crate) row_offset: usize,
}

/// One occupied rail cell after layout: the rail row it lands on (0 = top), the
/// winning marker kind, and the entry id to jump to when the cell is clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RailCell {
    pub(crate) rail_row: u16,
    pub(crate) marker: RailMarker,
    pub(crate) entry_id: u64,
}

/// The computed rail: the occupied cells (one per rail row that won a marker)
/// plus the half-open `[viewport_top, viewport_end)` band of rail rows the
/// current viewport covers, so the renderer can tint that slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RailLayout {
    /// Occupied cells, ascending by `rail_row`, at most one per row.
    pub(crate) cells: Vec<RailCell>,
    /// First rail row covered by the viewport (inclusive).
    pub(crate) viewport_top: u16,
    /// One past the last rail row covered by the viewport (exclusive). Always
    /// `> viewport_top` when `rail_height > 0`, so the band is never empty.
    pub(crate) viewport_end: u16,
}

impl RailLayout {
    /// The marker at `rail_row`, if that row is occupied. Linear over `cells`
    /// (a rail is short: bounded by terminal height), used by the renderer and
    /// the hit-test.
    pub(crate) fn cell_at(&self, rail_row: u16) -> Option<RailCell> {
        self.cells.iter().copied().find(|c| c.rail_row == rail_row)
    }

    /// True when `rail_row` falls inside the viewport band.
    pub(crate) fn row_in_viewport(&self, rail_row: u16) -> bool {
        rail_row >= self.viewport_top && rail_row < self.viewport_end
    }
}

/// Project a wrapped-row offset in `[0, total_rows)` onto a rail row in
/// `[0, rail_height)`. Linear map, saturating at the ends. With a single total
/// row everything lands on row 0.
fn project_row(offset: usize, total_rows: usize, rail_height: u16) -> u16 {
    if rail_height == 0 {
        return 0;
    }
    let last_rail = rail_height - 1;
    if total_rows <= 1 {
        return 0;
    }
    // Map offset/(total_rows-1) onto 0..=last_rail, rounding to nearest.
    let span = total_rows - 1;
    let scaled = (offset.min(span) * usize::from(last_rail) * 2 + span) / (span * 2);
    (scaled as u16).min(last_rail)
}

/// Build the rail layout for a frame.
///
/// - `entries` are the per-entry rail contributions, ascending by `row_offset`
///   (the order `transcript_lines_and_entry_offsets` yields).
/// - `total_rows` is the wrapped-row count of the whole transcript.
/// - `top_row` is the wrapped row currently at the top of the viewport.
/// - `viewport_h` is the viewport height in wrapped rows.
/// - `rail_height` is how many cells tall the rail is painted.
///
/// Each entry is projected onto a rail row; when several land on the same row
/// the highest-priority marker wins (so an error never hides behind a quieter
/// tick). The viewport band is the same projection applied to the top and
/// bottom visible rows. Returns an empty-cell layout when there is nothing to
/// show (no entries or a zero-height rail), with the band clamped so the
/// renderer can always trust `viewport_top < viewport_end` for a non-empty rail.
pub(crate) fn build_layout(
    entries: &[RailEntry],
    total_rows: usize,
    top_row: usize,
    viewport_h: usize,
    rail_height: u16,
) -> RailLayout {
    if rail_height == 0 {
        return RailLayout {
            cells: Vec::new(),
            viewport_top: 0,
            viewport_end: 0,
        };
    }
    // Place markers, keeping the highest-priority one per rail row.
    let mut by_row: Vec<Option<RailCell>> = vec![None; usize::from(rail_height)];
    for entry in entries {
        let rail_row = project_row(entry.row_offset, total_rows, rail_height);
        let slot = &mut by_row[usize::from(rail_row)];
        let replace = match slot {
            None => true,
            Some(existing) => entry.marker.priority() > existing.marker.priority(),
        };
        if replace {
            *slot = Some(RailCell {
                rail_row,
                marker: entry.marker,
                entry_id: entry.entry_id,
            });
        }
    }
    let cells: Vec<RailCell> = by_row.into_iter().flatten().collect();

    // The viewport band: project the top and (inclusive) bottom visible rows.
    // A `bottom_row` past the content end is clamped into range so the band
    // never collapses or overshoots the rail.
    let last_rail = rail_height - 1;
    let viewport_top = project_row(top_row, total_rows, rail_height);
    let bottom_row = top_row
        .saturating_add(viewport_h.saturating_sub(1))
        .min(total_rows.saturating_sub(1));
    let viewport_bottom = project_row(bottom_row, total_rows, rail_height).max(viewport_top);
    // Half-open end, at least one row tall, capped at the rail bottom.
    let viewport_end = viewport_bottom
        .saturating_add(1)
        .min(last_rail.saturating_add(1));

    RailLayout {
        cells,
        viewport_top,
        viewport_end,
    }
}

#[cfg(test)]
#[path = "minimap_tests.rs"]
mod tests;
