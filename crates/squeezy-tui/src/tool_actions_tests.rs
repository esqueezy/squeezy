//! Unit tests for the pure Actionable Tool Outputs model (§12.3.1). These exercise
//! the per-line detection rule (paths, URLs, errors, diffs, commands), the
//! ordering/capping, the bounded text, and the cursor/summary math directly, with
//! no terminal — the overlay's keyboard/mouse/render integration through the real
//! `render()` is covered by the capture-sink suite in `lib_tests.rs`.

use super::*;

/// The kinds detected for a multi-line output, in the order they appear, so a
/// single-shot assertion can pin the classifier across all five families.
fn detected_kinds(text: &str) -> Vec<ActionableKind> {
    detect_actionable_items(7, text)
        .into_iter()
        .map(|item| item.kind)
        .collect()
}

#[test]
fn empty_and_whitespace_output_yields_no_items() {
    assert!(detect_actionable_items(1, "").is_empty());
    assert!(detect_actionable_items(1, "   \n\t\n  ").is_empty());
}

#[test]
fn a_url_is_detected_and_copied_verbatim() {
    let items = detect_actionable_items(3, "see https://example.test/path?q=1 for details");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, ActionableKind::Url);
    assert_eq!(items[0].text, "https://example.test/path?q=1");
    assert_eq!(items[0].entry_id, 3, "the source entry id is stamped on");
}

#[test]
fn an_absolute_path_is_detected_as_a_path_not_a_url() {
    let items = detect_actionable_items(1, "compiling /home/user/project/src/main.rs now");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, ActionableKind::Path);
    assert_eq!(items[0].text, "/home/user/project/src/main.rs");
}

#[test]
fn a_file_scheme_url_classifies_as_url() {
    let items = detect_actionable_items(1, "open file:///var/log/system.log here");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, ActionableKind::Url);
    assert!(items[0].text.starts_with("file://"));
}

#[test]
fn an_error_line_is_detected() {
    let items = detect_actionable_items(1, "error[E0277]: the trait bound is not satisfied");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, ActionableKind::Error);
    assert!(items[0].text.starts_with("error[E0277]"));
}

#[test]
fn varied_error_shapes_all_classify() {
    for line in [
        "error: could not compile `foo`",
        "thread 'main' panicked at 'boom'",
        "fatal: not a git repository",
        "assertion failed: left == right",
        "test result: FAILED. 1 passed; 1 failed",
        "OSError: [Errno 13] Permission denied: '/etc/shadow'",
    ] {
        let kinds = detected_kinds(line);
        assert!(
            kinds.contains(&ActionableKind::Error),
            "{line:?} should classify as an error, got {kinds:?}",
        );
    }
}

#[test]
fn diff_headers_are_detected() {
    let text = "diff --git a/x.rs b/x.rs\n@@ -1,3 +1,4 @@\n+++ b/x.rs\n--- a/x.rs";
    let kinds = detected_kinds(text);
    assert_eq!(
        kinds,
        vec![
            ActionableKind::Diff,
            ActionableKind::Diff,
            ActionableKind::Diff,
            ActionableKind::Diff,
        ],
    );
}

#[test]
fn a_shell_prompt_echo_is_detected_as_a_command_with_the_prompt_stripped() {
    let items = detect_actionable_items(1, "$ cargo test --all-features");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, ActionableKind::Command);
    assert_eq!(
        items[0].text, "cargo test --all-features",
        "the `$ ` prompt is stripped from the copied command",
    );
}

#[test]
fn a_bare_dollar_or_arithmetic_trace_is_not_a_command() {
    // A `$ ` with nothing after it, and a `+ 1` arithmetic `set -x` trace, must
    // not become phantom commands (the first token must look command-like).
    assert!(detect_actionable_items(1, "$ ").is_empty());
    assert!(detect_actionable_items(1, "+ 1").is_empty());
}

#[test]
fn windows_drive_path_in_a_location_is_not_mistaken_for_a_unix_path() {
    // A Windows `C:\dir\file.rs:10:5` line has no leading `/`, so the conservative
    // POSIX-path detector does not light up; the line still has no other actionable
    // signal, so it yields nothing rather than a false-positive path. This proves
    // the detector degrades safely on Windows-style locations rather than
    // mis-linking them.
    let items = detect_actionable_items(1, "  C:\\dir\\file.rs:10:5");
    assert!(
        items.iter().all(|i| i.kind != ActionableKind::Path),
        "a backslash drive path must not classify as a POSIX path: {items:?}",
    );
}

#[test]
fn a_path_with_an_error_keyword_classifies_as_error_first() {
    // The error detector runs before the bare-path detector, so a failure line that
    // also names a path is surfaced as the (more useful) error line, copying the
    // whole line rather than just the path.
    let items = detect_actionable_items(1, "error: /tmp/out.log: permission denied");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, ActionableKind::Error);
    assert!(items[0].text.contains("permission denied"));
}

#[test]
fn ansi_escapes_are_stripped_before_detection() {
    // A colorized error line (CSI red, reset) must still classify and copy clean
    // text with no escape bytes.
    let colored = "\u{1b}[31merror: boom\u{1b}[0m";
    let items = detect_actionable_items(1, colored);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, ActionableKind::Error);
    assert!(
        !items[0].text.contains('\u{1b}'),
        "the copied text must be ANSI-free: {:?}",
        items[0].text,
    );
}

