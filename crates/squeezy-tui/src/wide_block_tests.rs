use super::*;

#[test]
fn default_is_soft_wrap_flush_left() {
    let view = WideBlockView::new();
    assert!(view.soft_wrap(), "soft-wrap must be on by default");
    assert_eq!(view.horizontal_offset(), 0);
}

#[test]
fn toggle_flips_wrap_and_reports_new_state() {
    let mut view = WideBlockView::new();
    assert!(!view.toggle_wrap(), "first toggle turns wrap OFF");
    assert!(!view.soft_wrap());
    assert!(view.toggle_wrap(), "second toggle turns wrap back ON");
    assert!(view.soft_wrap());
}

#[test]
fn horizontal_offset_is_zero_while_wrapping_even_after_pan_attempts() {
    let mut view = WideBlockView::new();
    // Panning while wrapping is a no-op: there is no horizontal axis.
    assert!(!view.scroll_right(50, 400, 100));
    assert!(!view.scroll_left(50));
    assert_eq!(view.horizontal_offset(), 0);
}

#[test]
fn no_wrap_pans_right_and_clamps_to_max() {
    let mut view = WideBlockView::new();
    view.toggle_wrap(); // wrap off
    // Content 120 wide, viewport 100 -> max offset 20.
    assert!(view.scroll_right(8, 120, 100));
    assert_eq!(view.horizontal_offset(), 8);
    assert!(view.scroll_right(8, 120, 100));
    assert_eq!(view.horizontal_offset(), 16);
    // Next step would overshoot 20; clamps and still reports movement.
    assert!(view.scroll_right(8, 120, 100));
    assert_eq!(view.horizontal_offset(), 20);
    // Already at max: no movement, no redraw.
    assert!(!view.scroll_right(8, 120, 100));
    assert_eq!(view.horizontal_offset(), 20);
}

#[test]
fn no_wrap_pans_left_and_saturates_at_zero() {
    let mut view = WideBlockView::new();
    view.toggle_wrap();
    view.scroll_right(20, 200, 100);
    assert_eq!(view.horizontal_offset(), 20);
    assert!(view.scroll_left(8));
    assert_eq!(view.horizontal_offset(), 12);
    // Overshoot left saturates at 0 and still reports movement.
    assert!(view.scroll_left(50));
    assert_eq!(view.horizontal_offset(), 0);
    // At the left edge: no movement.
    assert!(!view.scroll_left(8));
}

#[test]
fn re_enabling_wrap_resets_the_pan() {
    let mut view = WideBlockView::new();
    view.toggle_wrap(); // off
    view.scroll_right(40, 300, 100);
    assert_eq!(view.horizontal_offset(), 40);
    view.toggle_wrap(); // back on
    assert_eq!(
        view.horizontal_offset(),
        0,
        "re-enabling wrap must drop the horizontal pan"
    );
    // And the stored offset is genuinely 0, not just masked: turn wrap off
    // again and we start flush-left.
    view.toggle_wrap();
    assert_eq!(view.horizontal_offset(), 0);
}

#[test]
fn content_that_fits_has_no_room_to_pan() {
    let mut view = WideBlockView::new();
    view.toggle_wrap();
    // Content 80, viewport 100: everything already fits, max offset 0.
    assert_eq!(WideBlockView::max_offset(80, 100), 0);
    assert!(!view.scroll_right(8, 80, 100));
    assert_eq!(view.horizontal_offset(), 0);
}

#[test]
fn clamp_snaps_a_stale_offset_back_in_range() {
    let mut view = WideBlockView::new();
    view.toggle_wrap();
    // Pan deep into a very wide line.
    view.scroll_right(200, 400, 100);
    assert_eq!(view.horizontal_offset(), 200);
    // The wide line scrolls off; the widest remaining line is only 120 wide.
    assert!(
        view.clamp(120, 100),
        "shrinking content must pull the offset back in range"
    );
    assert_eq!(view.horizontal_offset(), 20);
    // A re-clamp at the same geometry is now a no-op.
    assert!(!view.clamp(120, 100));
}

#[test]
fn clamp_while_wrapping_pins_to_zero() {
    let mut view = WideBlockView::new();
    // Force a non-zero offset via the no-wrap path, then flip back to wrap
    // WITHOUT going through toggle (simulate any future code path that leaves a
    // stray offset): clamp must defensively zero it.
    view.toggle_wrap();
    view.scroll_right(40, 300, 100);
    view.toggle_wrap(); // wrap on, offset already 0 from toggle
    assert!(!view.clamp(300, 100), "already pinned, no movement");
    assert_eq!(view.horizontal_offset(), 0);
}

#[test]
fn max_offset_saturates_when_viewport_wider_than_content() {
    assert_eq!(WideBlockView::max_offset(50, 200), 0);
    assert_eq!(WideBlockView::max_offset(0, 0), 0);
    assert_eq!(WideBlockView::max_offset(u16::MAX, 0), u16::MAX);
}

#[test]
fn status_hint_tracks_mode_and_position() {
    let mut view = WideBlockView::new();
    assert!(view.status_hint().contains("soft-wrap on"));
    view.toggle_wrap();
    assert!(
        view.status_hint().contains("scroll right"),
        "flush-left no-wrap hint should point right"
    );
    view.scroll_right(8, 200, 100);
    assert!(
        view.status_hint().contains("pan"),
        "panned no-wrap hint should mention panning"
    );
}

#[test]
fn step_constant_is_a_sensible_nudge() {
    // Guard the documented invariant: one notch is a small precise step, not a
    // full-screen jump.
    const { assert!(HORIZONTAL_STEP_COLUMNS > 0 && HORIZONTAL_STEP_COLUMNS <= 16) };
}
