//! Unit tests for the pure Smart Split Panes layout solver + overlay model
//! (§12.4.2).
//!
//! These cover the solver across the side / stacked / single-column thresholds,
//! the aspect-ratio `Auto` decision, the split-ratio bias + clamp, the pane-kind
//! / orientation cycle, the overlay cursor/adjust/reset rules, and the geometry
//! helpers — all in isolation, no terminal, no `TuiApp`. The overlay's behaviour
//! through the real `render()` + key/mouse dispatch is covered by the
//! capture-sink suite in `lib_tests.rs`.

use super::*;
use ratatui::layout::Rect;

fn content(width: u16, height: u16) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width,
        height,
    }
}

// ---------------------------------------------------------------------------
// PaneKind
// ---------------------------------------------------------------------------

#[test]
fn pane_kind_all_holds_every_kind_in_order() {
    assert_eq!(
        PaneKind::ALL,
        [PaneKind::Detail, PaneKind::Scratch, PaneKind::Compare]
    );
    for (i, kind) in PaneKind::ALL.iter().enumerate() {
        assert_eq!(kind.index(), i, "index must match ALL position");
    }
}

#[test]
fn pane_kind_fractions_and_floors_are_sane() {
    for kind in PaneKind::ALL {
        let (num, den) = kind.ideal_fraction();
        assert!(num > 0 && den > 0 && num < den, "{:?}", kind);
        assert!(kind.min_main() >= 20, "{:?}", kind);
        assert!(!kind.label().is_empty());
        assert!(!kind.description().is_empty());
    }
    // The compare view splits down the middle; detail/scratch keep the
    // transcript as the majority. Compare the fractions by cross-multiplying
    // (different denominators) rather than the raw numerators.
    assert_eq!(PaneKind::Compare.ideal_fraction(), (1, 2));
    let (dn, dd) = PaneKind::Detail.ideal_fraction();
    let (cn, cd) = PaneKind::Compare.ideal_fraction();
    assert!(
        u32::from(dn) * u32::from(cd) < u32::from(cn) * u32::from(dd),
        "the detail pane takes a smaller share than the even compare split"
    );
}

// ---------------------------------------------------------------------------
// Orientation
// ---------------------------------------------------------------------------

#[test]
fn orientation_cycles_through_all_and_wraps() {
    assert_eq!(Orientation::Auto.next(), Orientation::Side);
    assert_eq!(Orientation::Side.next(), Orientation::Stacked);
    assert_eq!(Orientation::Stacked.next(), Orientation::Auto);
    // Three nexts return to the start.
    let mut o = Orientation::Auto;
    for _ in 0..Orientation::ALL.len() {
        o = o.next();
    }
    assert_eq!(o, Orientation::Auto);
}

// ---------------------------------------------------------------------------
// SplitRatio
// ---------------------------------------------------------------------------

#[test]
fn split_ratio_widens_and_narrows_within_bounds() {
    let mut r = SplitRatio::NEUTRAL;
    assert_eq!(r.steps(), 0);
    // Widen up to the cap, then refuse.
    let mut widened = 0;
    while r.widen() {
        widened += 1;
        assert!(widened <= 8, "widen must terminate at the cap");
    }
    assert!(r.steps() > 0, "ratio must move from neutral");
    assert!(!r.widen(), "widen past the cap is a no-op");
    // Narrow all the way back past neutral to the floor, then refuse.
    while r.narrow() {}
    assert!(r.steps() < 0, "ratio must move negative");
    assert!(!r.narrow(), "narrow past the floor is a no-op");
}

// ---------------------------------------------------------------------------
// LayoutSolver — side / stacked / single-column thresholds
// ---------------------------------------------------------------------------

#[test]
fn solver_splits_side_by_side_on_a_wide_terminal() {
    let solver = LayoutSolver::default();
    // 120x30 is comfortably wide; Auto should pick a side split.
    let placement = solver.solve(
        content(120, 30),
        PaneKind::Detail,
        Orientation::Auto,
        SplitRatio::NEUTRAL,
    );
    assert!(placement.is_split());
    assert_eq!(placement.label(), "side-by-side");
    let PanePlacement::Side {
        main,
        separator,
        pane,
    } = placement
    else {
        panic!("expected a side split, got {placement:?}");
    };
    // The three rects tile the content exactly along the width with no overlap.
    assert_eq!(main.x, 0);
    assert_eq!(separator.x, main.width);
    assert_eq!(separator.width, 1);
    assert_eq!(pane.x, main.width + 1);
    assert_eq!(main.width + separator.width + pane.width, 120);
    // Both columns keep the full height.
    assert_eq!(main.height, 30);
    assert_eq!(pane.height, 30);
    // The transcript keeps at least its min_main floor.
    assert!(main.width >= PaneKind::Detail.min_main());
}

