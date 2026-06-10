//! Reading Position Bookmarks (§12.2.4 / backlog 12.2.4): drop named or
//! anonymous bookmarks at a transcript position and jump between them.
//!
//! Bookmarks are deliberately distinct from jump marks ([`crate::jump_marks`]).
//! A jump mark is a transient LIFO "remember where I was" stack that drains as
//! you pop it; a bookmark is a **durable reading-position anchor** that stays put
//! across appends, resize, folds, and filters until the user deletes it. The
//! spec frames them as semantic anchors, not terminal rows, so each bookmark is
//! keyed by a stable [`TranscriptEntry::id`](crate) — the one identity that
//! survives every reflow. A row offset would go stale the instant the transcript
//! grows; the entry id does not.
//!
//! This module is intentionally pure: it owns the bookmark list and the
//! ordering/navigation math (add, remove, rename, list, next, previous) and
//! nothing about geometry, rendering, or input. `lib.rs` captures the current
//! top-visible entry id, feeds it in, and turns the entry id the store hands back
//! into a scroll target via the same `jump_to_entry_id` path the jump marks use.
//! That keeps the navigation testable without a terminal.
//!
//! Bookmarks are ordered by their anchor entry id. Entry ids are allocated
//! monotonically in creation order, so ordering by id is a faithful proxy for
//! transcript order: `next`/`previous` walk the list in the same top-to-bottom
//! order the user reads, independent of any later reflow.

/// Largest number of bookmarks retained. Bookmarks are a lightweight
/// reading-position aid, not a database; a small, predictable cap keeps the list
/// overlay scannable and `next`/`previous` cycling meaningful. Adding past the
/// cap drops the oldest-by-creation bookmark so the newest intent always lands.
pub(crate) const BOOKMARK_CAP: usize = 64;

/// A single reading-position bookmark anchored to a stable transcript entry id.
///
/// `id` is a per-store sequential handle (never reused within a store) used to
/// address the bookmark for rename/delete and to register a stable click target;
/// it is independent of `entry_id`, so two bookmarks may anchor the same entry.
/// `name` is the optional user label (`None` = an anonymous bookmark, shown by
/// its anchor). `order` is the creation sequence, retained so the list can break
/// ties (two bookmarks on the same entry) deterministically and so the cap drops
/// the oldest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bookmark {
    /// Stable per-store handle for this bookmark (addresses rename/delete/click).
    pub(crate) id: u64,
    /// The anchored transcript entry's stable id (`TranscriptEntry::id`).
    pub(crate) entry_id: u64,
    /// Optional user label. `None` is an anonymous bookmark.
    pub(crate) name: Option<String>,
    /// Creation sequence, oldest first. Used for tie-breaks and cap eviction.
    pub(crate) order: u64,
}

impl Bookmark {
    /// A compact display label: the user name when set, else an em-dash
    /// placeholder so an anonymous bookmark still reads as a row rather than a
    /// blank. Never empty.
    pub(crate) fn display_name(&self) -> &str {
        match &self.name {
            Some(name) if !name.trim().is_empty() => name.as_str(),
            _ => "\u{2014}",
        }
    }
}

/// Pure bookmark bookkeeping over stable entry ids.
///
/// The list is kept sorted by `(entry_id, order)` so iteration, `next`, and
/// `previous` all walk in transcript-reading order with a deterministic tie-break
/// when two bookmarks share an entry. The resting state is an empty `Vec`, so a
/// session that never bookmarks anything costs nothing.
#[derive(Debug, Clone, Default)]
pub(crate) struct BookmarkStore {
    bookmarks: Vec<Bookmark>,
    next_id: u64,
    next_order: u64,
}

