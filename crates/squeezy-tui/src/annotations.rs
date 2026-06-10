//! Entry Annotations (§12.2.5 / backlog 12.2.5): attach a short user note to a
//! transcript entry without polluting model context.
//!
//! An annotation is a private margin note the user pins to a turn, tool output,
//! or error. The spec frames the attachment as semantic, not positional, so each
//! annotation is keyed by a stable [`TranscriptEntry::id`](crate) — the one
//! identity that survives every reflow, resize, fold, and filter. A row offset
//! would go stale the instant the transcript grows; the entry id does not. The
//! note text lives only in this store (session UI metadata), never in the model
//! transcript, so it can never leak into context — the spec's core risk.
//!
//! This module is intentionally pure: it owns the annotation list and the
//! ordering/navigation math (add, remove, edit text, list, next, previous) and
//! nothing about geometry, rendering, or input. `lib.rs` captures the focused or
//! top-visible entry id, feeds it in, paints a small presence marker on the row
//! via [`annotated`](AnnotationStore::annotated), and turns the entry id the store
//! hands back into a scroll target via the same `jump_to_entry_id` path the jump
//! marks and bookmarks use. That keeps the CRUD + navigation testable without a
//! terminal.
//!
//! Annotations are ordered by their anchor entry id. Entry ids are allocated
//! monotonically in creation order, so ordering by id is a faithful proxy for
//! transcript order: `next`/`previous` walk the list in the same top-to-bottom
//! order the user reads, independent of any later reflow.

/// Largest number of annotations retained. Annotations are a lightweight margin
/// aid, not a database; a small, predictable cap keeps the list overlay scannable
/// and `next`/`previous` cycling meaningful. Adding past the cap drops the
/// oldest-by-creation annotation so the newest intent always lands.
pub(crate) const ANNOTATION_CAP: usize = 128;

/// Longest annotation note retained, in characters. The note is a short margin
/// remark, not an essay; clamping keeps the editor line and the list overlay
/// bounded and the badge presence check cheap. Text past the limit is truncated on
/// the character boundary (never mid-grapheme-byte) at write time.
pub(crate) const ANNOTATION_TEXT_LIMIT: usize = 280;

/// A single entry annotation anchored to a stable transcript entry id.
///
/// `id` is a per-store sequential handle (never reused within a store) used to
/// address the annotation for edit/delete and to register a stable click target;
/// it is independent of `entry_id`, so two annotations may anchor the same entry.
/// `text` is the user's note (never empty — an empty note is a delete). `order` is
/// the creation sequence, retained so the list can break ties (two annotations on
/// the same entry) deterministically and so the cap drops the oldest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Annotation {
    /// Stable per-store handle for this annotation (addresses edit/delete/click).
    pub(crate) id: u64,
    /// The anchored transcript entry's stable id (`TranscriptEntry::id`).
    pub(crate) entry_id: u64,
    /// The user's note. Trimmed and non-empty (a blank note deletes instead).
    pub(crate) text: String,
    /// Creation sequence, oldest first. Used for tie-breaks and cap eviction.
    pub(crate) order: u64,
}

impl Annotation {
    /// A compact one-line preview of the note for the list overlay and the inline
    /// marker hover label: the first line, clamped to `max` characters with an
    /// ellipsis when longer. Never panics on multi-byte text (clamps on a char
    /// boundary). Returns the whole first line when it already fits.
    pub(crate) fn preview(&self, max: usize) -> String {
        // Only the first physical line — a note may contain newlines from an
        // editor paste, but the marker/list show a single compact line.
        let first = self.text.lines().next().unwrap_or("");
        let count = first.chars().count();
        if count <= max {
            return first.to_string();
        }
        // Leave room for the one-char ellipsis.
        let keep = max.saturating_sub(1);
        let mut out: String = first.chars().take(keep).collect();
        out.push('\u{2026}');
        out
    }
}

/// Normalise a candidate note: trim surrounding whitespace and clamp to
/// [`ANNOTATION_TEXT_LIMIT`] characters on a char boundary. Returns `None` for a
/// blank note so the caller can treat "save an empty note" as a delete.
fn normalise_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let clamped: String = trimmed.chars().take(ANNOTATION_TEXT_LIMIT).collect();
    Some(clamped)
}

/// Pure annotation bookkeeping over stable entry ids.
///
/// The list is kept sorted by `(entry_id, order)` so iteration, `next`, and
/// `previous` all walk in transcript-reading order with a deterministic tie-break
/// when two annotations share an entry. The resting state is an empty `Vec`, so a
/// session that never annotates anything costs nothing.
#[derive(Debug, Clone, Default)]
pub(crate) struct AnnotationStore {
    annotations: Vec<Annotation>,
    next_id: u64,
    next_order: u64,
}