#[test]
fn solver_stacks_on_a_narrow_but_tall_terminal() {
    let solver = LayoutSolver::default();
    // 40 wide is below MIN_SIDE_WIDTH (64) but 30 tall is plenty for a stack.
    let placement = solver.solve(
        content(40, 30),
        PaneKind::Detail,
        Orientation::Auto,
        SplitRatio::NEUTRAL,
    );
    assert!(placement.is_split());
    assert_eq!(placement.label(), "stacked");
    let PanePlacement::Stacked {
        main,
        separator,
        pane,
    } = placement
    else {
        panic!("expected a stacked split, got {placement:?}");
    };
    // The three rects tile the content exactly along the height.
    assert_eq!(main.y, 0);
    assert_eq!(separator.y, main.height);
    assert_eq!(separator.height, 1);
    assert_eq!(pane.y, main.height + 1);
    assert_eq!(main.height + separator.height + pane.height, 30);
    // Both bands keep the full width.
    assert_eq!(main.width, 40);
    assert_eq!(pane.width, 40);
}

#[test]
fn solver_degrades_to_single_column_when_too_small_for_either_split() {
    let solver = LayoutSolver::default();
    // 40 wide (< MIN_SIDE_WIDTH) and 5 tall (< MIN_STACK_HEIGHT): neither fits.
    let placement = solver.solve(
        content(40, 5),
        PaneKind::Detail,
        Orientation::Auto,
        SplitRatio::NEUTRAL,
    );
    assert!(!placement.is_split());
    assert_eq!(placement.label(), "single column");
    assert_eq!(placement.pane(), None, "no pane in a single column");
    assert_eq!(
        placement.separator(),
        None,
        "no separator in a single column"
    );
    // The transcript keeps the whole content.
    assert_eq!(placement.main(), content(40, 5));
}

#[test]
fn solver_yields_single_column_for_zero_area() {
    let solver = LayoutSolver::default();
    for rect in [content(0, 24), content(80, 0), content(0, 0)] {
        let placement = solver.solve(
            rect,
            PaneKind::Detail,
            Orientation::Auto,
            SplitRatio::NEUTRAL,
        );
        assert!(!placement.is_split(), "zero-area must not split: {rect:?}");
        assert_eq!(placement.pane(), None);
    }
}

#[test]
fn forced_side_falls_back_to_stacked_then_single_column() {
    let solver = LayoutSolver::default();
    // Forcing Side on a too-narrow-but-tall terminal still produces a *stack*
    // (the solver tries the other orientation before giving up) rather than a
    // corrupted squeeze.
    let placement = solver.solve(
        content(40, 30),
        PaneKind::Detail,
        Orientation::Side,
        SplitRatio::NEUTRAL,
    );
    assert!(placement.is_split());
    assert!(
        matches!(placement, PanePlacement::Stacked { .. }),
        "forced side on a narrow/tall terminal should stack, got {placement:?}"
    );
    // Forcing Side on a terminal too small for either split degrades to a single
    // column.
    let tiny = solver.solve(
        content(40, 5),
        PaneKind::Detail,
        Orientation::Side,
        SplitRatio::NEUTRAL,
    );
    assert!(!tiny.is_split());
}

#[test]
fn forced_stacked_falls_back_to_side_on_a_wide_short_terminal() {
    let solver = LayoutSolver::default();
    // A wide but very short terminal cannot stack (height < MIN_STACK_HEIGHT) but
    // can side-split — forcing Stacked should still side-split rather than refuse.
    let placement = solver.solve(
        content(120, 5),
        PaneKind::Detail,
        Orientation::Stacked,
        SplitRatio::NEUTRAL,
    );
    assert!(placement.is_split());
    assert!(
        matches!(placement, PanePlacement::Side { .. }),
        "forced stacked on a wide/short terminal should side-split, got {placement:?}"
    );
}

// ---------------------------------------------------------------------------
// Aspect-ratio decision
// ---------------------------------------------------------------------------

