//! Inline Rename Labels (§12.1.7 / backlog 12.1.7): let the user pin a short,
//! human-meaningful *label* onto an important transcript entry (a turn, a pinned
//! output, an error) or a queued prompt — "the auth refactor", "broken test" —
//! and have that label paint as a small badge so the session is navigable by
//! meaning rather than by raw position.
//!
//! A label is pure UI metadata. It lives only in this store (session UI metadata),
//! never in the model transcript, so — like an annotation — it can never leak into
//! model-provider context unless the user explicitly quotes/copies it. That is the
//! spec's core invariant: labels "never alter transcript/model-provider history".
//!
//! Each label is keyed by a stable [`LabelTargetId`], not a screen coordinate or a
//! row offset — the one identity that survives every reflow, resize, fold, and
//! filter. A transcript entry is addressed by its stable `TranscriptEntry::id`; a
//! queued prompt by its position in the live queue. The spec frames the key as a
//! small, extensible `LabelTargetId` enum (turn, entry, pinned output, queue item,
//! bookmark, jump mark); this module models that enum and implements the two
//! surfaced targets — the transcript entry and the queue item — end to end, with
//! room for the rest to slot in without a schema change.
//!
//! This module is intentionally pure: it owns the label map and the normalise /
//! create / edit / clear / lookup math and nothing about geometry, rendering, or
//! input. `lib.rs` captures the focused (or top-visible) entry id, feeds it in,
//! paints a small label badge on the row, and reuses the composer-edit primitive
//! (an in-place [`InlineEditState`] buffer, exactly like the annotation editor) to
//! create/edit the text. That keeps the CRUD testable without a terminal.

/// Largest number of labels retained. Labels are a lightweight navigation aid, not
/// a database; a small, predictable cap keeps the map bounded and every per-frame
/// lookup cheap. Adding past the cap drops the oldest-by-creation label so the
/// newest intent always lands.
pub(crate) const LABEL_CAP: usize = 256;

/// Longest label retained, in characters. A label is a short name, not a note;
/// clamping keeps the inline editor line and the badge bounded. Text past the
/// limit is truncated on a char boundary (never mid-grapheme-byte) at write time.
pub(crate) const LABEL_TEXT_LIMIT: usize = 48;

/// The stable identity a label is pinned to. Never a screen coordinate — always a
/// logical handle that survives reflow/resize/fold/filter.
///
/// The spec lists turn, transcript entry, pinned output, queue item, bookmark, and
/// jump mark. The two variants below are the ones surfaced end to end; the enum is
/// the extension point for the rest (each is just another stable key into the same
/// map), so adding one is additive and needs no schema migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum LabelTargetId {
    /// A transcript entry, addressed by its stable `TranscriptEntry::id`.
    Entry(u64),
    /// A queued prompt, addressed by its 0-based position in the live queue. A
    /// queue position is the queue's only stable handle (items are stored by text,
    /// not by id); reorder/delete remaps positions, which the queue store owns.
    ///
    /// This is the spec's extension target for queued prompts. The store handles it
    /// uniformly (it is just another stable key into the same map) and the unit
    /// tests exercise it, but the queue-overlay *surface* is not yet wired — the
    /// queue's position-based handle remaps on reorder/delete, so surfacing it
    /// safely needs the queue's own id remap, kept out of this additive change. The
    /// `allow` keeps the extension point from tripping the dead-code gate until then.
    #[allow(dead_code)]
    QueueItem(usize),
}

/// In-place text-edit buffer for the inline rename editor (§12.1.7). Mirrors the
/// annotation composer-edit primitive: a small owned buffer plus the target being
/// renamed and whether this edit is creating a brand-new label (so a blank commit
/// can cancel a creation but delete an existing label). Resting state is `None` on
/// `app` — no editor, no cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InlineEditState {
    /// The target whose label is being edited.
    pub(crate) target: LabelTargetId,
    /// The current editor text (the composer buffer).
    pub(crate) buffer: String,
    /// `true` when the target had no label before this edit began. A blank commit
    /// then cancels (adds nothing) rather than deleting; a blank commit while
    /// editing an existing label deletes it.
    pub(crate) is_new: bool,
}