impl AnnotationStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Attach a note anchored at `entry_id`. Returns the new annotation's stable id
    /// on success, or `None` when the note is blank (nothing is added — an empty
    /// note is not an annotation). Keeps the list sorted by `(entry_id, order)`;
    /// exceeding [`ANNOTATION_CAP`] evicts the oldest-by-creation annotation.
    pub(crate) fn add(&mut self, entry_id: u64, text: &str) -> Option<u64> {
        let text = normalise_text(text)?;
        let id = self.next_id;
        self.next_id += 1;
        let order = self.next_order;
        self.next_order += 1;
        self.annotations.push(Annotation {
            id,
            entry_id,
            text,
            order,
        });
        self.sort();
        // Evict oldest-by-creation while over the cap.
        while self.annotations.len() > ANNOTATION_CAP {
            if let Some((idx, _)) = self
                .annotations
                .iter()
                .enumerate()
                .min_by_key(|(_, a)| a.order)
            {
                self.annotations.remove(idx);
            } else {
                break;
            }
        }
        Some(id)
    }

    /// Remove the annotation with the given stable id. Returns `true` when one was
    /// removed, `false` when no annotation carried that id.
    pub(crate) fn remove(&mut self, id: u64) -> bool {
        let before = self.annotations.len();
        self.annotations.retain(|a| a.id != id);
        self.annotations.len() != before
    }

    /// Replace the note text of the annotation with the given stable id. A blank
    /// new note deletes the annotation (returns `true` — it was found and removed).
    /// Returns `false` only when no annotation carried that id. Re-sorting is
    /// unnecessary — editing text never changes the `(entry_id, order)` sort key.
    pub(crate) fn set_text(&mut self, id: u64, text: &str) -> bool {
        match normalise_text(text) {
            Some(text) => {
                if let Some(annotation) = self.annotations.iter_mut().find(|a| a.id == id) {
                    annotation.text = text;
                    true
                } else {
                    false
                }
            }
            // A blank edit is a delete.
            None => self.remove(id),
        }
    }

    /// The annotations in transcript-reading order (by `(entry_id, order)`).
    pub(crate) fn list(&self) -> &[Annotation] {
        &self.annotations
    }

    /// Number of annotations currently stored.
    pub(crate) fn len(&self) -> usize {
        self.annotations.len()
    }

    /// Whether the store holds no annotations.
    pub(crate) fn is_empty(&self) -> bool {
        self.annotations.is_empty()
    }

    /// The annotation at list index `index` (in reading order), if any.
    pub(crate) fn get(&self, index: usize) -> Option<&Annotation> {
        self.annotations.get(index)
    }

    /// The list index of the annotation with the given stable id, if present.
    pub(crate) fn index_of(&self, id: u64) -> Option<usize> {
        self.annotations.iter().position(|a| a.id == id)
    }

    /// Whether `entry_id` carries at least one annotation — the badge-presence
    /// check the renderer calls per visible entry to decide whether to paint the
    /// inline marker. A linear scan over a small, capped list; the resting empty
    /// store returns `false` immediately, so an un-annotated session pays nothing.
    pub(crate) fn annotated(&self, entry_id: u64) -> bool {
        self.annotations.iter().any(|a| a.entry_id == entry_id)
    }

    /// How many annotations anchor `entry_id` (for a "+N" marker when an entry
    /// carries several notes). `0` when none.
    pub(crate) fn count_for_entry(&self, entry_id: u64) -> usize {
        self.annotations
            .iter()
            .filter(|a| a.entry_id == entry_id)
            .count()
    }

    /// The list index of the first annotation anchored at `entry_id`, if any — so
    /// a click on an entry's inline marker can open the list parked on that entry's
    /// note.
    pub(crate) fn first_index_for_entry(&self, entry_id: u64) -> Option<usize> {
        self.annotations.iter().position(|a| a.entry_id == entry_id)
    }

    /// The next annotation strictly *after* the given reading position, for
    /// "jump to next annotation". `from_entry_id` is the entry id currently at the
    /// reading position (`None` when nothing is anchored, e.g. an empty
    /// transcript). Returns the first annotation whose `entry_id` is greater than
    /// `from_entry_id`, wrapping to the first annotation when already at/after the
    /// last — so repeated presses cycle forward through every annotation. `None`
    /// only when the store is empty.
    pub(crate) fn next(&self, from_entry_id: Option<u64>) -> Option<&Annotation> {
        if self.annotations.is_empty() {
            return None;
        }
        match from_entry_id {
            Some(from) => self
                .annotations
                .iter()
                .find(|a| a.entry_id > from)
                .or_else(|| self.annotations.first()),
            None => self.annotations.first(),
        }
    }

    /// The previous annotation strictly *before* the given reading position, for
    /// "jump to previous annotation". Mirror of [`next`](Self::next): returns the
    /// last annotation whose `entry_id` is less than `from_entry_id`, wrapping to
    /// the last annotation when already at/before the first. `None` only when the
    /// store is empty.
    pub(crate) fn prev(&self, from_entry_id: Option<u64>) -> Option<&Annotation> {
        if self.annotations.is_empty() {
            return None;
        }
        match from_entry_id {
            Some(from) => self
                .annotations
                .iter()
                .rev()
                .find(|a| a.entry_id < from)
                .or_else(|| self.annotations.last()),
            None => self.annotations.last(),
        }
    }

    fn sort(&mut self) {
        self.annotations
            .sort_by(|a, b| a.entry_id.cmp(&b.entry_id).then(a.order.cmp(&b.order)));
    }
}

#[cfg(test)]
#[path = "annotations_tests.rs"]
mod tests;
