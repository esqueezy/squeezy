//! Unit tests for the Inline Rename Labels store and edit primitive (§12.1.7).
//! Pure, terminal-free coverage of create/edit/cancel/clear/persist, the
//! normalise/collapse/cap rules, the in-place edit buffer, the badge preview
//! truncation, and the stable-key contract (entry vs queue-item targets are
//! independent and survive in the same map).

use super::*;

#[test]
fn normalise_trims_collapses_and_clamps() {
    assert_eq!(
        normalise_label("  the auth   refactor  "),
        Some("the auth refactor".to_string())
    );
    // A blank / whitespace-only label normalises to None (a clear).
    assert_eq!(normalise_label("   "), None);
    assert_eq!(normalise_label(""), None);
    // Over-length is clamped to the char limit.
    let long = "x".repeat(LABEL_TEXT_LIMIT + 20);
    let normalised = normalise_label(&long).expect("non-blank");
    assert_eq!(normalised.chars().count(), LABEL_TEXT_LIMIT);
}

#[test]
fn set_creates_then_replaces_label() {
    let mut store = LabelStore::new();
    assert!(store.is_empty());
    let target = LabelTargetId::Entry(7);
    assert!(store.set(target, "broken test"));
    assert_eq!(store.label_for(target), Some("broken test"));
    assert!(store.has_label(target));
    assert_eq!(store.len(), 1);
    // Re-setting replaces text in place (does not stack).
    assert!(store.set(target, "fixed test"));
    assert_eq!(store.label_for(target), Some("fixed test"));
    assert_eq!(store.len(), 1, "replace does not add a second label");
}

#[test]
fn blank_set_clears_existing_and_is_a_noop_when_absent() {
    let mut store = LabelStore::new();
    let target = LabelTargetId::Entry(1);
    store.set(target, "keep me");
    // A blank set clears the existing label and reports "nothing stored".
    assert!(!store.set(target, "   "));
    assert!(!store.has_label(target));
    assert!(store.is_empty());
    // A blank set on an absent target is a harmless no-op.
    assert!(!store.set(LabelTargetId::Entry(99), ""));
    assert!(store.is_empty());
}

#[test]
fn clear_removes_only_the_named_target() {
    let mut store = LabelStore::new();
    store.set(LabelTargetId::Entry(1), "a");
    store.set(LabelTargetId::Entry(2), "b");
    assert!(store.clear(LabelTargetId::Entry(1)));
    assert!(!store.has_label(LabelTargetId::Entry(1)));
    assert_eq!(store.label_for(LabelTargetId::Entry(2)), Some("b"));
    // Clearing an absent target reports false.
    assert!(!store.clear(LabelTargetId::Entry(1)));
}

#[test]
fn entry_and_queue_targets_are_independent_keys() {
    // The same numeric value under different target kinds is a different label —
    // the stable-key contract that lets entries and queue items coexist.
    let mut store = LabelStore::new();
    store.set(LabelTargetId::Entry(3), "entry three");
    store.set(LabelTargetId::QueueItem(3), "queued three");
    assert_eq!(
        store.label_for(LabelTargetId::Entry(3)),
        Some("entry three")
    );
    assert_eq!(
        store.label_for(LabelTargetId::QueueItem(3)),
        Some("queued three"),
    );
    assert_eq!(store.len(), 2);
}

#[test]
fn cap_evicts_oldest_by_creation() {
    let mut store = LabelStore::new();
    for i in 0..(LABEL_CAP as u64 + 5) {
        store.set(LabelTargetId::Entry(i), &format!("label {i}"));
    }
    assert_eq!(store.len(), LABEL_CAP, "stays at the cap");
    // The oldest five (entries 0..5) were evicted; the newest survive.
    assert!(!store.has_label(LabelTargetId::Entry(0)));
    assert!(!store.has_label(LabelTargetId::Entry(4)));
    assert!(store.has_label(LabelTargetId::Entry(LABEL_CAP as u64 + 4)));
}

#[test]
fn replacing_text_does_not_change_eviction_order() {
    // Editing an existing label keeps its creation order, so a re-edit of the
    // OLDEST label does not save it from being the next evicted.
    let mut store = LabelStore::new();
    for i in 0..LABEL_CAP as u64 {
        store.set(LabelTargetId::Entry(i), &format!("l{i}"));
    }
    // Re-edit the oldest (entry 0) — its order must NOT move to newest.
    store.set(LabelTargetId::Entry(0), "edited oldest");
    // Add one more past the cap: entry 0 (still oldest by creation) is evicted.
    store.set(LabelTargetId::Entry(LABEL_CAP as u64), "newest");
    assert_eq!(store.len(), LABEL_CAP);
    assert!(
        !store.has_label(LabelTargetId::Entry(0)),
        "re-editing text must not protect the oldest label from eviction",
    );
}

#[test]
fn inline_edit_buffer_push_and_backspace_respect_the_limit() {
    let mut edit = InlineEditState::new(LabelTargetId::Entry(1), String::new(), true);
    assert!(edit.is_new);
    assert!(edit.push('h'));
    assert!(edit.push('i'));
    assert_eq!(edit.buffer, "hi");
    assert!(edit.backspace());
    assert_eq!(edit.buffer, "h");
    // Backspace on an empty buffer reports no change.
    assert!(edit.backspace());
    assert!(!edit.backspace());
    // Pushing past the limit reports no change and does not grow the buffer.
    let mut full =
        InlineEditState::new(LabelTargetId::Entry(2), "x".repeat(LABEL_TEXT_LIMIT), false);
    assert!(!full.push('y'));
    assert_eq!(full.buffer.chars().count(), LABEL_TEXT_LIMIT);
}

#[test]
fn badge_preview_truncates_long_labels_on_char_boundary() {
    assert_eq!(badge_preview("short", 10), "short");
    let truncated = badge_preview("a very long label that overflows", 8);
    assert_eq!(truncated.chars().count(), 8);
    assert!(truncated.ends_with('\u{2026}'));
    // A zero budget yields an empty preview without panicking.
    assert_eq!(badge_preview("anything", 0), "");
    // Multi-byte text never panics and clamps on a char boundary.
    let multi = badge_preview("héllo wörld ünïcode", 6);
    assert_eq!(multi.chars().count(), 6);
}

#[test]
fn labels_persist_across_store_clone() {
    // The store is the session-UI-metadata home; a clone (e.g. snapshotting the
    // session) carries the labels, proving they are plain owned data.
    let mut store = LabelStore::new();
    store.set(LabelTargetId::Entry(5), "carried over");
    let resumed = store.clone();
    assert_eq!(
        resumed.label_for(LabelTargetId::Entry(5)),
        Some("carried over")
    );
}