impl InlineEditState {
    /// Begin editing `target`, seeding the buffer with `seed` (the existing label,
    /// or empty for a brand-new one). `is_new` records whether a label already
    /// existed so the commit can tell "cancel a creation" from "delete an edit".
    pub(crate) fn new(target: LabelTargetId, seed: String, is_new: bool) -> Self {
        Self {
            target,
            buffer: seed,
            is_new,
        }
    }

    /// Append a typed character (clamped to the text limit so the editor line stays
    /// bounded). Returns `true` when the buffer changed (so the caller can schedule
    /// a redraw only when something moved).
    pub(crate) fn push(&mut self, ch: char) -> bool {
        if self.buffer.chars().count() >= LABEL_TEXT_LIMIT {
            return false;
        }
        self.buffer.push(ch);
        true
    }

    /// Delete the last character. Returns `true` when the buffer changed.
    pub(crate) fn backspace(&mut self) -> bool {
        self.buffer.pop().is_some()
    }
}

/// Normalise a candidate label: trim surrounding whitespace, collapse interior
/// runs of whitespace to single spaces (a label is a single compact line), and
/// clamp to [`LABEL_TEXT_LIMIT`] characters on a char boundary. Returns `None` for
/// a blank label so the caller can treat "save an empty label" as a clear.
pub(crate) fn normalise_label(text: &str) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    let clamped: String = collapsed.chars().take(LABEL_TEXT_LIMIT).collect();
    Some(clamped)
}

/// A single stored label: its target, its text, and the creation order (so the cap
/// can evict the oldest deterministically).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Label {
    text: String,
    order: u64,
}

/// Pure label bookkeeping keyed by stable [`LabelTargetId`]. The resting state is
/// an empty map, so a session that never labels anything costs nothing: every
/// per-frame `label_for` lookup short-circuits on the empty map.
#[derive(Debug, Clone, Default)]
pub(crate) struct LabelStore {
    labels: std::collections::BTreeMap<LabelTargetId, Label>,
    next_order: u64,
}

impl LabelStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Set (create or replace) the label for `target`. A blank label clears the
    /// target instead (returns `false` — nothing is stored). Returns `true` when a
    /// label is now stored for the target. Replacing an existing label keeps its
    /// creation order (so editing text never reshuffles cap eviction); a brand-new
    /// label takes the next order and, if that pushes the map over [`LABEL_CAP`],
    /// evicts the oldest-by-creation label.
    pub(crate) fn set(&mut self, target: LabelTargetId, text: &str) -> bool {
        let Some(text) = normalise_label(text) else {
            self.clear(target);
            return false;
        };
        if let Some(existing) = self.labels.get_mut(&target) {
            existing.text = text;
            return true;
        }
        let order = self.next_order;
        self.next_order += 1;
        self.labels.insert(target, Label { text, order });
        while self.labels.len() > LABEL_CAP {
            if let Some(oldest) = self
                .labels
                .iter()
                .min_by_key(|(_, l)| l.order)
                .map(|(k, _)| *k)
            {
                self.labels.remove(&oldest);
            } else {
                break;
            }
        }
        true
    }

    /// Remove the label for `target`, if any. Returns `true` when one was removed.
    pub(crate) fn clear(&mut self, target: LabelTargetId) -> bool {
        self.labels.remove(&target).is_some()
    }

    /// The label text for `target`, if one is stored.
    pub(crate) fn label_for(&self, target: LabelTargetId) -> Option<&str> {
        self.labels.get(&target).map(|l| l.text.as_str())
    }

    /// Whether `target` carries a label — the badge-presence check the renderer
    /// calls per visible row. The empty store returns `false` immediately.
    pub(crate) fn has_label(&self, target: LabelTargetId) -> bool {
        self.labels.contains_key(&target)
    }

    /// Number of labels stored.
    pub(crate) fn len(&self) -> usize {
        self.labels.len()
    }

    /// Whether the store holds no labels (the resting state).
    pub(crate) fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }
}

/// Compact a label for the inline badge: clamp to `max` characters with a trailing
/// ellipsis when longer, so a long label never blows out a narrow row. Never
/// panics on multi-byte text (clamps on a char boundary). Returns the whole label
/// when it already fits.
pub(crate) fn badge_preview(label: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let count = label.chars().count();
    if count <= max {
        return label.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = label.chars().take(keep).collect();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
#[path = "rename_labels_tests.rs"]
mod tests;
