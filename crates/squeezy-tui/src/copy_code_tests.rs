use super::*;
use crate::transcript_surface::{
    EntryId, FoldState, RowId, RowKind, TranscriptRow, plain_text_of_line,
};
use ratatui::text::Line;

/// Build a row owned by `entry` (or chrome when `entry` is `None`) carrying
/// `text` as its plain `copy_text`. Mirrors the helper in `copy_tests.rs`: one
/// `TranscriptRow` is one already-wrapped visual line.
fn row(id: usize, entry: Option<u64>, kind: Option<RowKind>, text: &str) -> TranscriptRow {
    let line = Line::from(text.to_string());
    let copy_text = plain_text_of_line(&line);
    let char_len = copy_text.chars().count();
    TranscriptRow {
        row_id: RowId(id),
        entry_id: entry.map(EntryId),
        entry_kind: kind,
        visual_line_index: 0,
        line,
        copy_text,
        text_range: 0..char_len,
        style_spans: Vec::new(),
        fold_state: FoldState::Expanded,
        search_match_ranges: Vec::new(),
    }
}

/// The box-drawing / rail glyphs a code-aware copy must never carry through.
const BOX_DRAWING_GLYPHS: &[char] = &[
    '│', '├', '╰', '─', '☽', '☾', '◐', '◑', '◔', '◕', '●', '○', '▌',
];

fn assert_no_box_drawing(text: &str) {
    for ch in text.chars() {
        assert!(
            !BOX_DRAWING_GLYPHS.contains(&ch),
            "code copy must be clean, but carried rail glyph {ch:?} in:\n{text}"
        );
    }
}

// ---------------------------------------------------------------------------
// fence_language
// ---------------------------------------------------------------------------

#[test]
fn fence_language_reads_info_string() {
    assert_eq!(fence_language("```rust"), "rust");
    assert_eq!(fence_language("```RUST"), "rust");
    assert_eq!(fence_language("  ```python"), "python");
    assert_eq!(fence_language("~~~sh"), "sh");
}

#[test]
fn fence_language_is_empty_for_bare_fence() {
    assert_eq!(fence_language("```"), "");
    assert_eq!(fence_language("~~~"), "");
}

#[test]
fn fence_language_takes_first_token_only() {
    // CommonMark lets the opening fence carry trailing words; the language is the
    // first whitespace-delimited token.
    assert_eq!(fence_language("```rust ignore should_panic"), "rust");
    assert_eq!(fence_language("````toml"), "toml");
}

// ---------------------------------------------------------------------------
// extract_code_blocks
// ---------------------------------------------------------------------------

#[test]
fn extracts_single_block_with_language_and_interior() {
    let rows = vec![
        row(0, Some(1), Some(RowKind::Message), "intro prose"),
        row(1, Some(1), Some(RowKind::Message), "```rust"),
        row(2, Some(1), Some(RowKind::Message), "let x = 1;"),
        row(3, Some(1), Some(RowKind::Message), "let y = 2;"),
        row(4, Some(1), Some(RowKind::Message), "```"),
        row(5, Some(1), Some(RowKind::Message), "outro prose"),
    ];
    let blocks = extract_code_blocks(&rows);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].language, "rust");
    assert_eq!(blocks[0].lines, vec!["let x = 1;", "let y = 2;"]);
}

#[test]
fn extracts_multiple_blocks_in_document_order() {
    let rows = vec![
        row(0, Some(1), Some(RowKind::Message), "```py"),
        row(1, Some(1), Some(RowKind::Message), "print(1)"),
        row(2, Some(1), Some(RowKind::Message), "```"),
        row(3, Some(1), Some(RowKind::Message), "and then:"),
        row(4, Some(1), Some(RowKind::Message), "```sh"),
        row(5, Some(1), Some(RowKind::Message), "ls -la"),
        row(6, Some(1), Some(RowKind::Message), "```"),
    ];
    let blocks = extract_code_blocks(&rows);
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].language, "py");
    assert_eq!(blocks[0].lines, vec!["print(1)"]);
    assert_eq!(blocks[1].language, "sh");
    assert_eq!(blocks[1].lines, vec!["ls -la"]);
}

