use super::*;

#[test]
fn fixed_size_returns_scripted_dimensions() {
    let src = FixedSize(120, 40);
    assert_eq!(src.size().unwrap(), (120, 40));
}

#[test]
fn fixed_size_preserves_cols_rows_order() {
    // First field is columns (width), second is rows (height);
    // this guards against an accidental swap when callers migrate
    // from `let (width, height) = ...`.
    let (cols, rows) = FixedSize(80, 24).size().unwrap();
    assert_eq!(cols, 80, "first field must be columns (width)");
    assert_eq!(rows, 24, "second field must be rows (height)");
}

#[test]
fn fixed_size_allows_zero_dimensions() {
    // The append-only renderer treats a zero dimension as a no-op
    // frame; the seam must be able to reproduce that input exactly.
    assert_eq!(FixedSize(0, 0).size().unwrap(), (0, 0));
    assert_eq!(FixedSize(0, 30).size().unwrap(), (0, 30));
    assert_eq!(FixedSize(100, 0).size().unwrap(), (100, 0));
}

#[test]
fn fixed_size_is_copy_and_repeatable() {
    let src = FixedSize(200, 50);
    let copy = src;
    // Copy semantics: the original is still usable after the copy.
    assert_eq!(src.size().unwrap(), (200, 50));
    assert_eq!(copy.size().unwrap(), (200, 50));
    // Repeated reads are stable (no internal scripting/consumption).
    assert_eq!(src.size().unwrap(), src.size().unwrap());
}

#[test]
fn real_size_size_matches_crossterm_directly() {
    // In a headless test environment `crossterm::terminal::size`
    // may error (no tty); whichever way it resolves, `RealSize`
    // must agree byte-for-byte with the direct call it delegates to.
    match crossterm::terminal::size() {
        Ok(direct) => assert_eq!(RealSize.size().unwrap(), direct),
        Err(_) => assert!(RealSize.size().is_err()),
    }
}
