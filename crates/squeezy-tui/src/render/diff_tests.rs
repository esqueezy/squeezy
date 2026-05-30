use super::*;
use squeezy_vcs::{DiffFile, DiffFileStatus, DiffHunk};

fn sample_file(path: &str, patch: &str) -> DiffFile {
    DiffFile {
        path: path.to_string(),
        status: DiffFileStatus::Modified,
        code: "M".to_string(),
        additions: 1,
        deletions: 1,
        binary: false,
        hunks: vec![DiffHunk {
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            start_line: 1,
            end_line: 2,
        }],
        patch: Some(patch.to_string()),
        patch_truncated: false,
    }
}

fn find_span<'a>(lines: &'a [Line<'static>], starts_with: &str) -> &'a Span<'static> {
    lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref().starts_with(starts_with))
        .unwrap_or_else(|| panic!("no span starts with {starts_with:?}"))
}

#[test]
fn added_lines_carry_green_background_tint() {
    let file = sample_file("src/lib.rs", "@@ -1 +1 @@\n-old\n+new\n");
    let lines = render_diff_file(&file);

    let add_sign = find_span(&lines, "+");
    assert_eq!(
        add_sign.style.fg, None,
        "+ sign should not carry a diff foreground color",
    );
    assert_eq!(
        add_sign.style.bg,
        Some(diff_add_bg()),
        "+ sign should carry add bg tint",
    );
}

#[test]
fn removed_lines_carry_red_background_tint() {
    let file = sample_file("src/lib.rs", "@@ -1 +1 @@\n-old\n+new\n");
    let lines = render_diff_file(&file);

    let del_sign = find_span(&lines, "-");
    assert_eq!(
        del_sign.style.fg, None,
        "- sign should not carry a diff foreground color",
    );
    assert_eq!(
        del_sign.style.bg,
        Some(diff_del_bg()),
        "- sign should carry delete bg tint",
    );
}

#[test]
fn context_lines_have_no_background_tint() {
    let file = sample_file("src/lib.rs", "@@ -1,3 +1,3 @@\n context\n-old\n+new\n");
    let lines = render_diff_file(&file);

    // context line content begins with a literal space, then the body.
    let context_line = lines
        .iter()
        .find(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref() == "context")
        })
        .expect("context line");
    for span in &context_line.spans {
        assert_eq!(
            span.style.bg, None,
            "context spans should not have a bg tint",
        );
    }
}

#[test]
fn gutter_on_changed_lines_shares_the_tint() {
    let file = sample_file("src/lib.rs", "@@ -1 +1 @@\n-old\n+new\n");
    let lines = render_diff_file(&file);

    let add_line = lines
        .iter()
        .find(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().starts_with('+'))
        })
        .expect("add line");
    let del_line = lines
        .iter()
        .find(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().starts_with('-'))
        })
        .expect("del line");

    // Every span on a +/- line (gutter, sign, content) carries the tint.
    assert_eq!(add_line.style.bg, Some(diff_add_bg()));
    assert_eq!(del_line.style.bg, Some(diff_del_bg()));
    for span in &add_line.spans {
        assert_eq!(span.style.bg, Some(diff_add_bg()));
    }
    for span in &del_line.spans {
        assert_eq!(span.style.bg, Some(diff_del_bg()));
    }
}

#[test]
fn changed_rows_do_not_syntax_highlight_text() {
    let file = sample_file(
        "src/lib.rs",
        "@@ -1 +1 @@\n-fn old() {}\n+fn brand_new() {}\n",
    );
    let lines = render_diff_file(&file);

    let add_content = lines
        .iter()
        .find(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().starts_with('+'))
        })
        .and_then(|line| {
            line.spans
                .iter()
                .find(|span| span.content.as_ref().contains("fn brand_new"))
        })
        .expect("added line content span");
    assert_eq!(
        add_content.style.fg, None,
        "added row content should keep the default foreground",
    );
    assert_eq!(add_content.style.bg, Some(diff_add_bg()));
}

#[test]
fn unknown_extension_keeps_default_foreground_on_diff_background() {
    let file = sample_file("notes.unknownext", "@@ -1 +1 @@\n-old line\n+new line\n");
    let lines = render_diff_file(&file);

    let add_content = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "new line")
        .expect("add content span");
    assert_eq!(
        add_content.style.fg, None,
        "without a known language hint diff rows should not color text red/green",
    );
    assert_eq!(add_content.style.bg, Some(diff_add_bg()));
}

#[test]
fn language_hint_from_path_extracts_extension() {
    assert_eq!(language_hint_from_path("src/lib.rs"), Some("rs"));
    assert_eq!(language_hint_from_path("README"), None);
    assert_eq!(language_hint_from_path("a/b/c.tsx"), Some("tsx"));
    assert_eq!(language_hint_from_path(".gitignore"), None);
}