#[test]
fn prefers_side_tracks_aspect_ratio() {
    let solver = LayoutSolver::default();
    // Very wide: prefers side.
    assert!(solver.prefers_side(content(200, 20)));
    // Tall/square-ish in cells (which read as tall visually): prefers stack.
    assert!(!solver.prefers_side(content(80, 60)));
    // Zero height never prefers side (guarded against divide-by-zero).
    assert!(!solver.prefers_side(content(80, 0)));
}

#[test]
fn auto_picks_side_for_wide_and_stacked_for_tall_when_both_fit() {
    let solver = LayoutSolver::default();
    // Wide and tall enough for either: Auto picks side (aspect favours width).
    let wide = solver.solve(
        content(160, 40),
        PaneKind::Compare,
        Orientation::Auto,
        SplitRatio::NEUTRAL,
    );
    assert!(matches!(wide, PanePlacement::Side { .. }));
    // A tall, modestly-wide terminal (still >= MIN_SIDE_WIDTH so side is possible)
    // whose aspect favours height: Auto picks the stack.
    let tall = solver.solve(
        content(64, 60),
        PaneKind::Compare,
        Orientation::Auto,
        SplitRatio::NEUTRAL,
    );
    assert!(
        matches!(tall, PanePlacement::Stacked { .. }),
        "a tall terminal should auto-stack, got {tall:?}"
    );
}

// ---------------------------------------------------------------------------
// Split-ratio bias affects the pane share, clamped so neither side starves
// ---------------------------------------------------------------------------

#[test]
fn widening_the_ratio_grows_the_pane_and_clamps() {
    let solver = LayoutSolver::default();
    let neutral = solver
        .solve(
            content(120, 30),
            PaneKind::Detail,
            Orientation::Side,
            SplitRatio::NEUTRAL,
        )
        .pane()
        .expect("side split has a pane");

    let mut wide = SplitRatio::NEUTRAL;
    wide.widen();
    wide.widen();
    let widened = solver
        .solve(content(120, 30), PaneKind::Detail, Orientation::Side, wide)
        .pane()
        .expect("side split has a pane");
    assert!(
        widened.width > neutral.width,
        "widening must grow the pane: {} vs {}",
        widened.width,
        neutral.width
    );

    let mut narrow = SplitRatio::NEUTRAL;
    narrow.narrow();
    narrow.narrow();
    let narrowed = solver
        .solve(
            content(120, 30),
            PaneKind::Detail,
            Orientation::Side,
            narrow,
        )
        .pane()
        .expect("side split has a pane");
    assert!(
        narrowed.width < neutral.width,
        "narrowing must shrink the pane"
    );
    // Even at the widest bias the transcript keeps its min_main floor.
    let mut maxed = SplitRatio::NEUTRAL;
    while maxed.widen() {}
    let placement = solver.solve(content(120, 30), PaneKind::Detail, Orientation::Side, maxed);
    assert!(placement.main().width >= PaneKind::Detail.min_main());
    assert!(placement.pane().expect("pane").width >= 1);
}

// ---------------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------------

#[test]
fn pane_inner_insets_one_cell_each_side_and_saturates() {
    let r = pane_inner(Rect {
        x: 5,
        y: 7,
        width: 20,
        height: 10,
    });
    assert_eq!(
        r,
        Rect {
            x: 6,
            y: 8,
            width: 18,
            height: 8
        }
    );
    // A 1x1 rect cannot hold inner content; the inset saturates to zero area.
    let tiny = pane_inner(Rect {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    });
    assert_eq!(tiny.width, 0);
    assert_eq!(tiny.height, 0);
}

#[test]
fn rect_contains_is_half_open_on_both_axes() {
    let r = Rect {
        x: 2,
        y: 3,
        width: 4,
        height: 5,
    };
    assert!(rect_contains(r, 2, 3), "top-left is inside");
    assert!(rect_contains(r, 5, 7), "bottom-right cell is inside");
    assert!(
        !rect_contains(r, 6, 7),
        "one past the right edge is outside"
    );
    assert!(
        !rect_contains(r, 5, 8),
        "one past the bottom edge is outside"
    );
    assert!(!rect_contains(r, 1, 3), "left of the rect is outside");
    // A zero-area rect contains nothing.
    assert!(!rect_contains(
        Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 5
        },
        0,
        0
    ));
}

// ---------------------------------------------------------------------------
// SplitField + SmartSplitState — the overlay model
// ---------------------------------------------------------------------------

