use std::collections::VecDeque;

use super::*;

fn queue_of(items: &[&str]) -> VecDeque<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn ids_of(ids: &[u64]) -> VecDeque<u64> {
    ids.iter().copied().collect()
}

#[test]
fn pick_edit_returns_id_and_text_at_selected() {
    let queue = queue_of(&["alpha", "beta", "gamma"]);
    let ids = ids_of(&[10, 11, 12]);
    let pick = pick_edit(&queue, &ids, 1).expect("row in range");
    assert_eq!(
        pick,
        EditPick {
            id: 11,
            text: "beta".to_string()
        }
    );
}

#[test]
fn pick_edit_empty_queue_is_none() {
    let queue: VecDeque<String> = VecDeque::new();
    let ids: VecDeque<u64> = VecDeque::new();
    assert_eq!(pick_edit(&queue, &ids, 0), None);
}

#[test]
fn pick_edit_out_of_range_cursor_is_none() {
    let queue = queue_of(&["a"]);
    let ids = ids_of(&[7]);
    // A stale cursor past the end resolves to nothing rather than panicking.
    assert_eq!(pick_edit(&queue, &ids, 5), None);
}

#[test]
fn apply_edit_replaces_text_in_place_keeping_position_and_id() {
    let mut queue = queue_of(&["a", "b", "c"]);
    let ids = ids_of(&[0, 1, 2]);
    let result = apply_edit(&mut queue, &ids, 1, "B-edited".to_string());
    assert_eq!(result, EditResult::Updated);
    // Only the targeted slot changed; its neighbours and order are untouched.
    assert_eq!(
        queue.iter().cloned().collect::<Vec<_>>(),
        vec!["a", "B-edited", "c"]
    );
}

#[test]
fn apply_edit_resolves_by_id_not_position_after_reorder() {
    // The item we edited started at index 0 but has since moved to index 2.
    // Saving must still land on it (by id), not on whatever now sits at 0.
    let mut queue = queue_of(&["b", "c", "a"]);
    let ids = ids_of(&[1, 2, 0]); // id 0 ("a") drifted to the tail
    let result = apply_edit(&mut queue, &ids, 0, "a-edited".to_string());
    assert_eq!(result, EditResult::Updated);
    assert_eq!(
        queue.iter().cloned().collect::<Vec<_>>(),
        vec!["b", "c", "a-edited"]
    );
}

#[test]
fn apply_edit_vanished_id_leaves_queue_untouched() {
    let mut queue = queue_of(&["a", "b"]);
    let ids = ids_of(&[0, 1]);
    // id 99 drained out / was deleted — no slot to update.
    let result = apply_edit(&mut queue, &ids, 99, "ghost".to_string());
    assert_eq!(result, EditResult::Vanished);
    assert_eq!(queue.iter().cloned().collect::<Vec<_>>(), vec!["a", "b"]);
}

#[test]
fn apply_edit_to_empty_queue_vanishes() {
    let mut queue: VecDeque<String> = VecDeque::new();
    let ids: VecDeque<u64> = VecDeque::new();
    assert_eq!(
        apply_edit(&mut queue, &ids, 0, "x".to_string()),
        EditResult::Vanished
    );
    assert!(queue.is_empty());
}
