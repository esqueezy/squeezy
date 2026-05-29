use super::*;

#[test]
fn renders_plain_text_into_grid() {
    let rendered = render_markdown_to_grid("hello world", 16, 4).expect("render");
    assert!(
        rendered.plain_text.starts_with("hello world"),
        "plain={:?}",
        rendered.plain_text
    );
    assert!(
        rendered.ansi.contains("hello world"),
        "ansi={:?}",
        rendered.ansi
    );
    // First row should have 11 non-blank cells.
    let row0: Vec<&TuiCell> = rendered.cells.iter().filter(|c| c.y == 0).collect();
    assert!(!row0.is_empty(), "expected non-blank cells in row 0");
}

#[test]
fn dimensions_round_to_grid_bounds() {
    let rendered = render_markdown_to_grid("hi", 4, 2).expect("render");
    // Two rows of four columns + one newline per row.
    assert_eq!(rendered.plain_text.len(), (4 + 1) * 2);
}

#[test]
fn capture_renders_overlay_details_into_grid() {
    let overlays = vec![TuiOverlayEvent {
        kind: "request_user_input".into(),
        summary: "Pick a mode".into(),
        disposition: "freeform:Use the compact view with full details".into(),
        details: vec!["choices: compact=compact, full=full".into()],
        preview: Vec::new(),
    }];
    let rendered = render_capture_to_grid("Done.", &overlays, 80, 8).expect("render");
    assert!(
        rendered.plain_text.contains("Overlay state"),
        "{}",
        rendered.plain_text
    );
    assert!(
        rendered
            .plain_text
            .contains("freeform:Use the compact view with full"),
        "{}",
        rendered.plain_text
    );
    assert!(
        rendered.plain_text.contains("details"),
        "{}",
        rendered.plain_text
    );
}

#[test]
fn render_reports_visual_truncation() {
    let rendered =
        render_markdown_to_grid("one\n\ntwo\n\nthree\n\nfour\n\nfive", 20, 3).expect("render");
    assert!(rendered.visual_truncated, "{rendered:?}");
    assert!(rendered.omitted_line_count > 0, "{rendered:?}");
}

#[test]
fn provision_returns_none_when_disabled() {
    let dir = std::env::temp_dir().join(format!("squeezy-eval-tui-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let cfg = TuiCaptureConfig::default();
    let writer = TuiCaptureWriter::provision(&dir, &cfg).expect("provision");
    assert!(writer.is_none(), "disabled config should not provision");
}
