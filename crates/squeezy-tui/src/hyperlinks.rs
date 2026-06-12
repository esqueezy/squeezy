//! OSC 8 hyperlinks (§11.5 / backlog 11G.5): make URLs and absolute file paths
//! in rendered transcript text click-to-open in capable terminals, with a
//! plain-text fallback everywhere else.
//!
//! ## What OSC 8 is
//!
//! OSC 8 is the terminal escape that turns a run of cells into a hyperlink:
//!
//! ```text
//! ESC ] 8 ; ; <uri> ST   <visible text>   ESC ] 8 ; ; ST
//! ```
//!
//! where `ST` (string terminator) is `ESC \`. A terminal that understands it
//! underlines the run and opens `<uri>` on click/Cmd-click; one that doesn't
//! ignores the escapes and shows the bare text. So the escape is *additive*:
//! the visible glyphs are identical either way — only the click affordance and
//! (on capable terminals) the link underline differ.
//!
//! ## Why a pure module
//!
//! This module owns three pure pieces and nothing about rendering, input, or
//! geometry:
//!
//!   1. **Capability probe** — [`detect_hyperlink_capabilities_from_env`], an
//!      env-closure heuristic mirroring `clipboard::detect_clipboard_capabilities_from_env`
//!      so production threads `std::env::var_os` while tests pass a fixture map.
//!      A terminal not on the known-good list emits plain text, never escapes.
//!   2. **Link detection** — [`find_links`] scans a single row of plain text for
//!      URL runs (`http`/`https`/`file` schemes) and absolute file paths,
//!      returning byte-offset spans paired with the `uri` each should open. It is
//!      conservative: only well-anchored, unambiguous runs match, so a stray
//!      `/` or a bare word is never mis-linked.
//!   3. **Escape encoding** — [`open_sequence`]/[`CLOSE_SEQUENCE`] produce the
//!      OSC 8 open/close bytes, sanitising the uri so a control byte in the text
//!      can never break out of the escape.
//!
//! `lib.rs` owns one [`HyperlinkCapabilities`] (detected at startup, like the
//! clipboard caps), a runtime toggle the user can flip from a keybinding when a
//! mis-detected terminal mangles the escapes, and calls [`find_links`] +
//! [`open_sequence`] from the row emitter that writes the exit-mirror transcript
//! into native scrollback — the one render path where a persisted, clickable URL
//! actually helps. Keeping the detection/encoding here means it is exhaustively
//! unit-testable without a terminal, and the emitter stays a thin consumer.

use std::ffi::OsString;

/// The OSC 8 link *close* sequence: an empty-uri OSC 8 (`ESC ] 8 ; ; ST`). The
/// same bytes always end a link run, so it is a constant rather than a builder.
pub(crate) const CLOSE_SEQUENCE: &str = "\u{1b}]8;;\u{1b}\\";

/// Terminal hyperlink (OSC 8) capabilities resolved from the environment.
///
/// A single `osc8` flag today, kept as a struct (not a bare `bool`) to match the
/// [`crate::clipboard::ClipboardCapabilities`] shape and leave room for future
/// link-related signals (e.g. a per-scheme allowlist) without a churny
/// signature change at every call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct HyperlinkCapabilities {
    /// Whether the host terminal is believed to honour OSC 8 hyperlink escapes.
    /// When `false` the emitter writes the bare visible text and never emits an
    /// open/close pair — the safe default for an unknown terminal.
    pub osc8: bool,
}

impl HyperlinkCapabilities {
    /// A capabilities value with OSC 8 forced on, for the runtime "force links"
    /// override (a user on a capable terminal the heuristic missed).
    pub(crate) fn enabled() -> Self {
        Self { osc8: true }
    }

    /// A capabilities value with OSC 8 forced off, for the runtime "plain text"
    /// override (a user whose terminal mangles the escapes).
    pub(crate) fn disabled() -> Self {
        Self { osc8: false }
    }
}

