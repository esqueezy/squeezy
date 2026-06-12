//! Quote-to-compose (§11.1): turn the active visual SELECTION into a quoted
//! Markdown block dropped into the composer.
//!
//! This module owns only the *pure* text transform — given the clean text of a
//! selection (already gutter-stripped and ANSI-free by
//! [`crate::selection::selection_clean_text`]), it produces the block of
//! `> `-prefixed lines plus a trailing blank line that the composer inserts so
//! the user's caret lands ready to reply *below* the quote. The crate root
//! (`lib.rs`) owns the side effects: reading the live selection, calling
//! [`crate::input::insert_input_text`], setting the status line, and clearing
//! the selection. Keeping the transform here lets the unit tests pin the exact
//! quoting/indentation rules without standing up a `TuiApp`.
//!
//! ## Quoting rules
//!
//! * Each source line becomes `"> " + line`. A line that is already empty
//!   becomes a bare `">"` (no trailing space) so the quote reads as a clean
//!   Markdown blockquote rather than carrying trailing whitespace.
//! * A line that is *itself* already a quote (`>`-prefixed) is nested one level
//!   deeper (`> > …`), the conventional Markdown behaviour for quoting a quote.
//! * The block ends with `"\n\n"` so that, when inserted at the end of an empty
//!   composer, the caret sits on a fresh blank line under the quote and the
//!   blockquote is terminated by the blank line Markdown requires.
//!
//! The transform never trims interior content; the only normalisation is the
//! trailing-whitespace drop that [`crate::selection::selection_clean_text`]
//! already applies before the text reaches here.

/// Build the composer payload for quoting `selected` text.
///
/// Returns `None` when `selected` has no non-whitespace content — there is
/// nothing meaningful to quote, and the caller should treat that as a no-op so
/// the `>` keystroke falls through to normal composer input.
///
/// On `Some`, the returned string is the full block to insert at the composer
/// caret: one `> `-prefixed line per source line, terminated by a blank line so
/// the caret lands below the quote ready to type a reply.
pub(crate) fn quote_block(selected: &str) -> Option<String> {
    if selected.trim().is_empty() {
        return None;
    }
    let mut out = String::new();
    for line in selected.lines() {
        out.push_str(&quote_line(line));
        out.push('\n');
    }
    // Trailing blank line: terminates the Markdown blockquote and parks the
    // caret on a fresh line under it.
    out.push('\n');
    Some(out)
}

/// Quote a single line. An empty line becomes a bare `">"`; an already-quoted
/// line is nested one level deeper (`"> "` is prepended unconditionally, which
/// turns `"> x"` into `"> > x"`); every other line is prefixed with `"> "`.
fn quote_line(line: &str) -> String {
    if line.is_empty() {
        return ">".to_string();
    }
    format!("> {line}")
}

#[cfg(test)]
#[path = "quote_compose_tests.rs"]
mod tests;
