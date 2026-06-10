//! Unit tests for the pure Contextual Action Palette model (§12.1.2). These
//! exercise the per-kind gathering rule, the action ordering, the labels, and the
//! cursor math directly, with no terminal — the palette's keyboard/mouse/render
//! integration is covered by the capture-sink suite in `lib_tests.rs`.

use super::*;

/// Every `UnitKind` always offers the five always-available verbs (copy entry,
/// annotate, toggle fold, related entries, jump) regardless of detail.
#[test]
fn every_unit_offers_the_always_available_verbs() {
    let always = [
        PaletteAction::CopyEntry,
        PaletteAction::Annotate,
        PaletteAction::ToggleFold,
        PaletteAction::RelatedLinks,
        PaletteAction::JumpToEntry,
    ];
    for kind in [
        UnitKind::UserMessage,
        UnitKind::AssistantMessage,
        UnitKind::Reasoning,
        UnitKind::ToolResult,
        UnitKind::PlanCard,
        UnitKind::Diff,
        UnitKind::Note,
    ] {
        let actions = applicable_actions(kind, false);
        for verb in always {
            assert!(
                actions.contains(&verb),
                "{kind:?} must offer {verb:?}: {actions:?}",
            );
        }
    }
}

/// An assistant message offers copy code and quote, but not copy-tool-output. With
/// no detail-pane content it does not offer open-in-detail.
#[test]
fn assistant_message_actions() {
    let actions = applicable_actions(UnitKind::AssistantMessage, false);
    assert!(actions.contains(&PaletteAction::CopyCode), "{actions:?}");
    assert!(
        actions.contains(&PaletteAction::QuoteToCompose),
        "{actions:?}",
    );
    assert!(
        !actions.contains(&PaletteAction::CopyToolOutput),
        "a message has no tool output: {actions:?}",
    );
    assert!(
        !actions.contains(&PaletteAction::OpenInDetail),
        "no detail content -> no open-in-detail: {actions:?}",
    );
}

/// A short user message offers quote (it is quotable prose) but not copy-tool-output.
#[test]
fn user_message_is_quotable_but_has_no_tool_output() {
    let actions = applicable_actions(UnitKind::UserMessage, false);
    assert!(
        actions.contains(&PaletteAction::QuoteToCompose),
        "{actions:?}",
    );
    assert!(
        actions.contains(&PaletteAction::CopyCode),
        "a user message may carry a fenced snippet: {actions:?}",
    );
    assert!(
        !actions.contains(&PaletteAction::CopyToolOutput),
        "{actions:?}",
    );
}

/// A tool result offers copy-tool-output and copy-code, but is NOT quotable prose
/// (quoting a wall of tool output into the composer is the accidental-mutation risk
/// the spec warns about).
#[test]
fn tool_result_offers_tool_output_not_quote() {
    let actions = applicable_actions(UnitKind::ToolResult, true);
    assert!(
        actions.contains(&PaletteAction::CopyToolOutput),
        "{actions:?}",
    );
    assert!(actions.contains(&PaletteAction::CopyCode), "{actions:?}");
    assert!(
        !actions.contains(&PaletteAction::QuoteToCompose),
        "tool output is not quotable prose: {actions:?}",
    );
    // With detail content, open-in-detail appears.
    assert!(
        actions.contains(&PaletteAction::OpenInDetail),
        "tool result with detail offers open-in-detail: {actions:?}",
    );
}

/// A plan / diff / note carries no fenced code and is not quotable prose, so the
/// code/quote/tool-output verbs are absent; only the always-available verbs (plus
/// open-in-detail when it has detail) remain.
#[test]
fn plan_diff_note_offer_only_the_general_verbs() {
    for kind in [UnitKind::PlanCard, UnitKind::Diff, UnitKind::Note] {
        let actions = applicable_actions(kind, false);
        assert!(
            !actions.contains(&PaletteAction::CopyCode),
            "{kind:?} carries no code: {actions:?}",
        );
        assert!(
            !actions.contains(&PaletteAction::CopyToolOutput),
            "{kind:?}: {actions:?}",
        );
        assert!(
            !actions.contains(&PaletteAction::QuoteToCompose),
            "{kind:?} is not quotable prose: {actions:?}",
        );
    }
}