/// Pure capability heuristic for OSC 8 hyperlink support based on
/// environment-variable signals exposed by the host terminal.
///
/// Mirrors `clipboard::detect_clipboard_capabilities_from_env`: factored out so
/// production threads [`std::env::var_os`] while tests pass a fixture-backed
/// closure, exercising the resolver without mutating real process env.
///
/// The known-good list is the family of terminals that have shipped OSC 8
/// support for years (kitty, WezTerm, Ghostty, iTerm2, VS Code's integrated
/// terminal), plus tmux (which forwards OSC 8 to the outer terminal when it is
/// itself capable). Everything else is treated as incapable and gets plain
/// text, which is always safe.
pub(crate) fn detect_hyperlink_capabilities_from_env<F>(env_get: F) -> HyperlinkCapabilities
where
    F: Fn(&str) -> Option<OsString>,
{
    HyperlinkCapabilities {
        osc8: detect_osc8_from_env(&env_get),
    }
}

fn detect_osc8_from_env<F>(env_get: &F) -> bool
where
    F: Fn(&str) -> Option<OsString>,
{
    // Per-emulator marker env vars are the most reliable signal (set by the
    // emulator itself, not spoofable by a wrapper the way TERM is).
    if env_get("KITTY_WINDOW_ID").is_some()
        || env_get("WEZTERM_PANE").is_some()
        || env_get("WEZTERM_EXECUTABLE").is_some()
        || env_get("GHOSTTY_RESOURCES_DIR").is_some()
        || env_get("ITERM_SESSION_ID").is_some()
        || env_get("VTE_VERSION").is_some()
    {
        return true;
    }
    if let Some(prog) = env_get("TERM_PROGRAM") {
        let prog = prog.to_string_lossy().to_ascii_lowercase();
        if matches!(
            prog.as_str(),
            "iterm.app" | "iterm2" | "wezterm" | "ghostty" | "kitty" | "vscode" | "tmux"
        ) {
            return true;
        }
    }
    if let Some(term) = env_get("TERM") {
        let term = term.to_string_lossy().to_ascii_lowercase();
        if term.contains("kitty")
            || term.contains("wezterm")
            || term.contains("ghostty")
            || term.contains("vte")
        {
            return true;
        }
    }
    false
}

/// One detected link inside a row of plain text.
///
/// Addressed by **byte offsets** into the source `&str` (`start..end`, the
/// half-open visible run) plus the resolved `uri` the run should open. Byte
/// offsets (not char or column offsets) keep the span aligned to the source
/// slice the emitter walks; the emitter maps bytes to terminal columns as it
/// goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkSpan {
    /// Inclusive byte offset of the first character of the visible run.
    pub start: usize,
    /// Exclusive byte offset just past the last character of the visible run.
    pub end: usize,
    /// The absolute uri to open: the matched text itself for a URL, or a
    /// `file://`-prefixed absolute path for a file path.
    pub uri: String,
}

impl LinkSpan {
    /// The number of bytes the visible run spans. Test-only diagnostic; the
    /// emitter maps the `start`/`end` offsets to columns directly.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.end - self.start
    }
}

