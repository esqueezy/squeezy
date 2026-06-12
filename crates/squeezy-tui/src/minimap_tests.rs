use super::*;

fn entry(id: u64, marker: RailMarker, row_offset: usize) -> RailEntry {
    RailEntry {
        entry_id: id,
        marker,
        row_offset,
    }
}

#[test]
fn empty_entries_yield_no_cells_but_a_valid_viewport_band() {
    let layout = build_layout(&[], 100, 0, 20, 10);
    assert!(layout.cells.is_empty());
    // The band is still well-formed for a non-empty rail.
    assert!(layout.viewport_top < layout.viewport_end);
    assert_eq!(layout.viewport_top, 0);
}

#[test]
fn zero_height_rail_is_a_safe_empty_layout() {
    let entries = [entry(1, RailMarker::UserTurn, 0)];
    let layout = build_layout(&entries, 100, 0, 20, 0);
    assert!(layout.cells.is_empty());
    assert_eq!(layout.viewport_top, 0);
    assert_eq!(layout.viewport_end, 0);
}

#[test]
fn markers_project_top_middle_bottom_onto_the_rail() {
    // 100 wrapped rows projected onto a 10-row rail. An entry at the very top
    // lands on row 0, one near the end lands on the last rail row.
    let entries = [
        entry(1, RailMarker::UserTurn, 0),
        entry(2, RailMarker::ToolCall, 50),
        entry(3, RailMarker::Error, 99),
    ];
    let layout = build_layout(&entries, 100, 0, 20, 10);
    assert_eq!(layout.cell_at(0).map(|c| c.entry_id), Some(1));
    assert_eq!(layout.cell_at(9).map(|c| c.entry_id), Some(3));
    // The middle marker lands somewhere in the interior.
    let mid = layout
        .cells
        .iter()
        .find(|c| c.entry_id == 2)
        .expect("middle marker placed");
    assert!(mid.rail_row > 0 && mid.rail_row < 9, "{mid:?}");
}

#[test]
fn higher_priority_marker_wins_a_shared_rail_row() {
    // Two entries collapse onto the same rail row (both near the top of a tall
    // transcript over a short rail). The error must beat the user turn.
    let entries = [
        entry(1, RailMarker::UserTurn, 0),
        entry(2, RailMarker::Error, 1),
    ];
    let layout = build_layout(&entries, 200, 0, 20, 4);
    let cell = layout.cell_at(0).expect("a marker on row 0");
    assert_eq!(cell.marker, RailMarker::Error);
    assert_eq!(
        cell.entry_id, 2,
        "the error entry id must be the jump target"
    );
}

#[test]
fn at_most_one_cell_per_rail_row() {
    // A dense cluster of entries all near the top collapses to a single cell.
    let entries: Vec<RailEntry> = (0..20)
        .map(|i| entry(i, RailMarker::Other, i as usize))
        .collect();
    let layout = build_layout(&entries, 1000, 0, 20, 5);
    let mut rows: Vec<u16> = layout.cells.iter().map(|c| c.rail_row).collect();
    rows.sort_unstable();
    let unique = {
        let mut r = rows.clone();
        r.dedup();
        r
    };
    assert_eq!(rows, unique, "no rail row may host two cells");
}

#[test]
fn viewport_band_tracks_the_top_visible_slice() {
    // Scrolled to the very top: the band starts at rail row 0.
    let entries = [entry(1, RailMarker::UserTurn, 0)];
    let top = build_layout(&entries, 100, 0, 20, 10);
    assert_eq!(top.viewport_top, 0);
    assert!(top.row_in_viewport(0));

    // Scrolled to the tail: the band reaches the last rail row.
    let bottom = build_layout(&entries, 100, 80, 20, 10);
    assert!(bottom.row_in_viewport(9), "tail band must cover the bottom");
    assert_eq!(bottom.viewport_end, 10);
}

#[test]
fn viewport_band_is_never_empty_for_a_nonempty_rail() {
    // A 1-row viewport still produces a 1-row band (top < end).
    let layout = build_layout(&[], 100, 40, 1, 10);
    assert!(layout.viewport_top < layout.viewport_end);
}

#[test]
fn single_row_transcript_places_everything_on_row_zero() {
    let entries = [entry(1, RailMarker::UserTurn, 0)];
    let layout = build_layout(&entries, 1, 0, 1, 8);
    assert_eq!(layout.cell_at(0).map(|c| c.entry_id), Some(1));
    assert!(layout.row_in_viewport(0));
}

#[test]
fn row_in_viewport_is_half_open() {
    let layout = RailLayout {
        cells: Vec::new(),
        viewport_top: 2,
        viewport_end: 5,
    };
    assert!(!layout.row_in_viewport(1));
    assert!(layout.row_in_viewport(2));
    assert!(layout.row_in_viewport(4));
    assert!(!layout.row_in_viewport(5), "end is exclusive");
}

#[test]
fn marker_glyphs_are_distinct_and_chrome_safe() {
    // All four markers must use distinct glyphs so the rail is legible without
    // color, and each must be a single visible cell.
    let glyphs = [
        RailMarker::UserTurn.glyph(),
        RailMarker::ToolCall.glyph(),
        RailMarker::Error.glyph(),
        RailMarker::Other.glyph(),
    ];
    let mut seen = glyphs.to_vec();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), glyphs.len(), "marker glyphs must be distinct");
    for g in glyphs {
        assert_eq!(g.chars().count(), 1, "marker {g:?} must be one cell");
    }
}

#[test]
fn priority_order_error_above_user_above_tool_above_other() {
    assert!(RailMarker::Error.priority() > RailMarker::UserTurn.priority());
    assert!(RailMarker::UserTurn.priority() > RailMarker::ToolCall.priority());
    assert!(RailMarker::ToolCall.priority() > RailMarker::Other.priority());
}
