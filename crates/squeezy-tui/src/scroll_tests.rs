use super::*;

// ---- to_u16_clamped -------------------------------------------------

#[test]
fn to_u16_clamped_passes_small_values() {
    assert_eq!(to_u16_clamped(0), 0);
    assert_eq!(to_u16_clamped(42), 42);
    assert_eq!(to_u16_clamped(u16::MAX as usize - 1), u16::MAX - 1);
}

#[test]
fn to_u16_clamped_saturates_at_max() {
    assert_eq!(to_u16_clamped(u16::MAX as usize), u16::MAX);
    assert_eq!(to_u16_clamped(u16::MAX as usize + 1), u16::MAX);
    assert_eq!(to_u16_clamped(1_000_000), u16::MAX);
    assert_eq!(to_u16_clamped(usize::MAX), u16::MAX);
}

// ---- ScrollState::offset (mirrors transcript_scroll_offset) ----------

/// Reference implementation: the exact logic of `transcript_scroll_offset`
/// in lib.rs, used to cross-check the usize model over a u16 range.
fn reference_offset(line_count: usize, area_height: u16, from_bottom: u16) -> u16 {
    let visible_lines = area_height as usize;
    let max_scroll = line_count.saturating_sub(visible_lines);
    max_scroll.saturating_sub(from_bottom as usize) as u16
}

#[test]
fn offset_empty_buffer_is_zero() {
    let s = ScrollState::pinned();
    assert_eq!(s.offset(0, 0), 0);
    assert_eq!(s.offset(0, 24), 0);
}

#[test]
fn offset_exact_fit_is_zero() {
    // Content exactly fills the viewport: nothing to scroll, offset is 0.
    let s = ScrollState::pinned();
    assert_eq!(s.offset(24, 24), 0);
    let mut up = ScrollState::pinned();
    up.scroll_by(5, 24, 24);
    assert_eq!(up.offset(24, 24), 0);
}

#[test]
fn offset_overflow_tail_shows_bottom() {
    // 100 lines, 24-row viewport => max_scroll 76. Pinned (from_bottom 0)
    // shows the tail at offset 76.
    let s = ScrollState::pinned();
    assert_eq!(s.offset(100, 24), 76);
}

#[test]
fn offset_scrolled_up_subtracts_from_max() {
    let mut s = ScrollState::pinned();
    s.scroll_by(10, 100, 24); // from_bottom = 10
    assert_eq!(s.offset(100, 24), 66);
}

#[test]
fn offset_scrolled_past_top_clamps_to_zero() {
    let mut s = ScrollState::pinned();
    s.scroll_by(1000, 100, 24); // clamped to max_scroll = 76
    assert_eq!(s.from_bottom(), 76);
    assert_eq!(s.offset(100, 24), 0);
}

#[test]
fn offset_matches_reference_over_u16_range() {
    let cases = [
        (0usize, 0u16, 0u16),
        (0, 24, 0),
        (24, 24, 0),
        (24, 24, 5),
        (100, 24, 0),
        (100, 24, 10),
        (100, 24, 76),
        (100, 24, 1000),
        (65_000, 80, 0),
        (65_000, 80, 100),
        (1, 1, 0),
    ];
    for (line_count, area_h, from_bottom) in cases {
        let s = ScrollState {
            from_bottom: from_bottom as usize,
            follow_tail: from_bottom == 0,
        };
        let got = to_u16_clamped(s.offset(line_count, area_h as usize));
        let want = reference_offset(line_count, area_h, from_bottom);
        assert_eq!(got, want, "case {line_count}/{area_h}/{from_bottom}");
    }
}

// ---- clamp ----------------------------------------------------------

#[test]
fn clamp_following_re_pins_to_zero() {
    let mut s = ScrollState {
        from_bottom: 50,
        follow_tail: true,
    };
    let changed = s.clamp(100, 24);
    assert!(changed);
    assert_eq!(s.from_bottom(), 0);
    assert!(s.is_following());
}

#[test]
fn clamp_caps_unpinned_to_max_scroll() {
    let mut s = ScrollState {
        from_bottom: 500,
        follow_tail: false,
    };
    // max_scroll = 100 - 24 = 76
    let changed = s.clamp(100, 24);
    assert!(changed);
    assert_eq!(s.from_bottom(), 76);
    assert!(!s.is_following());
}

