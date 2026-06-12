use super::*;

// ---- LogicalScroll constructors & accessors -------------------------

#[test]
fn pinned_follows_tail_at_top_zero() {
    let s = LogicalScroll::pinned();
    assert!(s.is_following());
    assert_eq!(s.raw_top(), 0);
}

#[test]
fn default_is_pinned() {
    assert_eq!(LogicalScroll::default(), LogicalScroll::pinned());
}

#[test]
fn at_top_offset_is_unpinned_even_at_zero() {
    // The overlay's "scrolled to row 0 (top)" must stay distinct from "follow
    // the tail" — `at_top_offset(0)` does NOT auto-pin.
    let s = LogicalScroll::at_top_offset(0);
    assert!(!s.is_following());
    assert_eq!(s.raw_top(), 0);

    let s = LogicalScroll::at_top_offset(7);
    assert!(!s.is_following());
    assert_eq!(s.raw_top(), 7);
}

// ---- max_top_offset --------------------------------------------------

#[test]
fn max_top_offset_saturates_when_content_fits() {
    assert_eq!(max_top_offset(0, 24), 0);
    assert_eq!(max_top_offset(24, 24), 0);
    assert_eq!(max_top_offset(10, 24), 0);
}

#[test]
fn max_top_offset_is_overflow_above_viewport() {
    assert_eq!(max_top_offset(100, 24), 76);
    assert_eq!(max_top_offset(25, 24), 1);
}

// ---- resolve_top_offset (render-boundary conversion) ----------------

/// Reference implementation: the EXACT logic the overlay's
/// `resolved_transcript_overlay_scroll_for_state` used before §11G.11, where the
/// overlay stored an absolute top-line `scroll` with `usize::MAX` meaning "snap
/// to the bottom". `resolve_top_offset` must agree with it for every input.
fn reference_overlay_resolve(stored: usize, content_len: usize, viewport_h: usize) -> usize {
    const SENTINEL: usize = usize::MAX;
    let max_scroll = content_len.saturating_sub(viewport_h);
    if stored == SENTINEL {
        max_scroll
    } else {
        stored.min(max_scroll)
    }
}

fn logical_from_stored(stored: usize) -> LogicalScroll {
    if stored == usize::MAX {
        LogicalScroll::pinned()
    } else {
        LogicalScroll::at_top_offset(stored)
    }
}

#[test]
fn resolve_pinned_shows_the_tail() {
    let s = LogicalScroll::pinned();
    assert_eq!(resolve_top_offset(s, 100, 24), 76);
    assert_eq!(resolve_top_offset(s, 24, 24), 0);
    assert_eq!(resolve_top_offset(s, 0, 24), 0);
}

#[test]
fn resolve_top_offset_clamps_overscroll_to_max() {
    let s = LogicalScroll::at_top_offset(1000);
    assert_eq!(resolve_top_offset(s, 100, 24), 76, "clamped to max_scroll");
    let s = LogicalScroll::at_top_offset(10);
    assert_eq!(resolve_top_offset(s, 100, 24), 10, "in-range stays put");
}

#[test]
fn resolve_top_offset_zero_viewport_is_max_scroll() {
    // Degenerate viewport: max_scroll == content_len, pinned shows it all off.
    let s = LogicalScroll::pinned();
    assert_eq!(resolve_top_offset(s, 5, 0), 5);
}

#[test]
fn resolve_matches_legacy_overlay_for_every_input() {
    // Exhaustive cross-check against the pre-unification overlay math over a
    // representative grid, including the `usize::MAX` sentinel.
    let stored_cases = [0usize, 1, 5, 10, 75, 76, 77, 1000, usize::MAX];
    for &content in &[0usize, 1, 24, 25, 100, 200] {
        for &viewport in &[0usize, 1, 24, 50] {
            for &stored in &stored_cases {
                let got = resolve_top_offset(logical_from_stored(stored), content, viewport);
                let want = reference_overlay_resolve(stored, content, viewport);
                assert_eq!(
                    got, want,
                    "stored={stored} content={content} viewport={viewport}"
                );
            }
        }
    }
}

// ---- thumb_geometry --------------------------------------------------

#[test]
fn thumb_geometry_none_when_content_fits() {
    assert!(thumb_geometry(24, 24, 0).is_none());
    assert!(thumb_geometry(10, 24, 0).is_none());
}

#[test]
fn thumb_geometry_none_for_zero_track() {
    assert!(thumb_geometry(100, 0, 0).is_none());
}

#[test]
fn thumb_at_top_sits_at_track_top() {
    let g = thumb_geometry(100, 24, 0).expect("scrollbar present");
    assert_eq!(g.thumb_offset, 0, "scroll 0 puts the thumb at the top");
    assert_eq!(g.max_scroll, 76);
    assert!(g.thumb_len >= 1 && g.thumb_len <= 24);
}

#[test]
fn thumb_at_bottom_sits_at_track_bottom() {
    let g = thumb_geometry(100, 24, 76).expect("scrollbar present");
    // Bottom of travel: thumb_offset + thumb_len == track height.
    assert_eq!(g.thumb_offset + g.thumb_len, 24);
}

/// Reference implementation: the EXACT thumb math both surfaces used before the
/// unification (`scroll * travel / max_scroll`, `usize`). `thumb_geometry` must
/// agree with it for in-range row counts.
fn reference_thumb(content_len: usize, track: usize, scroll: usize) -> Option<(usize, usize)> {
    if track == 0 || content_len <= track {
        return None;
    }
    let max_scroll = content_len.saturating_sub(track);
    if max_scroll == 0 {
        return None;
    }
    let thumb_len = ((track * track) / content_len).clamp(1, track);
    let travel = track.saturating_sub(thumb_len);
    let scroll = scroll.min(max_scroll);
    let thumb_offset = if travel == 0 {
        0
    } else {
        scroll * travel / max_scroll
    };
    Some((thumb_offset, thumb_len))
}

#[test]
fn thumb_geometry_matches_legacy_for_every_input() {
    for &content in &[25usize, 50, 100, 999] {
        for &track in &[1usize, 5, 24, 40] {
            for scroll in [0usize, 1, 7, 25, content.saturating_sub(track), content + 5] {
                let got = thumb_geometry(content, track, scroll);
                let want = reference_thumb(content, track, scroll);
                match (got, want) {
                    (Some(g), Some((off, len))) => {
                        assert_eq!(
                            (g.thumb_offset, g.thumb_len),
                            (off, len),
                            "content={content} track={track} scroll={scroll}"
                        );
                    }
                    (None, None) => {}
                    (g, w) => panic!(
                        "presence mismatch content={content} track={track} scroll={scroll}: {g:?} vs {w:?}"
                    ),
                }
            }
        }
    }
}

#[test]
fn thumb_geometry_no_overflow_for_huge_content() {
    // The whole point of the usize migration: row counts past the u16 ceiling
    // must not panic or overflow. The u128-widened product handles it.
    let content = 5_000_000usize;
    let track = 40usize;
    let max_scroll = content - track;
    let g = thumb_geometry(content, track, max_scroll).expect("scrollbar present");
    assert!(g.thumb_offset + g.thumb_len <= track);
    assert_eq!(g.max_scroll, max_scroll);
}
