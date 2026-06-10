//! Edit Queued Prompt (§11G.8).
//!
//! While the prompt-queue overlay is open, `Enter` or `e` on the focused
//! queued prompt — and a mouse double-click on its row — opens that prompt in
//! the composer for editing. The item being edited is tracked by its *stable
//! id* (`TuiApp::editing_queue_id`), never its Vec position, so a concurrent
//! front-drain or reorder between "begin edit" and "save" can never make the
//! save land on the wrong row. On save the live queue is searched for that id
//! and its text replaced in place; if the id has since drained out (the prompt
//! already ran), the edit falls back to a normal enqueue so the user's text is
//! never silently lost.
//!
//! This module is the *pure-state* surface: it owns nothing and depends only on
//! the two queue deques (`prompt_queue` + its `prompt_queue_ids` sidecar). The
//! id sidecar invariant (`ids.len() == queue.len()`, front id ↔ front prompt) is
//! the caller's contract — `lib.rs::sync_queue_ids` re-establishes it before any
//! of these helpers run — so every function here is a pure, terminal-free,
//! unit-testable function over model state.

use std::collections::VecDeque;

/// The prompt the user just asked to edit: its stable id plus a snapshot of its
/// text. Returned by [`pick_edit`]; the caller loads `text` into the composer
/// and stashes `id` on `TuiApp::editing_queue_id` so the eventual save resolves
/// back to the live row by id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditPick {
    /// Stable id of the queued item being edited.
    pub(crate) id: u64,
    /// The item's text at the moment editing began.
    pub(crate) text: String,
}

/// Outcome of saving an edit back onto the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditResult {
    /// The item with the tracked id was found and its text replaced in place.
    Updated,
    /// The id was no longer present (the prompt already drained/ran, or was
    /// deleted). The caller falls back to a normal enqueue so the text survives.
    Vanished,
}

/// Resolve the overlay's focused row (`selected`) to the queued item to edit.
///
/// Returns `None` when the queue is empty or `selected` is out of range (a
/// transient stale cursor) — the caller then no-ops rather than editing a
/// phantom row. The id is read from `ids` at the same index, so the snapshot
/// (id + text) names exactly one item for its whole edit lifetime.
pub(crate) fn pick_edit(
    queue: &VecDeque<String>,
    ids: &VecDeque<u64>,
    selected: usize,
) -> Option<EditPick> {
    let text = queue.get(selected)?.clone();
    let id = *ids.get(selected)?;
    Some(EditPick { id, text })
}

/// Save edited `new_text` back onto the queued item identified by stable `id`.
///
/// Resolves the id to its *current* index (so a front-drain or reorder since
/// editing began cannot stale the position) and replaces that slot's text in
/// place, leaving the id sidecar untouched (the item keeps its identity, hit
/// target, and queue position). Returns [`EditResult::Vanished`] without
/// mutating anything when the id is gone, so the caller can fall back to a
/// fresh enqueue.
pub(crate) fn apply_edit(
    queue: &mut VecDeque<String>,
    ids: &VecDeque<u64>,
    id: u64,
    new_text: String,
) -> EditResult {
    let Some(index) = ids.iter().position(|qid| *qid == id) else {
        return EditResult::Vanished;
    };
    // The sidecar invariant guarantees a matching slot; guard anyway so a
    // momentarily-desynced sidecar degrades to a no-update rather than a panic.
    let Some(slot) = queue.get_mut(index) else {
        return EditResult::Vanished;
    };
    *slot = new_text;
    EditResult::Updated
}

#[cfg(test)]
#[path = "queue_edit_tests.rs"]
mod tests;
