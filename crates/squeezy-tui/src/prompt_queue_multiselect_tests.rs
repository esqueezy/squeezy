use std::collections::BTreeSet;

use super::*;

fn set_of(ids: &[u64]) -> BTreeSet<u64> {
    ids.iter().copied().collect()
}

#[test]
fn toggle_adds_then_removes() {
    let mut ms = MultiSelect::new();
    assert!(ms.is_empty());
    assert!(ms.toggle(7)); // returns "added"
    assert!(ms.contains(7));
    assert_eq!(ms.len(), 1);
    assert!(!ms.toggle(7)); // returns "removed"
    assert!(!ms.contains(7));
    assert!(ms.is_empty());
}

#[test]
fn select_all_is_idempotent_and_clear_empties() {
    let mut ms = MultiSelect::new();
    let ids = [1, 2, 3];
    ms.select_all(&ids);
    ms.select_all(&ids); // idempotent: no duplicates
    assert_eq!(ms.len(), 3);
    assert!(ids.iter().all(|id| ms.contains(*id)));
    ms.clear();
    assert!(ms.is_empty());
}

#[test]
fn retain_live_drops_drained_ids() {
    let mut ms = MultiSelect::new();
    ms.select_all(&[1, 2, 3]);
    // 1 drained out of the queue; only 2 and 3 remain live.
    ms.retain_live(&[2, 3, 4]);
    assert!(!ms.contains(1));
    assert!(ms.contains(2));
    assert!(ms.contains(3));
    assert_eq!(ms.len(), 2);
}

#[test]
fn ids_in_queue_order_follows_live_order_not_insertion_order() {
    let mut ms = MultiSelect::new();
    // Tag in a scrambled order...
    ms.toggle(3);
    ms.toggle(1);
    // ...but the queue's live order is 1, 2, 3, so the result is 1, 3.
    let live = [1, 2, 3];
    assert_eq!(ms.ids_in_queue_order(&live), vec![1, 3]);
}

#[test]
fn ids_in_queue_order_filters_stale_ids() {
    let mut ms = MultiSelect::new();
    ms.select_all(&[1, 9]); // 9 is no longer in the queue
    let live = [1, 2, 3];
    assert_eq!(ms.ids_in_queue_order(&live), vec![1]);
}

#[test]
fn marker_glyph_and_span_differ_by_state() {
    assert_eq!(marker_glyph(true), "[x]");
    assert_eq!(marker_glyph(false), "[ ]");
    // The span content mirrors the glyph (the colour is theme-driven).
    assert_eq!(marker_span(true).content.as_ref(), "[x]");
    assert_eq!(marker_span(false).content.as_ref(), "[ ]");
}

// ---- move_group: the pure block-move math --------------------------------

#[test]
fn move_group_empty_selection_is_noop() {
    let ids = [1, 2, 3];
    assert_eq!(move_group(&ids, &set_of(&[]), MoveDir::Up), None);
    assert_eq!(move_group(&ids, &set_of(&[]), MoveDir::Down), None);
}

#[test]
fn move_group_single_item_up_and_down() {
    let ids = [1, 2, 3, 4];
    // Move id 3 (index 2) up one -> 1, 3, 2, 4.
    assert_eq!(
        move_group(&ids, &set_of(&[3]), MoveDir::Up),
        Some(vec![1, 3, 2, 4])
    );
    // Move id 3 (index 2) down one -> 1, 2, 4, 3.
    assert_eq!(
        move_group(&ids, &set_of(&[3]), MoveDir::Down),
        Some(vec![1, 2, 4, 3])
    );
}

#[test]
fn move_group_contiguous_block_moves_as_unit() {
    let ids = [1, 2, 3, 4, 5];
    // Tag 2 and 3 (a contiguous block), move up -> 2, 3, 1, 4, 5.
    assert_eq!(
        move_group(&ids, &set_of(&[2, 3]), MoveDir::Up),
        Some(vec![2, 3, 1, 4, 5])
    );
    // Same block, move down -> 1, 4, 2, 3, 5.
    assert_eq!(
        move_group(&ids, &set_of(&[2, 3]), MoveDir::Down),
        Some(vec![1, 4, 2, 3, 5])
    );
}

#[test]
fn move_group_scattered_block_preserves_relative_order() {
    let ids = [1, 2, 3, 4, 5];
    // Tag 2 and 4 (scattered). Up: each slides past its unselected upper
    // neighbour -> 2, 1, 4, 3, 5.
    assert_eq!(
        move_group(&ids, &set_of(&[2, 4]), MoveDir::Up),
        Some(vec![2, 1, 4, 3, 5])
    );
    // Down: each slides past its unselected lower neighbour -> 1, 3, 2, 5, 4.
    assert_eq!(
        move_group(&ids, &set_of(&[2, 4]), MoveDir::Down),
        Some(vec![1, 3, 2, 5, 4])
    );
}

#[test]
fn move_group_blocked_at_boundary_is_noop() {
    let ids = [1, 2, 3];
    // Top item already at the front: up is a no-op.
    assert_eq!(move_group(&ids, &set_of(&[1]), MoveDir::Up), None);
    // Bottom item already at the back: down is a no-op.
    assert_eq!(move_group(&ids, &set_of(&[3]), MoveDir::Down), None);
    // Whole queue selected: neither direction can move.
    assert_eq!(move_group(&ids, &set_of(&[1, 2, 3]), MoveDir::Up), None);
    assert_eq!(move_group(&ids, &set_of(&[1, 2, 3]), MoveDir::Down), None);
}

#[test]
fn move_group_ignores_stale_selected_ids() {
    let ids = [1, 2, 3];
    // 9 is tagged but not in the queue; only the live tag (2) drives the move.
    assert_eq!(
        move_group(&ids, &set_of(&[2, 9]), MoveDir::Up),
        Some(vec![2, 1, 3])
    );
}

#[test]
fn move_group_is_a_permutation() {
    // Whatever the move, the result is always a permutation of the input
    // (no id lost or duplicated). Exhaustive over small selections.
    let ids = [10, 20, 30, 40];
    let want: BTreeSet<u64> = ids.iter().copied().collect();
    for mask in 1u8..(1 << ids.len()) {
        let sel: BTreeSet<u64> = (0..ids.len())
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| ids[i])
            .collect();
        for dir in [MoveDir::Up, MoveDir::Down] {
            if let Some(out) = move_group(&ids, &sel, dir) {
                let got: BTreeSet<u64> = out.iter().copied().collect();
                assert_eq!(got, want, "mask {mask} dir {dir:?} lost/dup an id");
                assert_eq!(out.len(), ids.len());
            }
        }
    }
}