#[test]
fn detection_is_capped_so_a_giant_output_cannot_explode_the_overlay() {
    let mut text = String::new();
    for i in 0..(ITEMS_CAP * 4) {
        text.push_str(&format!("/abs/path/file-{i}.rs\n"));
    }
    let items = detect_actionable_items(1, &text);
    assert_eq!(items.len(), ITEMS_CAP, "items are capped at ITEMS_CAP");
}

#[test]
fn item_text_is_bounded() {
    let long = format!("/x/{}", "a".repeat(500));
    let items = detect_actionable_items(1, &long);
    assert_eq!(items.len(), 1);
    assert!(
        items[0].text.chars().count() <= TEXT_CAP + 1,
        "text is capped (plus the ellipsis): {} chars",
        items[0].text.chars().count(),
    );
    assert!(
        items[0].text.ends_with('\u{2026}'),
        "a cut item is ellipsized"
    );
}

#[test]
fn line_index_orders_items_by_output_line() {
    let text = "/first/path.rs\nplain prose\n$ run me";
    let items = detect_actionable_items(1, text);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].line_index, 0, "the path is on line 0");
    assert_eq!(items[1].line_index, 2, "the command is on line 2");
}

#[test]
fn primary_action_is_copy_and_actions_include_copy_and_jump() {
    let items = detect_actionable_items(1, "/abs/file.rs");
    let item = &items[0];
    assert_eq!(item.primary_action(), WorkflowAction::Copy);
    let actions = item.actions();
    assert!(actions.contains(&WorkflowAction::Copy));
    assert!(actions.contains(&WorkflowAction::Jump));
}

#[test]
fn every_kind_supports_jump() {
    for kind in ActionableKind::ALL.iter().copied() {
        assert!(kind.supports_jump(), "{kind:?} should support jump today");
    }
}

#[test]
fn kind_and_action_labels_are_ascii_only() {
    for kind in ActionableKind::ALL.iter().copied() {
        assert!(
            kind.label().is_ascii() && !kind.label().is_empty(),
            "{kind:?} label must be non-empty ASCII",
        );
    }
    for action in WorkflowAction::ALL.iter().copied() {
        assert!(
            action.label().is_ascii() && !action.label().is_empty(),
            "{action:?} label must be non-empty ASCII",
        );
    }
}

// ---- ToolActions overlay state ----

fn sample_overlay() -> ToolActions {
    let items = detect_actionable_items(
        42,
        "/abs/a.rs\nhttps://x.test/y\nerror: boom\ndiff --git a/x b/x",
    );
    ToolActions::open(42, "shell".to_string(), items)
}

#[test]
fn open_overlay_parks_cursor_on_first_item() {
    let overlay = sample_overlay();
    assert!(!overlay.is_empty());
    assert_eq!(overlay.len(), 4);
    assert_eq!(overlay.selected(), 0);
    assert_eq!(overlay.entry_id(), 42);
    assert_eq!(overlay.title(), "shell");
    assert_eq!(overlay.selected_item().unwrap().kind, ActionableKind::Path);
}

#[test]
fn cursor_moves_and_clamps_at_both_ends() {
    let mut overlay = sample_overlay();
    overlay.move_up();
    assert_eq!(overlay.selected(), 0, "up at the top saturates");
    overlay.move_down();
    overlay.move_down();
    assert_eq!(overlay.selected(), 2);
    for _ in 0..10 {
        overlay.move_down();
    }
    assert_eq!(
        overlay.selected(),
        overlay.len() - 1,
        "down clamps to the last item",
    );
}

#[test]
fn select_clamps_to_the_item_range() {
    let mut overlay = sample_overlay();
    overlay.select(99);
    assert_eq!(overlay.selected(), overlay.len() - 1);
    overlay.select(1);
    assert_eq!(overlay.selected(), 1);
    assert_eq!(overlay.selected_item().unwrap().kind, ActionableKind::Url);
}

#[test]
fn an_empty_overlay_paints_an_empty_state_and_has_no_selected_item() {
    let overlay = ToolActions::open(1, "grep".to_string(), Vec::new());
    assert!(overlay.is_empty());
    assert_eq!(overlay.len(), 0);
    assert_eq!(overlay.selected(), 0, "the cursor never goes negative");
    assert!(overlay.selected_item().is_none());
    assert_eq!(overlay.summary(), "", "an empty overlay has no summary");
    // Cursor moves on an empty list are no-ops, not panics.
    let mut overlay = overlay;
    overlay.move_up();
    overlay.move_down();
    overlay.select(5);
    assert_eq!(overlay.selected(), 0);
}

#[test]
fn summary_counts_each_kind() {
    let overlay = sample_overlay();
    let summary = overlay.summary();
    assert!(summary.starts_with("4 items"), "summary: {summary}");
    assert!(summary.contains("1 path"));
    assert!(summary.contains("1 url"));
    assert!(summary.contains("1 error"));
    assert!(summary.contains("1 diff"));
}