#[test]
fn clamp_noop_returns_false() {
    let mut s = ScrollState {
        from_bottom: 10,
        follow_tail: false,
    };
    let changed = s.clamp(100, 24);
    assert!(!changed);
    assert_eq!(s.from_bottom(), 10);
}

#[test]
fn clamp_when_content_fits_drops_to_zero() {
    let mut s = ScrollState {
        from_bottom: 5,
        follow_tail: false,
    };
    // Content shrank to fit: max_scroll = 0.
    let changed = s.clamp(20, 24);
    assert!(changed);
    assert_eq!(s.from_bottom(), 0);
}

// ---- scroll_by / follow-tail pin & unpin ----------------------------

#[test]
fn scroll_up_unpins() {
    let mut s = ScrollState::pinned();
    assert!(s.is_following());
    s.scroll_by(3, 100, 24);
    assert_eq!(s.from_bottom(), 3);
    assert!(!s.is_following());
}

#[test]
fn scroll_down_to_tail_re_pins() {
    let mut s = ScrollState::pinned();
    s.scroll_by(10, 100, 24);
    assert!(!s.is_following());
    s.scroll_by(-10, 100, 24);
    assert_eq!(s.from_bottom(), 0);
    assert!(s.is_following());
}

#[test]
fn scroll_down_past_tail_saturates_and_pins() {
    let mut s = ScrollState::pinned();
    s.scroll_by(5, 100, 24);
    s.scroll_by(-100, 100, 24);
    assert_eq!(s.from_bottom(), 0);
    assert!(s.is_following());
}

#[test]
fn scroll_up_past_top_clamps_to_max_scroll() {
    let mut s = ScrollState::pinned();
    s.scroll_by(10_000, 100, 24);
    assert_eq!(s.from_bottom(), 76); // max_scroll
    assert!(!s.is_following());
}

#[test]
fn scroll_partial_down_stays_unpinned() {
    let mut s = ScrollState::pinned();
    s.scroll_by(10, 100, 24);
    s.scroll_by(-4, 100, 24);
    assert_eq!(s.from_bottom(), 6);
    assert!(!s.is_following());
}

#[test]
fn pin_to_bottom_resets() {
    let mut s = ScrollState {
        from_bottom: 40,
        follow_tail: false,
    };
    s.pin_to_bottom();
    assert_eq!(s.from_bottom(), 0);
    assert!(s.is_following());
}

#[test]
fn scroll_when_content_fits_is_noop() {
    let mut s = ScrollState::pinned();
    s.scroll_by(50, 10, 24); // max_scroll = 0
    assert_eq!(s.from_bottom(), 0);
    assert!(s.is_following());
}

// ---- Phase 4: page-sized scrolls land exactly --------------------------

#[test]
fn page_up_then_page_down_is_a_round_trip() {
    // PageUp/PageDown are `scroll_by(±8)` at the app layer; the model must move
    // by exactly the page and come back to the same spot.
    let mut s = ScrollState::pinned();
    s.scroll_by(8, 100, 24);
    assert_eq!(s.from_bottom(), 8);
    assert!(!s.is_following(), "a page up unpins follow-tail");
    s.scroll_by(8, 100, 24);
    assert_eq!(
        s.from_bottom(),
        16,
        "a second page lands exactly 8 further up"
    );
    s.scroll_by(-8, 100, 24);
    assert_eq!(s.from_bottom(), 8);
    s.scroll_by(-8, 100, 24);
    assert_eq!(s.from_bottom(), 0, "paging back down to the tail re-pins");
    assert!(s.is_following());
}

#[test]
fn home_then_end_round_trips_top_to_tail() {
    // Home == scroll_to_top, End == pin_to_bottom. The pair must visit the real
    // extremes and leave follow-tail set only at the tail.
    let mut s = ScrollState::pinned();
    s.scroll_to_top(100, 24);
    assert_eq!(
        s.from_bottom(),
        76,
        "Home lands on the real max_scroll, not MAX"
    );
    assert!(!s.is_following());
    assert_eq!(s.offset(100, 24), 0, "top of content renders at offset 0");
    s.pin_to_bottom();
    assert_eq!(s.from_bottom(), 0);
    assert!(s.is_following(), "End re-pins to the live tail");
    assert_eq!(s.offset(100, 24), 76, "tail renders the last viewport");
}

