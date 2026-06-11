//! Unit tests for the Compare Subagent Outputs (§12.8.3) model: the bounded
//! two-slot mark set (toggle, cap, remove, pair ordering) and the compare-state
//! focus/scroll bookkeeping. Pure over `SubagentCompareMarks` /
//! `SubagentCompareState` — no terminal, no `TuiApp`. The end-to-end
//! keyboard/mouse/render coverage lives in `lib_tests.rs`.

use super::*;

#[test]
fn marks_start_empty() {
    let marks = SubagentCompareMarks::new();
    assert_eq!(marks.len(), 0);
    assert_eq!(marks.pair(), None);
    assert!(!marks.contains(7));
}

#[test]
fn toggle_marks_and_unmarks_and_reports_state() {
    let mut marks = SubagentCompareMarks::new();
    // Marking returns true (now marked); unmarking returns false.
    assert!(marks.toggle(3));
    assert!(marks.contains(3));
    assert_eq!(marks.len(), 1);
    assert!(!marks.toggle(3));
    assert!(!marks.contains(3));
    assert_eq!(marks.len(), 0);
}

#[test]
fn second_distinct_mark_forms_a_pair_oldest_left() {
    let mut marks = SubagentCompareMarks::new();
    assert!(marks.toggle(10));
    // One mark: no pair yet.
    assert_eq!(marks.pair(), None);
    assert!(marks.toggle(20));
    // Older mark (10) is left, newer (20) is right — the diff's old/new sides.
    assert_eq!(marks.pair(), Some((10, 20)));
    assert_eq!(marks.ids(), &[10, 20]);
}

#[test]
fn third_mark_rolls_the_oldest_off_keeping_the_newest_two() {
    let mut marks = SubagentCompareMarks::new();
    marks.toggle(1);
    marks.toggle(2);
    marks.toggle(3);
    // The two-slot cap keeps the newest two in order.
    assert_eq!(marks.len(), 2);
    assert!(!marks.contains(1));
    assert_eq!(marks.pair(), Some((2, 3)));
}

#[test]
fn remove_drops_a_specific_mark() {
    let mut marks = SubagentCompareMarks::new();
    marks.toggle(5);
    marks.toggle(6);
    assert!(marks.remove(5));
    assert_eq!(marks.ids(), &[6]);
    // Removing a mark that isn't there is a no-op false.
    assert!(!marks.remove(99));
    assert_eq!(marks.ids(), &[6]);
}

#[test]
fn clear_forgets_every_mark() {
    let mut marks = SubagentCompareMarks::new();
    marks.toggle(8);
    marks.toggle(9);
    marks.clear();
    assert_eq!(marks.len(), 0);
    assert_eq!(marks.pair(), None);
}

#[test]
fn re_marking_an_existing_mark_unmarks_it_not_appends() {
    // Toggling an already-marked id must remove it (so a double-c is a clean
    // unmark), never push a duplicate that would skew the pair.
    let mut marks = SubagentCompareMarks::new();
    marks.toggle(1);
    marks.toggle(2);
    // Unmark the older one, then mark a new one: the surviving + new form the pair.
    assert!(!marks.toggle(1));
    assert_eq!(marks.ids(), &[2]);
    marks.toggle(3);
    assert_eq!(marks.pair(), Some((2, 3)));
}

#[test]
fn state_new_opens_left_focused_content_top() {
    let state = SubagentCompareState::new(100, 200);
    assert_eq!(state.left_id, 100);
    assert_eq!(state.right_id, 200);
    assert_eq!(state.focus, ComparePane::Pinned);
    assert_eq!(state.mode, CompareMode::Content);
    assert_eq!(state.left_scroll, 0);
    assert_eq!(state.right_scroll, 0);
}

#[test]
fn id_for_maps_panes_to_subagents() {
    let state = SubagentCompareState::new(100, 200);
    assert_eq!(state.id_for(ComparePane::Pinned), 100);
    assert_eq!(state.id_for(ComparePane::Compare), 200);
}

#[test]
fn focused_scroll_targets_the_active_pane_independently() {
    let mut state = SubagentCompareState::new(1, 2);
    // Left focused: setting the focused scroll moves left only.
    state.set_focused_scroll(5);
    assert_eq!(state.left_scroll, 5);
    assert_eq!(state.right_scroll, 0);
    assert_eq!(state.focused_scroll(), 5);

    // Flip focus to the right pane: its scroll is independent.
    state.focus = state.focus.toggled();
    assert_eq!(state.focus, ComparePane::Compare);
    assert_eq!(state.focused_scroll(), 0);
    state.set_focused_scroll(9);
    assert_eq!(state.right_scroll, 9);
    // Left pane's offset is untouched — the two scroll independently.
    assert_eq!(state.left_scroll, 5);
    assert_eq!(state.scroll_for(ComparePane::Pinned), 5);
    assert_eq!(state.scroll_for(ComparePane::Compare), 9);
}
