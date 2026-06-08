use super::*;
use crate::termsim::types::Grid;

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
fn cursor_bounds_checks_against_frame_height() {
    let mark = FrameMark {
        byte_offset: 0,
        w: 80,
        h: 24,
    };
    let in_bounds = Grid {
        cursor: (5, 3),
        ..Grid::default()
    };
    assert!(cursor_row_in_bounds(&in_bounds, mark).is_ok());
    let escaped = Grid {
        cursor: (5, 24),
        ..Grid::default()
    };
    assert!(cursor_row_in_bounds(&escaped, mark).is_err());
}