/// `has_detail` is the only switch for open-in-detail: flipping it adds/removes the
/// verb without touching anything else.
#[test]
fn open_in_detail_follows_has_detail() {
    let without = applicable_actions(UnitKind::PlanCard, false);
    let with = applicable_actions(UnitKind::PlanCard, true);
    assert!(
        !without.contains(&PaletteAction::OpenInDetail),
        "{without:?}"
    );
    assert!(with.contains(&PaletteAction::OpenInDetail), "{with:?}");
    // Adding open-in-detail adds exactly one action, nothing else changes.
    assert_eq!(with.len(), without.len() + 1);
}

/// The gathered list is always in `PaletteAction::ALL` (menu) order, so the menu
/// reads top-to-bottom the same way regardless of which verbs apply.
#[test]
fn gathered_actions_preserve_menu_order() {
    let actions = applicable_actions(UnitKind::ToolResult, true);
    let all = PaletteAction::ALL;
    let mut last = 0;
    for action in &actions {
        let pos = all.iter().position(|a| a == action).expect("in ALL");
        assert!(pos >= last, "out of menu order at {action:?}: {actions:?}");
        last = pos;
    }
}

/// The fold label flips with the collapsed state so the row reads honestly.
#[test]
fn toggle_fold_label_reflects_collapsed_state() {
    assert_eq!(PaletteAction::ToggleFold.label(true), "expand entry");
    assert_eq!(PaletteAction::ToggleFold.label(false), "collapse entry");
}

/// `open` parks the cursor on the first action and exposes the gathered list.
#[test]
fn open_parks_cursor_on_first_action() {
    let actions = applicable_actions(UnitKind::AssistantMessage, false);
    let count = actions.len();
    let palette = ActionPalette::open(
        7,
        UnitKind::AssistantMessage,
        false,
        "Here is the answer".to_string(),
        actions,
    );
    assert_eq!(palette.entry_id, 7);
    assert_eq!(palette.len(), count);
    assert_eq!(palette.selected(), 0);
    assert_eq!(palette.selected_action(), palette.action_at(0));
    assert!(!palette.is_empty());
}

/// Cursor moves clamp at both ends (no wrap) and `select` lands on an exact row.
#[test]
fn cursor_moves_clamp_and_select_lands() {
    let actions = applicable_actions(UnitKind::ToolResult, true);
    let count = actions.len();
    assert!(count >= 3, "tool result offers several actions: {count}");
    let mut palette =
        ActionPalette::open(1, UnitKind::ToolResult, false, "shell".to_string(), actions);

    // Up at the top is a no-op.
    palette.move_up();
    assert_eq!(palette.selected(), 0);

    // Down advances and clamps at the last action.
    for _ in 0..count + 5 {
        palette.move_down();
    }
    assert_eq!(
        palette.selected(),
        count - 1,
        "Down clamps at the last action"
    );

    // Up retreats.
    palette.move_up();
    assert_eq!(palette.selected(), count - 2);

    // select clamps a far index to the last row.
    palette.select(9999);
    assert_eq!(palette.selected(), count - 1);
    // select lands exactly.
    palette.select(1);
    assert_eq!(palette.selected(), 1);
    assert_eq!(palette.selected_action(), palette.action_at(1));
}

/// An empty action list (a degenerate gather) is handled without panicking:
/// `is_empty` is true, the cursor stays at 0, and `selected_action` is `None`.
#[test]
fn empty_palette_is_safe() {
    let mut palette = ActionPalette::open(1, UnitKind::Note, false, "note".to_string(), Vec::new());
    assert!(palette.is_empty());
    assert_eq!(palette.selected(), 0);
    assert_eq!(palette.selected_action(), None);
    // Moves on an empty palette are harmless no-ops.
    palette.move_down();
    palette.move_up();
    palette.select(3);
    assert_eq!(palette.selected(), 0);
}

/// Every kind's noun is a non-empty ASCII string (screen-reader friendly, no
/// glyphs) and every action's label is non-empty.
#[test]
fn nouns_and_labels_are_nonempty_ascii() {
    for kind in [
        UnitKind::UserMessage,
        UnitKind::AssistantMessage,
        UnitKind::Reasoning,
        UnitKind::ToolResult,
        UnitKind::PlanCard,
        UnitKind::Diff,
        UnitKind::Note,
    ] {
        let noun = kind.noun();
        assert!(!noun.is_empty(), "{kind:?} noun empty");
        assert!(noun.is_ascii(), "{kind:?} noun not ascii: {noun}");
    }
    for action in PaletteAction::ALL {
        assert!(!action.label(false).is_empty(), "{action:?} label empty");
        assert!(!action.label(true).is_empty(), "{action:?} label empty");
    }
}