/// Find every URL and absolute-file-path run in one row of plain text, in
/// left-to-right order with no overlaps.
///
/// Detection rules, deliberately conservative so a non-link never lights up:
///   - A **URL** run starts at a scheme (`http://`, `https://`, or `file://`)
///     that is at the start of the string or preceded by a non-URL character
///     (whitespace, `(`, `<`, `[`, `"`, `'`). It runs to the first whitespace or
///     a closing delimiter, with trailing sentence punctuation (`.,;:!?` and a
///     single matched `)`/`>`/`]`) trimmed off so `(see https://x.test/a).`
///     links `https://x.test/a`, not `https://x.test/a).`. Its `uri` is the run
///     verbatim.
///   - A **file path** run is an absolute POSIX path: a `/` at a word boundary
///     followed by at least one path segment (so a bare `/` or an arithmetic
///     `a / b` never matches), running to the first whitespace, with the same
///     trailing-punctuation trim. Its `uri` is the path with a `file://` scheme
///     prepended.
///
/// Returns an empty vec when the row has no links (the overwhelmingly common
/// case), so the emitter pays nothing for a plain row beyond the scan.
pub(crate) fn find_links(text: &str) -> Vec<LinkSpan> {
    let bytes = text.as_bytes();
    let mut spans: Vec<LinkSpan> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        // Only consider a candidate at a left boundary: the string start or
        // after a separator character, so a scheme embedded mid-token (e.g.
        // `xhttps://`) is not treated as a link.
        let at_boundary = i == 0 || is_boundary_byte(bytes[i - 1]);
        if at_boundary && let Some(span) = match_url(text, i).or_else(|| match_path(text, i)) {
            i = span.end;
            spans.push(span);
            continue;
        }
        // Advance one whole UTF-8 char so we never split a multibyte boundary.
        i += utf8_char_len(bytes[i]);
    }
    spans
}

/// True when `b` is a character that can precede a link run (a left boundary).
/// Anything that is part of a normal word is NOT a boundary, so a link must be
/// preceded by whitespace or an opening bracket/quote.
fn is_boundary_byte(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t' | b'(' | b'<' | b'[' | b'{' | b'"' | b'\'' | b'=' | b':'
    )
}

/// The known URL schemes we linkify, longest first so `https` is tried before
/// `http`. Each is the literal prefix INCLUDING the `://`.
const URL_SCHEMES: &[&str] = &["https://", "http://", "file://"];

/// Try to match a URL run starting at byte offset `start`. Returns the trimmed
/// visible span (its `uri` equal to the visible text) or `None`.
fn match_url(text: &str, start: usize) -> Option<LinkSpan> {
    let rest = &text[start..];
    let scheme = URL_SCHEMES.iter().find(|s| rest.starts_with(**s))?;
    // Require at least one host/path byte after the scheme so a bare
    // `https://` with nothing after it is not a link.
    let after_scheme = start + scheme.len();
    let run_end = scan_run_end(text, after_scheme);
    if run_end <= after_scheme {
        return None;
    }
    let trimmed_end = trim_trailing(text, start, run_end);
    if trimmed_end <= after_scheme {
        return None;
    }
    let uri = text[start..trimmed_end].to_string();
    Some(LinkSpan {
        start,
        end: trimmed_end,
        uri,
    })
}

/// Try to match an absolute POSIX file-path run starting at byte offset
/// `start`. Returns a span whose `uri` is the path with `file://` prepended, or
/// `None` when the run is not a path (a bare `/`, a `//` URL-less double slash,
/// or `a / b` arithmetic).
fn match_path(text: &str, start: usize) -> Option<LinkSpan> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'/') {
        return None;
    }
    // The byte after the leading `/` must begin a real path segment: a
    // path-safe, non-slash, non-space character. This rejects a bare `/`
    // (end/space follows), `a / b` (space follows), and `//x` (slash follows).
    match bytes.get(start + 1) {
        Some(&c) if is_path_segment_byte(c) => {}
        _ => return None,
    }
    let run_end = scan_run_end(text, start);
    let trimmed_end = trim_trailing(text, start, run_end);
    // Need at least `/x`.
    if trimmed_end <= start + 1 {
        return None;
    }
    let path = &text[start..trimmed_end];
    let uri = format!("file://{path}");
    Some(LinkSpan {
        start,
        end: trimmed_end,
        uri,
    })
}

/// Scan forward from `from` to the end of an unbroken link run: stop at the
/// first ASCII whitespace or run-terminating delimiter. Returns the exclusive
/// end byte offset.
fn scan_run_end(text: &str, from: usize) -> usize {
    let bytes = text.as_bytes();
    let mut j = from;
    while j < bytes.len() {
        let b = bytes[j];
        if b.is_ascii_whitespace() || is_run_terminator(b) {
            break;
        }
        j += utf8_char_len(b);
    }
    j
}

