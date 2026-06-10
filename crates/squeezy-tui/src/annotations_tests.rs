use super::*;

#[test]
fn new_store_is_empty() {
    let store = AnnotationStore::new();
    assert!(store.is_empty());
    assert_eq!(store.len(), 0);
    assert!(store.list().is_empty());
    assert!(store.next(None).is_none());
    assert!(store.prev(None).is_none());
    assert!(!store.annotated(1));
    assert_eq!(store.count_for_entry(1), 0);
}

#[test]
fn add_returns_distinct_ids_and_keeps_reading_order() {
    let mut store = AnnotationStore::new();
    // Added out of transcript order; the list must still read by entry id.
    let id_b = store.add(30, "b").expect("note added");
    let id_a = store.add(10, "a").expect("note added");
    let id_c = store.add(20, "c").expect("note added");
    assert_ne!(id_a, id_b);
    assert_ne!(id_b, id_c);

    let ids: Vec<u64> = store.list().iter().map(|a| a.entry_id).collect();
    assert_eq!(ids, vec![10, 20, 30], "list is sorted by anchor entry id");
    // The handle still addresses the right annotation after the re-sort.
    assert_eq!(store.index_of(id_a), Some(0));
    assert_eq!(store.index_of(id_c), Some(1));
    assert_eq!(store.index_of(id_b), Some(2));
}

#[test]
fn blank_note_is_rejected() {
    let mut store = AnnotationStore::new();
    assert_eq!(store.add(1, "   "), None, "whitespace note is not added");
    assert_eq!(store.add(1, ""), None, "empty note is not added");
    assert!(store.is_empty());
    // A real note is trimmed and kept.
    assert!(store.add(1, "  kept  ").is_some());
    assert_eq!(store.get(0).unwrap().text, "kept", "note is trimmed");
}

#[test]
fn text_is_clamped_to_the_limit_on_a_char_boundary() {
    let mut store = AnnotationStore::new();
    // A multi-byte char repeated past the limit must clamp without panicking.
    let long: String = "\u{00e9}".repeat(ANNOTATION_TEXT_LIMIT + 50);
    store.add(1, &long).expect("clamped note added");
    assert_eq!(
        store.get(0).unwrap().text.chars().count(),
        ANNOTATION_TEXT_LIMIT,
        "text is clamped to the char limit",
    );
}

#[test]
fn preview_clamps_to_first_line_with_ellipsis() {
    let mut store = AnnotationStore::new();
    store
        .add(1, "first line\nsecond line is hidden")
        .expect("note added");
    // Only the first physical line, and short enough to keep whole.
    assert_eq!(store.get(0).unwrap().preview(40), "first line");

    store.add(2, "abcdefghij").expect("note added");
    let preview = store.get(1).unwrap().preview(5);
    assert_eq!(preview.chars().count(), 5, "preview fits the budget");
    assert!(
        preview.ends_with('\u{2026}'),
        "long preview gets an ellipsis"
    );
    assert!(preview.starts_with("abcd"), "preview keeps the prefix");
}

#[test]
fn remove_by_id_and_missing_id_is_no_op() {
    let mut store = AnnotationStore::new();
    let id = store.add(5, "note").expect("note added");
    assert!(store.remove(id), "removing a present annotation succeeds");
    assert!(store.is_empty());
    assert!(!store.remove(id), "removing again is a no-op");
    assert!(!store.remove(9999), "removing an unknown id is a no-op");
}

#[test]
fn set_text_updates_and_blank_deletes() {
    let mut store = AnnotationStore::new();
    let id = store.add(5, "old").expect("note added");
    assert!(store.set_text(id, "new"));
    assert_eq!(store.get(0).unwrap().text, "new");
    // A blank edit deletes the annotation.
    assert!(store.set_text(id, "   "), "blank edit reports success");
    assert!(store.is_empty(), "blank edit deletes the annotation");
    // Editing an unknown id is a no-op.
    assert!(!store.set_text(9999, "x"));
}

#[test]
fn annotated_and_count_track_presence() {
    let mut store = AnnotationStore::new();
    assert!(!store.annotated(7));
    store.add(7, "one").expect("note added");
    store.add(7, "two").expect("note added");
    store.add(9, "elsewhere").expect("note added");
    assert!(store.annotated(7));
    assert!(store.annotated(9));
    assert!(!store.annotated(8));
    assert_eq!(store.count_for_entry(7), 2, "two notes on entry 7");
    assert_eq!(store.count_for_entry(9), 1);
    assert_eq!(store.count_for_entry(8), 0);
    // The first index for an entry addresses the earliest-created note there.
    assert_eq!(store.first_index_for_entry(7), Some(0));
    assert_eq!(store.first_index_for_entry(9), Some(2));
    assert_eq!(store.first_index_for_entry(8), None);
}

