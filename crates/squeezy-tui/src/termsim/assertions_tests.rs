use super::*;
use crate::termsim::types::{CursorTracking, Grid};

fn grid_with_viewport(rows: &[&str]) -> Grid {
    Grid {
        viewport: rows.iter().map(|s| s.to_string()).collect(),
        ..Grid::default()
    }
}

#[test]
fn composer_horizon_matches_coin_then_dashes_not_turn_divider() {
    assert!(is_composer_horizon("☽────────────"));
    assert!(is_composer_horizon("☽ ╌╌┈┈"));
    // The turn divider has the moon followed by a space then "Worked",
    // never a dash, so it is NOT a composer horizon.
    assert!(!is_composer_horizon("   ╰─☽ Worked for 2s ───────"));
    assert!(!is_composer_horizon("plain text row"));
}

#[test]
fn at_most_one_horizon_passes_for_one_fails_for_two() {
    let one = grid_with_viewport(&["body line", "☽────────"]);
    assert!(at_most_one_composer_horizon(&one).is_ok());

    let two = grid_with_viewport(&["☽────────", "more body", "☽────────"]);
    assert!(at_most_one_composer_horizon(&two).is_err());
}

#[test]
fn turn_divider_count_respects_max() {
    let g = grid_with_viewport(&["   ╰─☽ Worked for 2s ──", "body"]);
    assert!(no_duplicate_turn_divider(&g, 1).is_ok());
    assert!(no_duplicate_turn_divider(&g, 0).is_err());

    let two = grid_with_viewport(&["Worked for 1s", "Worked for 2s"]);
    assert!(no_duplicate_turn_divider(&two, 1).is_err());
}

#[test]
fn latest_response_found_in_viewport_or_scrollback() {
    let mut g = grid_with_viewport(&["the answer tailword"]);
    assert!(latest_response_present(&g, "tailword").is_ok());
    assert!(latest_response_present(&g, "missing").is_err());
    // Empty needle passes vacuously.
    assert!(latest_response_present(&g, "").is_ok());

    g.viewport.clear();
    g.scrollback = vec!["committed tailword line".to_string()];
    assert!(latest_response_present(&g, "tailword").is_ok());
}

#[test]
fn wide_run_present_strips_spacers_and_pins_contiguity() {
    // The grid stores each wide glyph's trailing cell as a blank spacer, so a
    // healthy run reads as "你 好 世 界". Despacing must recover the contiguous
    // run.
    let g = grid_with_viewport(&["   ☽ 你 好 世 界 你 好 世 界"]);
    assert!(wide_run_present(&g, "你好世界你好世界").is_ok());
    // Empty needle passes vacuously.
    assert!(wide_run_present(&g, "").is_ok());
    // A glyph dropped from the run breaks contiguity -> fail.
    let dropped = grid_with_viewport(&["   ☽ 你 好 界 你 好 世 界"]);
    assert!(wide_run_present(&dropped, "你好世界你好世界").is_err());
    // A reordered run breaks contiguity -> fail.
    let reordered = grid_with_viewport(&["   ☽ 好 你 世 界 你 好 世 界"]);
    assert!(wide_run_present(&reordered, "你好世界你好世界").is_err());
}

#[test]
fn wide_run_present_recovers_run_split_across_wrapped_rows() {
    // A narrow reflow can wrap the run across rows; despacing across the joined
    // rows must still recover the contiguous run.
    let g = grid_with_viewport(&["你 好 世", "界 你 好 世 界"]);
    assert!(wide_run_present(&g, "你好世界你好世界").is_ok());
}

#[test]
fn latest_response_found_via_joined_rows_across_wrap_boundary() {
    // A reflow split the logical line "the final answer" across two viewport
    // rows: "the final" then "answer". No single row contains the wrapped
    // needle, so the per-row pass must miss it and the joined-rows fallback
    // (rows joined by "\n") must recognize it.
    let g = grid_with_viewport(&["the final", "answer"]);
    // Per-row alone cannot see across the boundary.
    assert!(!g.viewport.iter().any(|r| r.contains("final\nanswer")));
    // The joined fallback reconstructs the boundary and finds the tail.
    assert!(latest_response_present(&g, "final\nanswer").is_ok());
    // A needle that is nowhere in either form still fails.
    assert!(latest_response_present(&g, "final answer").is_err());
}

#[test]
fn latest_response_found_via_joined_rows_across_scrollback_viewport_seam() {
    // The split can also straddle the scrollback→viewport seam (the last
    // committed row + the first live row). The fallback joins the two iterators
    // in that order, so a needle spanning the seam is still found.
    let mut g = grid_with_viewport(&["answer"]);
    g.scrollback = vec!["the final".to_string()];
    assert!(latest_response_present(&g, "final\nanswer").is_ok());
}

#[test]
fn cursor_bounds_checks_against_frame_height() {
    let mark = FrameMark {
        byte_offset: 0,
        w: 80,
        h: 24,
    };
    // The invariant reads the PRE-clamp `logical_cursor_row`, not the clamped
    // `cursor.1` (which the backends keep in-grid by construction).
    let in_bounds = Grid {
        logical_cursor_row: 3,
        ..Grid::default()
    };
    assert!(cursor_row_in_bounds(&in_bounds, mark).is_ok());
    // Below the live region (the xterm.js drift): logical row >= h.
    let escaped_below = Grid {
        logical_cursor_row: 24,
        ..Grid::default()
    };
    assert!(cursor_row_in_bounds(&escaped_below, mark).is_err());
    // Above the viewport top: a negative logical row also escapes bounds.
    let escaped_above = Grid {
        logical_cursor_row: -1,
        ..Grid::default()
    };
    assert!(cursor_row_in_bounds(&escaped_above, mark).is_err());
}

#[test]
fn below_wrap_drift_profile_trips_cursor_bounds_in_process() {
    // Drive the named `DriftsByBelowWrapDelta` profile (the xterm.js regression
    // the matrix exists to catch) through the SAME cursor-bounds invariant that
    // run_matrix uses, proving the assertion fires on a drifting emulator —
    // not just on a hand-built out-of-bounds Grid literal. The Rust legs never
    // produce this drift themselves, so this is its only in-process exercise.
    let mark = FrameMark {
        byte_offset: 0,
        w: 80,
        h: 24,
    };
    // A cursor that sits on the last live row, plus 5 wrapped rows that fell
    // below the fold. The well-behaved profile keeps it in bounds; the drift
    // profile pushes it below the viewport and the invariant must catch it.
    let base_row = 23;
    let below_fold = 5;

    let stable = Grid {
        logical_cursor_row: CursorTracking::TracksLogicalLine
            .project_logical_row(base_row, below_fold),
        ..Grid::default()
    };
    assert!(cursor_row_in_bounds(&stable, mark).is_ok());

    let drifted = Grid {
        logical_cursor_row: CursorTracking::DriftsByBelowWrapDelta
            .project_logical_row(base_row, below_fold),
        ..Grid::default()
    };
    assert!(
        cursor_row_in_bounds(&drifted, mark).is_err(),
        "the below-wrap drift must push the logical cursor past the viewport",
    );
}