#[test]
fn extract_strips_rail_gutter_from_fences_and_body() {
    // Every row carries rail chrome; detection AND the captured interior must be
    // gutter-stripped so the extracted code is clean.
    let rows = vec![
        row(0, Some(1), Some(RowKind::Message), "│ ```python"),
        row(1, Some(1), Some(RowKind::Message), "│ name = \"x\""),
        row(2, Some(1), Some(RowKind::Message), "│ print(name)"),
        row(3, Some(1), Some(RowKind::Message), "│ ```"),
    ];
    let blocks = extract_code_blocks(&rows);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].language, "python");
    assert_eq!(blocks[0].lines, vec!["name = \"x\"", "print(name)"]);
    for line in &blocks[0].lines {
        assert_no_box_drawing(line);
    }
}

#[test]
fn dangling_unclosed_fence_yields_no_block() {
    // An opening fence with no close is still-streaming code: it must NOT be
    // emitted as a (truncated) block.
    let rows = vec![
        row(0, Some(1), Some(RowKind::Message), "```rust"),
        row(1, Some(1), Some(RowKind::Message), "fn main() {}"),
    ];
    assert!(extract_code_blocks(&rows).is_empty());
}

#[test]
fn prose_only_rows_yield_no_blocks() {
    let rows = vec![
        row(0, Some(1), Some(RowKind::Message), "just talking"),
        row(1, Some(1), Some(RowKind::Message), "no code here"),
    ];
    assert!(extract_code_blocks(&rows).is_empty());
}

#[test]
fn empty_rows_yield_no_blocks() {
    assert!(extract_code_blocks(&[]).is_empty());
}

#[test]
fn empty_fenced_block_is_recorded_with_no_lines() {
    // ``` immediately followed by ``` is a deliberately-empty block.
    let rows = vec![
        row(0, Some(1), Some(RowKind::Message), "```js"),
        row(1, Some(1), Some(RowKind::Message), "```"),
    ];
    let blocks = extract_code_blocks(&rows);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].language, "js");
    assert!(blocks[0].lines.is_empty());
}

#[test]
fn cjk_code_interior_survives_verbatim() {
    let rows = vec![
        row(0, Some(1), Some(RowKind::Message), "│ ```py"),
        row(1, Some(1), Some(RowKind::Message), "│ 名前 = \"世界\""),
        row(2, Some(1), Some(RowKind::Message), "│ print(名前)"),
        row(3, Some(1), Some(RowKind::Message), "│ ```"),
    ];
    let blocks = extract_code_blocks(&rows);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].lines, vec!["名前 = \"世界\"", "print(名前)"]);
}

// ---------------------------------------------------------------------------
// render_code_payload
// ---------------------------------------------------------------------------

#[test]
fn fenced_style_reemits_clean_language_fence() {
    let blocks = vec![CodeBlock {
        language: "rust".to_string(),
        lines: vec!["let x = 1;".to_string(), "let y = 2;".to_string()],
    }];
    let payload = render_code_payload(&blocks, CodePayloadStyle::Fenced).unwrap();
    assert_eq!(payload, "```rust\nlet x = 1;\nlet y = 2;\n```");
    assert_no_box_drawing(&payload);
}

#[test]
fn fenced_style_omits_language_for_bare_block() {
    let blocks = vec![CodeBlock {
        language: String::new(),
        lines: vec!["plain".to_string()],
    }];
    let payload = render_code_payload(&blocks, CodePayloadStyle::Fenced).unwrap();
    assert_eq!(payload, "```\nplain\n```");
}