#[test]
fn two_annotations_on_same_entry_break_ties_by_creation_order() {
    let mut store = AnnotationStore::new();
    let first = store.add(7, "first").expect("note added");
    let second = store.add(7, "second").expect("note added");
    let order: Vec<&str> = store.list().iter().map(|a| a.text.as_str()).collect();
    assert_eq!(
        order,
        vec!["first", "second"],
        "same-entry annotations keep creation order",
    );
    assert_eq!(store.index_of(first), Some(0));
    assert_eq!(store.index_of(second), Some(1));
}

#[test]
fn next_walks_forward_and_wraps() {
    let mut store = AnnotationStore::new();
    store.add(10, "a").unwrap();
    store.add(20, "b").unwrap();
    store.add(30, "c").unwrap();
    // Below the first annotation -> first.
    assert_eq!(store.next(Some(5)).unwrap().entry_id, 10);
    // Between 10 and 20 -> 20.
    assert_eq!(store.next(Some(10)).unwrap().entry_id, 20);
    assert_eq!(store.next(Some(15)).unwrap().entry_id, 20);
    // At/after the last -> wrap to the first.
    assert_eq!(store.next(Some(30)).unwrap().entry_id, 10);
    assert_eq!(store.next(Some(99)).unwrap().entry_id, 10);
    // No reading position yet -> first annotation.
    assert_eq!(store.next(None).unwrap().entry_id, 10);
}

#[test]
fn prev_walks_backward_and_wraps() {
    let mut store = AnnotationStore::new();
    store.add(10, "a").unwrap();
    store.add(20, "b").unwrap();
    store.add(30, "c").unwrap();
    // Above the last annotation -> last.
    assert_eq!(store.prev(Some(99)).unwrap().entry_id, 30);
    // Between 20 and 30 -> 20.
    assert_eq!(store.prev(Some(30)).unwrap().entry_id, 20);
    assert_eq!(store.prev(Some(25)).unwrap().entry_id, 20);
    // At/before the first -> wrap to the last.
    assert_eq!(store.prev(Some(10)).unwrap().entry_id, 30);
    assert_eq!(store.prev(Some(1)).unwrap().entry_id, 30);
    // No reading position yet -> last annotation.
    assert_eq!(store.prev(None).unwrap().entry_id, 30);
}

#[test]
fn next_prev_on_empty_store_is_none() {
    let store = AnnotationStore::new();
    assert!(store.next(Some(5)).is_none());
    assert!(store.prev(Some(5)).is_none());
}

#[test]
fn cap_evicts_oldest_by_creation() {
    let mut store = AnnotationStore::new();
    // Add more than the cap, entry ids descending so creation order != sort
    // order — the eviction must use creation order, not list position.
    for i in 0..(ANNOTATION_CAP as u64 + 5) {
        // Descending entry ids: newest creations have the smallest entry id.
        store.add(1000 - i, "note").expect("note added");
    }
    assert_eq!(store.len(), ANNOTATION_CAP, "list is capped");
    // The five oldest creations (largest entry ids, 1000..=996) were evicted.
    let max_entry = store.list().iter().map(|a| a.entry_id).max().unwrap();
    assert!(
        max_entry <= 1000 - 5,
        "oldest-by-creation annotations (largest entry ids) were dropped, max={max_entry}",
    );
}

#[test]
fn index_of_unknown_id_is_none() {
    let mut store = AnnotationStore::new();
    let id = store.add(1, "note").expect("note added");
    assert_eq!(store.index_of(id), Some(0));
    assert_eq!(store.index_of(id + 999), None);
}

#[test]
fn next_after_remove_uses_remaining_anchors() {
    // Removing an annotation must not strand next/prev on the gone anchor.
    let mut store = AnnotationStore::new();
    store.add(10, "a").unwrap();
    let mid = store.add(20, "b").unwrap();
    store.add(30, "c").unwrap();
    assert!(store.remove(mid));
    // From entry 10, the next remaining annotation is now 30 (20 is gone).
    assert_eq!(store.next(Some(10)).unwrap().entry_id, 30);
    // From entry 30, prev wraps back to 10.
    assert_eq!(store.prev(Some(30)).unwrap().entry_id, 10);
}
