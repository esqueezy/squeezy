use super::*;

#[test]
fn new_store_is_empty() {
    let store = BookmarkStore::new();
    assert!(store.is_empty());
    assert_eq!(store.len(), 0);
    assert!(store.list().is_empty());
    assert!(store.next(None).is_none());
    assert!(store.prev(None).is_none());
}

#[test]
fn add_returns_distinct_ids_and_keeps_reading_order() {
    let mut store = BookmarkStore::new();
    // Added out of transcript order; the list must still read by entry id.
    let id_b = store.add(30, Some("b".to_string()));
    let id_a = store.add(10, Some("a".to_string()));
    let id_c = store.add(20, None);
    assert_ne!(id_a, id_b);
    assert_ne!(id_b, id_c);

    let ids: Vec<u64> = store.list().iter().map(|b| b.entry_id).collect();
    assert_eq!(ids, vec![10, 20, 30], "list is sorted by anchor entry id");
    // The handle still addresses the right bookmark after the re-sort.
    assert_eq!(store.index_of(id_a), Some(0));
    assert_eq!(store.index_of(id_c), Some(1));
    assert_eq!(store.index_of(id_b), Some(2));
}

#[test]
fn blank_name_normalises_to_anonymous() {
    let mut store = BookmarkStore::new();
    store.add(1, Some("   ".to_string()));
    store.add(2, Some("".to_string()));
    store.add(3, Some("  kept  ".to_string()));
    assert_eq!(store.get(0).unwrap().name, None, "whitespace -> anonymous");
    assert_eq!(store.get(1).unwrap().name, None, "empty -> anonymous");
    assert_eq!(
        store.get(2).unwrap().name.as_deref(),
        Some("kept"),
        "a real name is trimmed and kept",
    );
}

#[test]
fn display_name_falls_back_for_anonymous() {
    let mut store = BookmarkStore::new();
    store.add(1, None);
    store.add(2, Some("named".to_string()));
    assert_eq!(store.get(0).unwrap().display_name(), "\u{2014}");
    assert_eq!(store.get(1).unwrap().display_name(), "named");
}

#[test]
fn remove_by_id_and_missing_id_is_no_op() {
    let mut store = BookmarkStore::new();
    let id = store.add(5, None);
    assert!(store.remove(id), "removing a present bookmark succeeds");
    assert!(store.is_empty());
    assert!(!store.remove(id), "removing again is a no-op");
    assert!(!store.remove(9999), "removing an unknown id is a no-op");
}

#[test]
fn rename_updates_and_blank_clears_label() {
    let mut store = BookmarkStore::new();
    let id = store.add(5, Some("old".to_string()));
    assert!(store.rename(id, Some("new".to_string())));
    assert_eq!(store.get(0).unwrap().name.as_deref(), Some("new"));
    // A blank rename clears the label (turns it anonymous).
    assert!(store.rename(id, Some("   ".to_string())));
    assert_eq!(store.get(0).unwrap().name, None);
    // Renaming an unknown id is a no-op.
    assert!(!store.rename(9999, Some("x".to_string())));
}

#[test]
fn two_bookmarks_on_same_entry_break_ties_by_creation_order() {
    let mut store = BookmarkStore::new();
    let first = store.add(7, Some("first".to_string()));
    let second = store.add(7, Some("second".to_string()));
    let order: Vec<&str> = store.list().iter().map(|b| b.display_name()).collect();
    assert_eq!(
        order,
        vec!["first", "second"],
        "same-entry bookmarks keep creation order",
    );
    assert_eq!(store.index_of(first), Some(0));
    assert_eq!(store.index_of(second), Some(1));
}

#[test]
fn next_walks_forward_and_wraps() {
    let mut store = BookmarkStore::new();
    store.add(10, None);
    store.add(20, None);
    store.add(30, None);
    // Below the first bookmark -> first.
    assert_eq!(store.next(Some(5)).unwrap().entry_id, 10);
    // Between 10 and 20 -> 20.
    assert_eq!(store.next(Some(10)).unwrap().entry_id, 20);
    assert_eq!(store.next(Some(15)).unwrap().entry_id, 20);
    // At/after the last -> wrap to the first.
    assert_eq!(store.next(Some(30)).unwrap().entry_id, 10);
    assert_eq!(store.next(Some(99)).unwrap().entry_id, 10);
    // No reading position yet -> first bookmark.
    assert_eq!(store.next(None).unwrap().entry_id, 10);
}

#[test]
fn prev_walks_backward_and_wraps() {
    let mut store = BookmarkStore::new();
    store.add(10, None);
    store.add(20, None);
    store.add(30, None);
    // Above the last bookmark -> last.
    assert_eq!(store.prev(Some(99)).unwrap().entry_id, 30);
    // Between 20 and 30 -> 20.
    assert_eq!(store.prev(Some(30)).unwrap().entry_id, 20);
    assert_eq!(store.prev(Some(25)).unwrap().entry_id, 20);
    // At/before the first -> wrap to the last.
    assert_eq!(store.prev(Some(10)).unwrap().entry_id, 30);
    assert_eq!(store.prev(Some(1)).unwrap().entry_id, 30);
    // No reading position yet -> last bookmark.
    assert_eq!(store.prev(None).unwrap().entry_id, 30);
}

#[test]
fn next_prev_on_empty_store_is_none() {
    let store = BookmarkStore::new();
    assert!(store.next(Some(5)).is_none());
    assert!(store.prev(Some(5)).is_none());
}

#[test]
fn cap_evicts_oldest_by_creation() {
    let mut store = BookmarkStore::new();
    // Add more than the cap, entry ids descending so creation order != sort
    // order — the eviction must use creation order, not list position.
    for i in 0..(BOOKMARK_CAP as u64 + 5) {
        // Descending entry ids: newest creations have the smallest entry id.
        store.add(1000 - i, None);
    }
    assert_eq!(store.len(), BOOKMARK_CAP, "list is capped");
    // The five oldest creations (largest entry ids, 1000..=996) were evicted.
    let max_entry = store.list().iter().map(|b| b.entry_id).max().unwrap();
    assert!(
        max_entry <= 1000 - 5,
        "oldest-by-creation bookmarks (largest entry ids) were dropped, max={max_entry}",
    );
}

#[test]
fn index_of_unknown_id_is_none() {
    let mut store = BookmarkStore::new();
    let id = store.add(1, None);
    assert_eq!(store.index_of(id), Some(0));
    assert_eq!(store.index_of(id + 999), None);
}

#[test]
fn next_after_remove_uses_remaining_anchors() {
    // Removing a bookmark must not strand next/prev on the gone anchor.
    let mut store = BookmarkStore::new();
    store.add(10, None);
    let mid = store.add(20, None);
    store.add(30, None);
    assert!(store.remove(mid));
    // From entry 10, the next remaining bookmark is now 30 (20 is gone).
    assert_eq!(store.next(Some(10)).unwrap().entry_id, 30);
    // From entry 30, prev wraps back to 10.
    assert_eq!(store.prev(Some(30)).unwrap().entry_id, 10);
}
