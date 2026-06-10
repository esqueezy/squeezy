use std::collections::HashMap;
use std::ffi::OsString;

use super::*;

/// Build an env-closure over a fixed `name -> value` map, mirroring the
/// fixture-backed closure style the clipboard capability tests use. The map is
/// owned by the returned closure, so it borrows nothing from `pairs`.
fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |name: &str| map.get(name).map(OsString::from)
}

// ---------------------------------------------------------------------------
// Capability probe
// ---------------------------------------------------------------------------

#[test]
fn unknown_terminal_reports_no_osc8() {
    let env = env_from(&[("TERM", "xterm-256color")]);
    let caps = detect_hyperlink_capabilities_from_env(env);
    assert!(
        !caps.osc8,
        "a bare xterm is treated as incapable (plain-text fallback)"
    );
}

#[test]
fn empty_env_reports_no_osc8() {
    let caps = detect_hyperlink_capabilities_from_env(env_from(&[]));
    assert!(!caps.osc8, "no signals at all means plain-text fallback");
}

#[test]
fn kitty_marker_env_enables_osc8() {
    let caps = detect_hyperlink_capabilities_from_env(env_from(&[("KITTY_WINDOW_ID", "1")]));
    assert!(caps.osc8);
}

#[test]
fn wezterm_and_ghostty_markers_enable_osc8() {
    assert!(detect_hyperlink_capabilities_from_env(env_from(&[("WEZTERM_PANE", "0")])).osc8);
    assert!(
        detect_hyperlink_capabilities_from_env(env_from(&[("GHOSTTY_RESOURCES_DIR", "/x")])).osc8
    );
}

#[test]
fn term_program_vscode_enables_osc8() {
    let caps = detect_hyperlink_capabilities_from_env(env_from(&[("TERM_PROGRAM", "vscode")]));
    assert!(caps.osc8, "VS Code's integrated terminal supports OSC 8");
}

#[test]
fn term_program_is_case_insensitive() {
    let caps = detect_hyperlink_capabilities_from_env(env_from(&[("TERM_PROGRAM", "iTerm.app")]));
    assert!(caps.osc8);
}

#[test]
fn term_substring_kitty_enables_osc8() {
    let caps = detect_hyperlink_capabilities_from_env(env_from(&[("TERM", "xterm-kitty")]));
    assert!(caps.osc8);
}

#[test]
fn enabled_and_disabled_constructors_are_explicit_overrides() {
    assert!(HyperlinkCapabilities::enabled().osc8);
    assert!(!HyperlinkCapabilities::disabled().osc8);
    assert!(!HyperlinkCapabilities::default().osc8);
}

// ---------------------------------------------------------------------------
// URL detection
// ---------------------------------------------------------------------------

#[test]
fn plain_text_has_no_links() {
    assert!(find_links("just some words, nothing to click").is_empty());
    // A scheme word with no `://` is not a URL.
    assert!(find_links("the https thing").is_empty());
    // A bare slash / arithmetic is not a path.
    assert!(find_links("a / b and / alone").is_empty());
}

#[test]
fn detects_a_standalone_https_url() {
    let links = find_links("see https://example.test/page here");
    assert_eq!(links.len(), 1);
    let link = &links[0];
    assert_eq!(link.uri, "https://example.test/page");
    // The span covers exactly the visible URL text.
    let text = "see https://example.test/page here";
    assert_eq!(&text[link.start..link.end], "https://example.test/page");
}

#[test]
fn detects_http_and_file_schemes() {
    let http = find_links("http://a.test/x");
    assert_eq!(http.len(), 1);
    assert_eq!(http[0].uri, "http://a.test/x");

    let file = find_links("open file:///etc/hosts now");
    assert_eq!(file.len(), 1);
    assert_eq!(file[0].uri, "file:///etc/hosts");
}

#[test]
fn url_embedded_in_a_word_is_not_a_link() {
    // No left boundary before the scheme.
    assert!(find_links("xhttps://example.test/x").is_empty());
}

#[test]
fn bare_scheme_with_no_host_is_not_a_link() {
    assert!(find_links("https:// nothing").is_empty());
    assert!(find_links("file:// nothing").is_empty());
}

#[test]
fn trailing_sentence_punctuation_is_trimmed() {
    let links = find_links("visit https://example.test/a.");
    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0].uri, "https://example.test/a",
        "a trailing full stop is sentence punctuation, not part of the address"
    );

    let paren = find_links("(see https://example.test/b) for more");
    assert_eq!(paren.len(), 1);
    assert_eq!(
        paren[0].uri, "https://example.test/b",
        "an unmatched trailing close-paren is trimmed"
    );
}