impl BookmarkStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Drop a bookmark anchored at `entry_id` with the optional `name`. Returns
    /// the new bookmark's stable id. A blank/whitespace-only name is normalised
    /// to an anonymous bookmark (`None`) so the list never shows an empty label.
    /// Keeps the list sorted by `(entry_id, order)`; exceeding [`BOOKMARK_CAP`]
    /// evicts the oldest-by-creation bookmark.
    pub(crate) fn add(&mut self, entry_id: u64, name: Option<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let order = self.next_order;
        self.next_order += 1;
        let name = name.and_then(|n| {
            let trimmed = n.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        });
        self.bookmarks.push(Bookmark {
            id,
            entry_id,
            name,
            order,
        });
        self.sort();
        // Evict oldest-by-creation while over the cap.
        while self.bookmarks.len() > BOOKMARK_CAP {
            if let Some((idx, _)) = self
                .bookmarks
                .iter()
                .enumerate()
                .min_by_key(|(_, b)| b.order)
            {
                self.bookmarks.remove(idx);
            } else {
                break;
            }
        }
        id
    }

    /// Remove the bookmark with the given stable id. Returns `true` when one was
    /// removed, `false` when no bookmark carried that id.
    pub(crate) fn remove(&mut self, id: u64) -> bool {
        let before = self.bookmarks.len();
        self.bookmarks.retain(|b| b.id != id);
        self.bookmarks.len() != before
    }

    /// Rename the bookmark with the given stable id. A blank name clears the
    /// label (turns it anonymous). Returns `true` when a bookmark was found and
    /// updated. Re-sorting is unnecessary — a rename never changes the
    /// `(entry_id, order)` sort key.
    pub(crate) fn rename(&mut self, id: u64, name: Option<String>) -> bool {
        let name = name.and_then(|n| {
            let trimmed = n.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        });
        if let Some(bookmark) = self.bookmarks.iter_mut().find(|b| b.id == id) {
            bookmark.name = name;
            true
        } else {
            false
        }
    }

    /// The bookmarks in transcript-reading order (by `(entry_id, order)`).
    pub(crate) fn list(&self) -> &[Bookmark] {
        &self.bookmarks
    }

    /// Number of bookmarks currently stored.
    pub(crate) fn len(&self) -> usize {
        self.bookmarks.len()
    }

    /// Whether the store holds no bookmarks.
    pub(crate) fn is_empty(&self) -> bool {
        self.bookmarks.is_empty()
    }

    /// The bookmark at list index `index` (in reading order), if any.
    pub(crate) fn get(&self, index: usize) -> Option<&Bookmark> {
        self.bookmarks.get(index)
    }

    /// The list index of the bookmark with the given stable id, if present.
    pub(crate) fn index_of(&self, id: u64) -> Option<usize> {
        self.bookmarks.iter().position(|b| b.id == id)
    }

    /// The next bookmark strictly *after* the given reading position, for
    /// "jump to next bookmark". `from_entry_id` is the entry id currently at the
    /// top of the viewport (`None` when nothing is anchored, e.g. an empty
    /// transcript). Returns the first bookmark whose `entry_id` is greater than
    /// `from_entry_id`, wrapping to the first bookmark when already at/after the
    /// last — so repeated presses cycle forward through every bookmark. `None`
    /// only when the store is empty.
    pub(crate) fn next(&self, from_entry_id: Option<u64>) -> Option<&Bookmark> {
        if self.bookmarks.is_empty() {
            return None;
        }
        match from_entry_id {
            Some(from) => self
                .bookmarks
                .iter()
                .find(|b| b.entry_id > from)
                .or_else(|| self.bookmarks.first()),
            None => self.bookmarks.first(),
        }
    }

    /// The previous bookmark strictly *before* the given reading position, for
    /// "jump to previous bookmark". Mirror of [`next`](Self::next): returns the
    /// last bookmark whose `entry_id` is less than `from_entry_id`, wrapping to
    /// the last bookmark when already at/before the first. `None` only when the
    /// store is empty.
    pub(crate) fn prev(&self, from_entry_id: Option<u64>) -> Option<&Bookmark> {
        if self.bookmarks.is_empty() {
            return None;
        }
        match from_entry_id {
            Some(from) => self
                .bookmarks
                .iter()
                .rev()
                .find(|b| b.entry_id < from)
                .or_else(|| self.bookmarks.last()),
            None => self.bookmarks.last(),
        }
    }

    fn sort(&mut self) {
        self.bookmarks
            .sort_by(|a, b| a.entry_id.cmp(&b.entry_id).then(a.order.cmp(&b.order)));
    }
}

#[cfg(test)]
#[path = "bookmarks_tests.rs"]
mod tests;
