use super::*;

/// Char index of the caret (the first span carrying `Modifier::REVERSED`)
/// within the joined text of a line, counting chars in the spans before it.
/// Returns `None` when no span is reversed.
fn caret_char_index(line: &Line<'_>) -> Option<usize> {
    let mut offset = 0usize;
    for span in &line.spans {
        if span.style.add_modifier.contains(Modifier::REVERSED) {
            return Some(offset);
        }
        offset += span.content.chars().count();
    }
    None
}

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn caret_line_marks_cursor_mid_string_not_end() {
    // Two leading indent spaces then "hello"; cursor parked on index 2 ('l').
    let line = caret_line("hello", 2);
    // Indent is two chars, so the caret should land at joined index 4.
    assert_eq!(caret_char_index(&line), Some(4));
    assert_eq!(line_text(&line), "  hello");
}

#[test]
fn caret_line_at_end_reverses_trailing_space() {
    let line = caret_line("hi", 2);
    // Caret sits just past the last char: joined index 4 (2 indent + "hi").
    assert_eq!(caret_char_index(&line), Some(4));
    // A trailing space is drawn so the insertion point is visible.
    assert_eq!(line_text(&line), "  hi ");
}

#[test]
fn caret_line_handles_multibyte_chars() {
    // Cursor at char index 1 must split on a char boundary, not a byte one.
    let line = caret_line("café", 1);
    assert_eq!(caret_char_index(&line), Some(3));
    assert_eq!(line_text(&line), "  café");
}

#[test]
fn caret_line_clamps_out_of_range_cursor() {
    // Stale cursor past the end must not panic and parks on a trailing space.
    let line = caret_line("ab", 99);
    assert_eq!(line_text(&line), "  ab ");
    assert_eq!(caret_char_index(&line), Some(4));
}

#[test]
fn secret_caret_line_marks_cursor_mid_string() {
    // Masked display of a 5-char key, cursor on index 2.
    let line = secret_caret_line("•••••", 2);
    assert_eq!(caret_char_index(&line), Some(4));
}

#[test]
fn secret_caret_line_at_end_uses_underscore() {
    let line = secret_caret_line("•••", 3);
    // No reversed span when parked past the end; an accent underscore marks it.
    assert_eq!(caret_char_index(&line), None);
    assert_eq!(line_text(&line), "  •••_");
}