#[test]
fn balanced_parens_inside_a_url_survive_trimming() {
    let links = find_links("https://wiki.test/Foo_(bar)");
    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0].uri, "https://wiki.test/Foo_(bar)",
        "a matched close-paren is part of the address"
    );
}

#[test]
fn two_urls_on_one_line_are_both_found_in_order() {
    let text = "https://a.test/1 and https://b.test/2";
    let links = find_links(text);
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].uri, "https://a.test/1");
    assert_eq!(links[1].uri, "https://b.test/2");
    assert!(
        links[0].end <= links[1].start,
        "spans are non-overlapping and left-to-right"
    );
}

#[test]
fn angle_bracketed_url_links_only_the_address() {
    // `<...>` is a common URL delimiter; the run stops at `>`.
    let links = find_links("<https://example.test/x>");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].uri, "https://example.test/x");
}

// ---------------------------------------------------------------------------
// File-path detection
// ---------------------------------------------------------------------------

#[test]
fn detects_an_absolute_path_with_file_scheme_uri() {
    let links = find_links("edited /home/user/main.rs today");
    assert_eq!(links.len(), 1);
    let text = "edited /home/user/main.rs today";
    assert_eq!(&text[links[0].start..links[0].end], "/home/user/main.rs");
    assert_eq!(
        links[0].uri, "file:///home/user/main.rs",
        "a bare absolute path opens via the file:// scheme"
    );
}

#[test]
fn bare_slash_and_double_slash_are_not_paths() {
    assert!(find_links("a / b").is_empty());
    assert!(find_links("trailing slash /").is_empty());
    assert!(find_links("//comment").is_empty());
}

#[test]
fn path_trailing_punctuation_is_trimmed() {
    let links = find_links("see /etc/hosts.");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].uri, "file:///etc/hosts");
}

#[test]
fn relative_path_is_not_linked() {
    // Only absolute (leading-slash) paths link; a relative path is ambiguous.
    assert!(find_links("src/main.rs is relative").is_empty());
}

// ---------------------------------------------------------------------------
// Span bookkeeping
// ---------------------------------------------------------------------------

#[test]
fn span_len_matches_visible_run() {
    let links = find_links("x https://example.test/abc");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].len(), "https://example.test/abc".len());
}

#[test]
fn multibyte_prefix_does_not_break_offsets() {
    // A leading multibyte char shifts byte offsets; the span must still slice
    // back to exactly the URL.
    let text = "café https://example.test/x";
    let links = find_links(text);
    assert_eq!(links.len(), 1);
    assert_eq!(
        &text[links[0].start..links[0].end],
        "https://example.test/x"
    );
}

// ---------------------------------------------------------------------------
// Escape encoding + sanitisation
// ---------------------------------------------------------------------------

#[test]
fn open_sequence_wraps_uri_in_osc8() {
    let seq = open_sequence("https://example.test/x");
    assert_eq!(seq, "\u{1b}]8;;https://example.test/x\u{1b}\\");
}

#[test]
fn close_sequence_is_the_empty_osc8() {
    assert_eq!(CLOSE_SEQUENCE, "\u{1b}]8;;\u{1b}\\");
}

#[test]
fn open_sequence_strips_control_bytes_from_uri() {
    // An ESC (or any control) inside the uri would terminate the escape early
    // and could inject a second escape; the sanitiser drops it.
    let dirty = "https://example.test/\u{1b}]0;evil\u{7}/x";
    assert!(uri_has_control_bytes(dirty), "fixture has control bytes");
    let seq = open_sequence(dirty);
    // Exactly two ESC bytes remain: the OSC 8 lead and the ST terminator.
    assert_eq!(
        seq.bytes().filter(|&b| b == 0x1b).count(),
        2,
        "only the framing ESCs survive: {seq:?}"
    );
    assert!(
        !seq.contains('\u{7}'),
        "the BEL control byte is stripped: {seq:?}"
    );
}

#[test]
fn round_trip_encode_a_detected_link() {
    // Detection + encoding compose: a detected link's uri encodes to a
    // well-formed OSC 8 open sequence whose payload is exactly the uri.
    let links = find_links("go to https://example.test/p now");
    assert_eq!(links.len(), 1);
    let open = open_sequence(&links[0].uri);
    assert!(open.starts_with("\u{1b}]8;;"));
    assert!(open.ends_with("\u{1b}\\"));
    assert!(open.contains("https://example.test/p"));
}