// ---- set_from_bottom (scrollbar click / jump-nav target) ---------------

#[test]
fn set_from_bottom_clamps_and_repins_at_tail() {
    let mut s = ScrollState::scrolled_up(40);
    // A mid-content target: stored verbatim, unpinned.
    s.set_from_bottom(30, 100, 24);
    assert_eq!(s.from_bottom(), 30);
    assert!(!s.is_following());
    // Past the top: clamped to max_scroll (76), still unpinned.
    s.set_from_bottom(1000, 100, 24);
    assert_eq!(s.from_bottom(), 76);
    assert!(!s.is_following());
    // Exactly the tail: re-pins follow-tail.
    s.set_from_bottom(0, 100, 24);
    assert_eq!(s.from_bottom(), 0);
    assert!(s.is_following());
}

// ---- Phase 4: resize keeps the SAME logical content (model level) ------

#[test]
fn clamp_on_shrink_preserves_scrolled_up_anchor_when_in_range() {
    // Scrolled up 20 from the tail over 200 lines. Shrinking the viewport keeps
    // max_scroll well above 20, so the logical anchor (from_bottom) is unchanged
    // — the same content stays anchored at the bottom of the viewport.
    let mut s = ScrollState::scrolled_up(20);
    let changed = s.clamp(200, 10); // max_scroll = 190
    assert!(!changed, "an in-range anchor survives the reflow unchanged");
    assert_eq!(s.from_bottom(), 20);
    assert!(!s.is_following());
}

#[test]
fn resize_following_view_stays_at_latest() {
    // A following view stays pinned across both a shrink and a grow: offset
    // always resolves to the tail (max_scroll) for the new geometry.
    let mut s = ScrollState::pinned();
    s.clamp(200, 10);
    assert_eq!(s.from_bottom(), 0);
    assert_eq!(s.offset(200, 10), 190, "tail for the 10-row viewport");
    s.clamp(200, 50);
    assert_eq!(s.from_bottom(), 0);
    assert_eq!(s.offset(200, 50), 150, "tail for the 50-row viewport");
    assert!(s.is_following());
}

#[test]
fn resize_scrolled_up_keeps_same_bottom_line_in_view() {
    // The user is scrolled up so the viewport's BOTTOM shows content line
    // `total - 1 - from_bottom`. Across a viewport-height change (while the
    // anchor stays in range) that same content line must remain the bottom of
    // the viewport — that's what "same logical content visible" means here.
    let total = 300usize;
    let from_bottom = 50usize;
    let s = ScrollState::scrolled_up(from_bottom);
    // bottom_line = top_offset + viewport_h - 1 = (max_scroll - from_bottom) + h - 1
    //            = (total - h - from_bottom) + h - 1 = total - 1 - from_bottom.
    let bottom_line = |h: usize| s.offset(total, h) + h - 1;
    let want = total - 1 - from_bottom;
    assert_eq!(bottom_line(24), want);
    assert_eq!(bottom_line(12), want, "shrink keeps the same bottom line");
    assert_eq!(bottom_line(40), want, "grow keeps the same bottom line");
}

// ---- scrollbar_geometry (mirrors overlay geometry) ------------------

/// Reference: the exact thumb math from
/// `transcript_overlay_scrollbar_geometry`, taking a top-line `scroll`.
fn reference_geometry(
    content_len: usize,
    viewport_height: u16,
    scroll: usize,
) -> Option<(u16, u16)> {
    let track_height = usize::from(viewport_height);
    if track_height == 0 || content_len <= track_height {
        return None;
    }
    let max_scroll = content_len.saturating_sub(track_height);
    if max_scroll == 0 {
        return None;
    }
    let thumb_height = ((track_height * track_height) / content_len).clamp(1, track_height);
    let travel = track_height.saturating_sub(thumb_height);
    let scroll = scroll.min(max_scroll);
    let thumb_top = if travel == 0 {
        0
    } else {
        scroll * travel / max_scroll
    };
    Some((thumb_top as u16, thumb_height as u16))
}

#[test]
fn geometry_none_for_empty_buffer() {
    assert_eq!(scrollbar_geometry(0, 24, 0), None);
    assert_eq!(scrollbar_geometry(0, 0, 0), None);
}

#[test]
fn geometry_none_when_content_fits() {
    assert_eq!(scrollbar_geometry(24, 24, 0), None);
    assert_eq!(scrollbar_geometry(10, 24, 0), None);
}