/// True for the characters that hard-terminate a link run regardless of
/// matched-delimiter trimming: a space-like or quote/angle/control byte that
/// can never be a meaningful interior link character.
fn is_run_terminator(b: u8) -> bool {
    matches!(b, b'"' | b'\'' | b'<' | b'>' | b'`' | b'|') || b.is_ascii_control()
}

/// True when `b` can appear inside a path segment (used only to validate the
/// first segment byte). Excludes the slash and whitespace; everything else
/// printable is allowed so unusual but legal filenames still link.
fn is_path_segment_byte(b: u8) -> bool {
    !b.is_ascii_whitespace() && b != b'/' && !is_run_terminator(b)
}

/// Trim trailing sentence punctuation and a single unmatched close bracket from
/// a `start..end` run, so a link followed by `).` or a full stop links only the
/// address. `start` is needed to balance an interior open bracket: a trailing
/// `)` is kept when the run itself contains a matching `(` (e.g. a wiki URL).
fn trim_trailing(text: &str, start: usize, mut end: usize) -> usize {
    let bytes = text.as_bytes();
    while end > start {
        let last = bytes[end - 1];
        let trim = match last {
            // Sentence punctuation is never part of an address.
            b'.' | b',' | b';' | b':' | b'!' | b'?' => true,
            // A close bracket is trimmed only when it is UNMATCHED within the
            // run (no corresponding open bracket before it), so `(a)` survives
            // but `a)` does not.
            b')' => {
                count_byte(bytes, start, end - 1, b'(') <= count_byte(bytes, start, end - 1, b')')
            }
            b']' => {
                count_byte(bytes, start, end - 1, b'[') <= count_byte(bytes, start, end - 1, b']')
            }
            b'}' => {
                count_byte(bytes, start, end - 1, b'{') <= count_byte(bytes, start, end - 1, b'}')
            }
            _ => false,
        };
        if trim {
            end -= 1;
        } else {
            break;
        }
    }
    end
}

/// Count occurrences of byte `needle` in `bytes[lo..hi]`.
fn count_byte(bytes: &[u8], lo: usize, hi: usize, needle: u8) -> usize {
    bytes[lo..hi].iter().filter(|&&b| b == needle).count()
}

/// Length in bytes of the UTF-8 character whose lead byte is `b`. A safe
/// 1..=4 mapping that never returns 0, so the scan loops always make progress
/// even on a malformed continuation byte (treated as a 1-byte step).
fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

/// The OSC 8 *open* sequence for `uri`: `ESC ] 8 ; ; <uri> ST`.
///
/// The uri is sanitised: any control character is dropped — an ASCII C0 control
/// (below 0x20, including ESC itself), 0x7f (DEL), or a C1 control
/// (U+0080..=U+009F). C1 controls matter because a terminal in 8-bit mode (or
/// one that decodes the UTF-8 form `0xC2 0x9C`) treats U+009C as a String
/// Terminator and U+009B as a CSI introducer, so a C1 byte that slipped into the
/// detected run could terminate the escape early or inject a second escape just
/// as a C0 control would. The visible text is emitted separately by the caller
/// and is unaffected.
pub(crate) fn open_sequence(uri: &str) -> String {
    let mut clean = String::with_capacity(uri.len());
    for ch in uri.chars() {
        let c = ch as u32;
        if (0x20..0x7f).contains(&c) || c >= 0xa0 {
            clean.push(ch);
        }
    }
    format!("\u{1b}]8;;{clean}\u{1b}\\")
}

/// True when `text` contains any byte that [`open_sequence`] would strip from a
/// uri — a control byte (below 0x20, which includes ESC) or 0x7f. Used only by
/// tests to assert the sanitiser has something to do; production calls
/// [`open_sequence`] directly.
#[cfg(test)]
pub(crate) fn uri_has_control_bytes(text: &str) -> bool {
    text.bytes().any(|b| b < 0x20 || b == 0x7f)
}

#[cfg(test)]
#[path = "hyperlinks_tests.rs"]
mod tests;