#[test]
fn fenced_style_joins_blocks_with_blank_line() {
    let blocks = vec![
        CodeBlock {
            language: "py".to_string(),
            lines: vec!["print(1)".to_string()],
        },
        CodeBlock {
            language: "sh".to_string(),
            lines: vec!["ls".to_string()],
        },
    ];
    let payload = render_code_payload(&blocks, CodePayloadStyle::Fenced).unwrap();
    assert_eq!(payload, "```py\nprint(1)\n```\n\n```sh\nls\n```");
}

#[test]
fn bare_style_drops_fences_and_joins_interiors() {
    let blocks = vec![
        CodeBlock {
            language: "py".to_string(),
            lines: vec!["a = 1".to_string(), "b = 2".to_string()],
        },
        CodeBlock {
            language: "sh".to_string(),
            lines: vec!["echo hi".to_string()],
        },
    ];
    let payload = render_code_payload(&blocks, CodePayloadStyle::Bare).unwrap();
    assert_eq!(payload, "a = 1\nb = 2\n\necho hi");
    assert!(!payload.contains("```"), "bare style carries no fences");
}

#[test]
fn render_none_for_empty_block_list() {
    assert!(render_code_payload(&[], CodePayloadStyle::Fenced).is_none());
    assert!(render_code_payload(&[], CodePayloadStyle::Bare).is_none());
}

#[test]
fn empty_block_renders_an_empty_fence_in_fenced_style() {
    let blocks = vec![CodeBlock {
        language: "js".to_string(),
        lines: Vec::new(),
    }];
    let payload = render_code_payload(&blocks, CodePayloadStyle::Fenced).unwrap();
    assert_eq!(payload, "```js\n```");
}

// ---------------------------------------------------------------------------
// gather_code (end-to-end over the row model)
// ---------------------------------------------------------------------------

#[test]
fn gather_code_fenced_over_railed_mixed_message() {
    // A realistic assistant answer: prose, then two fenced blocks in different
    // languages, all under rail gutters. `gather_code` must extract both blocks,
    // preserve their languages, strip every rail, and drop the prose.
    let rows = vec![
        row(
            0,
            Some(1),
            Some(RowKind::Message),
            "╰─☽ here are two snippets:",
        ),
        row(1, Some(1), Some(RowKind::Message), "│ ```rust"),
        row(2, Some(1), Some(RowKind::Message), "│ fn main() {}"),
        row(3, Some(1), Some(RowKind::Message), "│ ```"),
        row(4, Some(1), Some(RowKind::Message), "│ and in bash:"),
        row(5, Some(1), Some(RowKind::Message), "│ ```bash"),
        row(6, Some(1), Some(RowKind::Message), "│ echo done"),
        row(7, Some(1), Some(RowKind::Message), "│ ```"),
    ];
    let payload = gather_code(&rows, CodePayloadStyle::Fenced).unwrap();
    assert_eq!(
        payload,
        "```rust\nfn main() {}\n```\n\n```bash\necho done\n```"
    );
    assert_no_box_drawing(&payload);
    // The prose lines were dropped entirely — only code survives.
    assert!(!payload.contains("snippets"));
    assert!(!payload.contains("and in bash"));
}

#[test]
fn gather_code_bare_strips_fences_too() {
    let rows = vec![
        row(0, Some(1), Some(RowKind::Message), "│ ```rust"),
        row(1, Some(1), Some(RowKind::Message), "│ fn main() {}"),
        row(2, Some(1), Some(RowKind::Message), "│ ```"),
    ];
    let payload = gather_code(&rows, CodePayloadStyle::Bare).unwrap();
    assert_eq!(payload, "fn main() {}");
    assert!(!payload.contains("```"));
}

#[test]
fn gather_code_none_when_no_code() {
    let rows = vec![row(0, Some(1), Some(RowKind::Message), "just prose")];
    assert!(gather_code(&rows, CodePayloadStyle::Fenced).is_none());
    assert!(gather_code(&rows, CodePayloadStyle::Bare).is_none());
}