#[test]
fn geometry_none_for_zero_height_track() {
    assert_eq!(scrollbar_geometry(100, 0, 0), None);
}

#[test]
fn geometry_thumb_len_is_proportional() {
    // 100 rows, 24-row track => 24*24/100 = 5.
    let g = scrollbar_geometry(100, 24, 0).unwrap();
    assert_eq!(g.thumb_len, 5);
}

#[test]
fn geometry_thumb_len_clamped_to_minimum_one() {
    // Huge content, small track => proportional thumb rounds to 0, clamped to 1.
    let g = scrollbar_geometry(1_000_000, 2, 0).unwrap();
    assert_eq!(g.thumb_len, 1);
}

#[test]
fn geometry_tail_places_thumb_at_bottom_of_travel() {
    // from_bottom == 0 (tail) => scroll == max_scroll => thumb at end of travel.
    let g = scrollbar_geometry(100, 24, 0).unwrap();
    let travel = 24 - g.thumb_len;
    assert_eq!(g.thumb_offset, travel);
}

#[test]
fn geometry_top_places_thumb_at_offset_zero() {
    // Scrolled fully up: from_bottom == max_scroll => scroll 0 => thumb at top.
    let max_scroll = 100 - 24;
    let g = scrollbar_geometry(100, 24, max_scroll).unwrap();
    assert_eq!(g.thumb_offset, 0);
}

#[test]
fn geometry_from_bottom_beyond_max_clamps() {
    let beyond = scrollbar_geometry(100, 24, 10_000).unwrap();
    let at_top = scrollbar_geometry(100, 24, 100 - 24).unwrap();
    assert_eq!(beyond, at_top);
}

#[test]
fn geometry_matches_reference_via_offset() {
    // For each from_bottom, our geometry must equal the reference geometry
    // fed the equivalent top-line scroll (max_scroll - from_bottom).
    let content = 100usize;
    let viewport = 24u16;
    let max_scroll = content - viewport as usize;
    for from_bottom in [0usize, 1, 10, 40, 76, 200] {
        let scroll = max_scroll.saturating_sub(from_bottom.min(max_scroll));
        let got = scrollbar_geometry(content, viewport as usize, from_bottom)
            .map(|g| (to_u16_clamped(g.thumb_offset), to_u16_clamped(g.thumb_len)));
        let want = reference_geometry(content, viewport, scroll);
        assert_eq!(got, want, "from_bottom={from_bottom}");
    }
}

// ---- >65k rows (the reason for the usize migration) -----------------

#[test]
fn offset_beyond_u16_range() {
    let line_count = 100_000usize;
    let viewport = 50usize;
    let s = ScrollState::pinned();
    // Tail offset = 100_000 - 50 = 99_950, which exceeds u16::MAX.
    assert_eq!(s.offset(line_count, viewport), 99_950);
    assert!(s.offset(line_count, viewport) > u16::MAX as usize);
}

#[test]
fn scroll_by_beyond_u16_range() {
    let line_count = 100_000usize;
    let viewport = 50usize;
    let mut s = ScrollState::pinned();
    s.scroll_by(70_000, line_count, viewport);
    assert_eq!(s.from_bottom(), 70_000);
    assert!(s.from_bottom() > u16::MAX as usize);
    // Offset = max_scroll(99_950) - 70_000 = 29_950.
    assert_eq!(s.offset(line_count, viewport), 29_950);
}

#[test]
fn geometry_beyond_u16_rows_thumb_clamps_to_one() {
    let g = scrollbar_geometry(100_000, 40, 0).unwrap();
    // 40*40/100_000 = 0 -> clamped to 1.
    assert_eq!(g.thumb_len, 1);
    // Tail: thumb at bottom of travel.
    assert_eq!(g.thumb_offset, 40 - 1);
}

#[test]
fn geometry_thumb_offset_can_exceed_u16_only_after_clamp() {
    // thumb_offset is bounded by the track height, so it never exceeds u16,
    // even with enormous content. Sanity check that invariant.
    let g = scrollbar_geometry(usize::MAX / 2, 30_000, 0).unwrap();
    assert!(g.thumb_offset <= 30_000);
    assert!(g.thumb_len >= 1 && g.thumb_len <= 30_000);
}