#[test]
fn split_field_all_indices_round_trip() {
    assert_eq!(
        SplitField::ALL,
        [SplitField::Kind, SplitField::Orientation, SplitField::Ratio]
    );
    for (i, field) in SplitField::ALL.iter().enumerate() {
        assert_eq!(field.index(), i);
        assert!(!field.label().is_empty());
    }
}

#[test]
fn new_seeds_the_pane_kind_with_defaults() {
    let state = SmartSplitState::new(PaneKind::Compare);
    assert_eq!(state.kind(), PaneKind::Compare);
    assert_eq!(state.orientation(), Orientation::Auto);
    assert_eq!(state.ratio(), SplitRatio::NEUTRAL);
    assert_eq!(state.cursor(), 0);
    assert_eq!(state.focused_field(), SplitField::Kind);
}

#[test]
fn focus_moves_between_rows_and_clamps() {
    let mut state = SmartSplitState::new(PaneKind::Detail);
    assert!(!state.focus_prev(), "already at the top");
    assert!(state.focus_next());
    assert_eq!(state.focused_field(), SplitField::Orientation);
    assert!(state.focus_next());
    assert_eq!(state.focused_field(), SplitField::Ratio);
    assert!(!state.focus_next(), "already at the bottom");
    assert!(state.focus_prev());
    assert_eq!(state.focused_field(), SplitField::Orientation);
}

#[test]
fn focus_row_jumps_directly_and_ignores_out_of_range() {
    let mut state = SmartSplitState::new(PaneKind::Detail);
    assert!(state.focus_row(2));
    assert_eq!(state.focused_field(), SplitField::Ratio);
    assert!(!state.focus_row(2), "already focused — no move");
    assert!(!state.focus_row(99), "out of range — ignored");
    assert_eq!(state.focused_field(), SplitField::Ratio);
}

#[test]
fn adjust_forward_and_backward_cycle_the_focused_field() {
    let mut state = SmartSplitState::new(PaneKind::Detail);
    // Kind row: forward cycles Detail -> Scratch -> Compare -> Detail.
    assert!(state.adjust_forward());
    assert_eq!(state.kind(), PaneKind::Scratch);
    assert!(state.adjust_backward());
    assert_eq!(state.kind(), PaneKind::Detail);

    // Orientation row.
    state.focus_next();
    assert!(state.adjust_forward());
    assert_eq!(state.orientation(), Orientation::Side);
    assert!(state.adjust_backward());
    assert_eq!(state.orientation(), Orientation::Auto);

    // Ratio row: forward widens, backward narrows.
    state.focus_next();
    assert!(state.adjust_forward());
    assert_eq!(state.ratio().steps(), 1);
    assert!(state.adjust_backward());
    assert_eq!(state.ratio().steps(), 0);
}

#[test]
fn reset_restores_auto_orientation_and_neutral_ratio_but_keeps_kind() {
    let mut state = SmartSplitState::new(PaneKind::Compare);
    // Shape the orientation + ratio away from the defaults.
    state.focus_row(SplitField::Orientation.index());
    state.adjust_forward(); // Side
    state.focus_row(SplitField::Ratio.index());
    state.adjust_forward(); // widen
    state.adjust_forward();
    assert_eq!(state.orientation(), Orientation::Side);
    assert_eq!(state.ratio().steps(), 2);

    assert!(state.reset(), "reset reports a change");
    assert_eq!(state.orientation(), Orientation::Auto);
    assert_eq!(state.ratio(), SplitRatio::NEUTRAL);
    assert_eq!(state.kind(), PaneKind::Compare, "kind is preserved");
    assert!(!state.reset(), "reset at defaults is a no-op");
}

#[test]
fn ratio_widen_narrow_helpers_track_the_inner_bias() {
    let mut state = SmartSplitState::new(PaneKind::Detail);
    assert!(state.ratio_widen());
    assert_eq!(state.ratio().steps(), 1);
    assert!(state.ratio_narrow());
    assert_eq!(state.ratio().steps(), 0);
}

#[test]
fn state_solve_matches_the_default_solver() {
    let state = SmartSplitState::new(PaneKind::Detail);
    let direct = LayoutSolver::default().solve(
        content(120, 30),
        PaneKind::Detail,
        Orientation::Auto,
        SplitRatio::NEUTRAL,
    );
    assert_eq!(state.solve(content(120, 30)), direct);
}
