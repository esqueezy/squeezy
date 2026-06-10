//! Unit tests for the pure quote-to-compose text transform.
//!
//! Included into [`crate::quote_compose`] via `#[path]` per the repo test
//! layout. These pin the exact quoting/indentation rules independent of any
//! `TuiApp`; the keyboard/mouse end-to-end path is covered in `lib_tests.rs`.

use super::*;

#[test]
fn single_line_gets_a_quote_prefix_and_trailing_blank() {
    let block = quote_block("hello world").expect("non-empty");
    // `> ` prefix, then a blank line so the caret lands below the quote.
    assert_eq!(block, "> hello world\n\n");
}

#[test]
fn multi_line_quotes_every_line() {
    let block = quote_block("first\nsecond\nthird").expect("non-empty");
    assert_eq!(block, "> first\n> second\n> third\n\n");
}

#[test]
fn interior_blank_line_becomes_a_bare_marker_without_trailing_space() {
    let block = quote_block("a\n\nb").expect("non-empty");
    // The blank source line becomes a bare `">"` (no trailing whitespace), so
    // the blockquote reads cleanly.
    assert_eq!(block, "> a\n>\n> b\n\n");
    assert!(
        !block.contains("> \n"),
        "an empty quoted line must not carry a trailing space: {block:?}"
    );
}

#[test]
fn already_quoted_line_nests_one_level_deeper() {
    let block = quote_block("> existing quote").expect("non-empty");
    assert_eq!(block, "> > existing quote\n\n");
}

#[test]
fn empty_or_whitespace_only_selection_is_a_noop() {
    assert_eq!(quote_block(""), None);
    assert_eq!(quote_block("   "), None);
    assert_eq!(quote_block("\n\n  \n"), None);
}

#[test]
fn code_like_content_is_preserved_verbatim_after_the_prefix() {
    // Quote-to-compose must not reflow or strip interior content — only prefix.
    let src = "fn main() {\n    println!(\"hi\");\n}";
    let block = quote_block(src).expect("non-empty");
    assert_eq!(block, "> fn main() {\n>     println!(\"hi\");\n> }\n\n");
}

#[test]
fn quote_line_helper_rules() {
    assert_eq!(quote_line(""), ">");
    assert_eq!(quote_line("x"), "> x");
    assert_eq!(quote_line("> y"), "> > y");
    // Leading whitespace is content, not stripped.
    assert_eq!(quote_line("    indented"), ">     indented");
}
