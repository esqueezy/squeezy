//! Duplicate-Output Folding (§12.5.4): detect runs of repeated / near-duplicate
//! tool outputs in the transcript and collapse each run into a single
//! [`FoldSpan`] carrying a count, while **retaining every member's raw content**
//! for expand, search, copy, export, and diagnostics. Errors are never folded —
//! a failure always stays visible on its own.
//!
//! **What folds.** A *fold candidate* is a tool-result entry reduced to a
//! normalized output fingerprint (see [`output_fingerprint`]): the raw text with
//! `\r\n`/`\r` progress rewrites flattened, ANSI escapes stripped, per-line
//! whitespace collapsed, and blank lines dropped. Two consecutive candidates with
//! the *same* fingerprint are duplicates; a maximal run of `>= 2` duplicates
//! becomes one [`FoldSpan`]. The run's first member is the **lead** (it stays
//! visible, annotated with the fold count); the rest are **folded** (hidden until
//! expanded). A candidate flagged `is_error` is never a member — it breaks the
//! run and stays visible on its own line, so a unique failure is never swallowed
//! by a fold above or below it.
//!
//! **Stable ids, never row offsets.** Like the transcript index (§12.5.1) and the
//! jump-mark stack, every key here is a stable `TranscriptEntry::id`, never a
//! width-/fold-dependent row coordinate. An id survives reflow (resize,
//! streaming, collapse, coalescing), so a span built before a reflow still
//! resolves to the right entries afterwards.
//!
//! **Zero idle cost, incremental rebuild.** The model carries a `fingerprint`
//! folded over every candidate `(id, revision, output-fingerprint, is_error)`.
//! The caller feeds the same fingerprint each refresh via
//! [`DuplicateFolds::rebuild_if_stale`]; when it matches the stored one the call
//! returns immediately and touches nothing. Folds are only recomputed when the
//! transcript actually changed — exactly the events that move the fingerprint. An
//! idle session pays one cheap `u64` comparison per refresh.
//!
//! This module is deliberately pure: it owns the fingerprinting and run-detection
//! bookkeeping and nothing about geometry, rendering, or input. `lib.rs`
//! classifies each tool-result entry into a [`FoldableOutput`] (reusing the same
//! `entry_is_error` predicate the renderer and jump-nav use) and feeds the slice
//! in; this module turns that into spans and answers fold/expand/navigation
//! queries. That keeps the folding math testable without a terminal.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

/// Normalize one tool-output text for duplicate detection, then fold it to a
/// `u64` fingerprint. Two outputs that differ only in progress rewrites
/// (`\r`-overwritten lines), ANSI color, trailing whitespace, or blank-line
/// padding share a fingerprint and therefore fold together; any real content
/// difference moves the value so they stay distinct. Pure and standalone so the
/// caller can compute it cheaply per entry.
pub(crate) fn output_fingerprint(text: &str) -> u64 {
    let normalized = normalize_output(text);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    normalized.hash(&mut hasher);
    hasher.finish()
}

/// Reduce a raw tool output to its canonical comparison form: flatten `\r\n` and
/// bare `\r` progress rewrites to newlines (so a spinner/percentage that
/// overwrote itself collapses), strip ANSI escape sequences, collapse each line's
/// internal whitespace to single spaces and trim its ends, drop blank lines, and
/// trim the whole. The result is what two outputs are compared on. Kept separate
/// from [`output_fingerprint`] so the normalization is unit-testable directly.
pub(crate) fn normalize_output(text: &str) -> String {
    let flattened = text.replace("\r\n", "\n").replace('\r', "\n");
    let stripped = strip_ansi(&flattened);
    stripped
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Remove ANSI/VT escape sequences (CSI `\x1b[...m` and friends, plus a bare
/// `\x1b` followed by a single byte) so color/cursor control does not defeat
/// duplicate detection. Self-contained so the pure module has no dependency on
/// the renderer's stripper.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    // Consume parameter/intermediate bytes up to and including
                    // the final byte in the 0x40..=0x7e range.
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                Some(_) => {
                    // Bare two-byte escape (e.g. `\x1bM`): drop the next byte.
                    chars.next();
                }
                None => {}
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// One fold-candidate transcript entry, as the caller feeds it in. `seq` is the
/// entry's **position in the full transcript** (not in the candidate slice), so
/// run detection can tell genuinely consecutive tool outputs apart from ones
/// separated by intervening conversation; `id` is the stable
/// `TranscriptEntry::id`; `revision` is its content revision (folded into the
/// staleness fingerprint so a mutation recomputes); `output` is the normalized
/// output fingerprint from [`output_fingerprint`]; `is_error` flags a failure
/// that must never be folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoldableOutput {
    pub(crate) seq: usize,
    pub(crate) id: u64,
    pub(crate) revision: u64,
    pub(crate) output: u64,
    pub(crate) is_error: bool,
}

/// A collapsed run of consecutive duplicate outputs (§12.5.4). `lead_id` is the
/// first (visible) member; `member_ids` lists every member in transcript order,
/// `lead_id` included, so `member_ids.len() == count`. `count >= 2` always (a
/// single output is not a fold). `output` is the shared output fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoldSpan {
    pub(crate) lead_id: u64,
    pub(crate) member_ids: Vec<u64>,
    pub(crate) output: u64,
}

impl FoldSpan {
    /// Number of outputs collapsed into this span (`>= 2`).
    pub(crate) fn count(&self) -> usize {
        self.member_ids.len()
    }

    /// The folded (hidden-until-expanded) members: every member after the lead.
    /// The row-projection consumer that hides these behind the lead lands in a
    /// follow-up, so this is exercised by the unit suite today.
    #[cfg(test)]
    pub(crate) fn folded_ids(&self) -> &[u64] {
        &self.member_ids[1..]
    }
}

/// The computed duplicate-fold model over the transcript's tool outputs.
///
/// `spans` is the ordered list of detected folds (transcript order by lead).
/// `lead_to_span` maps a lead id to its span index for O(1) lookup, and
/// `folded` is the set of hidden member ids (every member except its lead).
/// `expanded` tracks which spans the user has manually expanded (by lead id), so
/// an expanded span's members project as visible. `fingerprint` is the staleness
/// tag described in the module docs.
#[derive(Debug, Clone, Default)]
pub(crate) struct DuplicateFolds {
    spans: Vec<FoldSpan>,
    lead_to_span: HashMap<u64, usize>,
    folded: HashSet<u64>,
    expanded: HashSet<u64>,
    fingerprint: u64,
    /// Total number of outputs hidden by folding (sum over spans of count-1).
    hidden: usize,
    /// Whether a rebuild has ever run, so a genuinely empty transcript is not
    /// re-walked every refresh.
    built: bool,
}

impl DuplicateFolds {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Fold a staleness fingerprint over a sequence of candidates. Order- and
    /// content-sensitive: transcript position, id, revision, output fingerprint,
    /// and the error flag all participate, so an append, a revision bump, a
    /// reorder, an output change, a drop, or intervening conversation that shifts
    /// a tool output's transcript position (and can break a run) all move the
    /// value. Pure and standalone so the caller can compute it cheaply each
    /// refresh and compare before deciding to recompute.
    pub(crate) fn fingerprint_of<'a>(
        candidates: impl IntoIterator<Item = &'a FoldableOutput>,
    ) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for c in candidates {
            c.seq.hash(&mut hasher);
            c.id.hash(&mut hasher);
            c.revision.hash(&mut hasher);
            c.output.hash(&mut hasher);
            c.is_error.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Recompute the fold model from `candidates` **only if** `fingerprint`
    /// differs from the one captured at the last rebuild (or this is the first
    /// build). Returns `true` when a recompute actually ran, `false` when the
    /// cached model was already current (the zero-idle-cost fast path).
    ///
    /// Manual expand state is preserved across a rebuild *only* for leads that
    /// still head a span — an expanded lead that no longer folds (its run broke
    /// up) drops out, so stale expand state can never strand a hidden id.
    pub(crate) fn rebuild_if_stale(
        &mut self,
        fingerprint: u64,
        candidates: &[FoldableOutput],
    ) -> bool {
        if self.built && fingerprint == self.fingerprint {
            return false;
        }
        self.spans.clear();
        self.lead_to_span.clear();
        self.folded.clear();
        self.hidden = 0;

        let mut i = 0;
        while i < candidates.len() {
            let head = &candidates[i];
            // An error is never a fold member: it stands alone and breaks any
            // run, so a unique failure is always visible.
            if head.is_error {
                i += 1;
                continue;
            }
            // Extend a run of consecutive non-error candidates sharing this
            // output fingerprint. Contiguity is *transcript-relative*: each step
            // must sit immediately after the previous one in the full transcript
            // (`seq` is +1), so any intervening user/assistant/log entry breaks
            // the run even though such entries are absent from this tool-only
            // candidate slice. Without this, two identical outputs separated by a
            // conversation turn would fold as if adjacent.
            let mut j = i + 1;
            while j < candidates.len()
                && !candidates[j].is_error
                && candidates[j].output == head.output
                && candidates[j].seq == candidates[j - 1].seq + 1
            {
                j += 1;
            }
            let run = &candidates[i..j];
            if run.len() >= 2 {
                let member_ids: Vec<u64> = run.iter().map(|c| c.id).collect();
                for &id in &member_ids[1..] {
                    self.folded.insert(id);
                }
                self.hidden += member_ids.len() - 1;
                self.lead_to_span.insert(head.id, self.spans.len());
                self.spans.push(FoldSpan {
                    lead_id: head.id,
                    member_ids,
                    output: head.output,
                });
            }
            i = j;
        }

        // Drop expand state for leads that no longer fold.
        self.expanded
            .retain(|id| self.lead_to_span.contains_key(id));

        self.fingerprint = fingerprint;
        self.built = true;
        true
    }

    /// The stored staleness fingerprint from the last rebuild. Test/diagnostic
    /// accessor; production compares inside `rebuild_if_stale`.
    #[cfg(test)]
    pub(crate) fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// The detected fold spans in transcript order (by lead).
    pub(crate) fn spans(&self) -> &[FoldSpan] {
        &self.spans
    }

    /// Number of fold spans.
    pub(crate) fn span_count(&self) -> usize {
        self.spans.len()
    }

    /// Total number of outputs hidden by folding (sum over spans of count-1).
    /// This is the count the user "saves" by folding; zero when nothing folds.
    /// The summary line reads `hidden` directly; the standalone accessor is for
    /// diagnostics / the unit suite.
    #[cfg(test)]
    pub(crate) fn hidden_count(&self) -> usize {
        self.hidden
    }

    /// Whether entry `id` is a folded (hidden-until-expanded) member of some
    /// span. A lead is never folded; only the trailing duplicates are. The
    /// row-projection consumer lands in a follow-up, so this is exercised by the
    /// unit suite today.
    #[cfg(test)]
    pub(crate) fn is_folded(&self, id: u64) -> bool {
        self.folded.contains(&id)
    }

    /// Whether entry `id` is the visible lead of a fold span. The row-projection
    /// consumer (which annotates the lead with its fold count) lands in a
    /// follow-up, so this is exercised by the unit suite today.
    #[cfg(test)]
    pub(crate) fn is_lead(&self, id: u64) -> bool {
        self.lead_to_span.contains_key(&id)
    }

    /// The fold span led by `id`, or `None` when `id` is not a lead.
    pub(crate) fn span_for_lead(&self, id: u64) -> Option<&FoldSpan> {
        self.lead_to_span.get(&id).and_then(|&i| self.spans.get(i))
    }

    /// Whether the span led by `id` is currently expanded (its folded members
    /// project as visible). Always `false` for a non-lead id.
    pub(crate) fn is_expanded(&self, id: u64) -> bool {
        self.expanded.contains(&id)
    }

    /// Toggle the expanded state of the span led by `id`. Returns the new state
    /// (`true` = now expanded), or `None` when `id` is not a fold lead.
    pub(crate) fn toggle_expanded(&mut self, id: u64) -> Option<bool> {
        if !self.lead_to_span.contains_key(&id) {
            return None;
        }
        let now = if self.expanded.remove(&id) {
            false
        } else {
            self.expanded.insert(id);
            true
        };
        Some(now)
    }

    /// Whether entry `id` should be *hidden* from the visible row projection:
    /// it is a folded member **and** its span is not currently expanded. The
    /// single predicate the renderer consults — a folded member of an expanded
    /// span projects as visible (raw retention), a folded member of a collapsed
    /// span is hidden behind its lead's count. The row-projection consumer lands
    /// in a follow-up, so this is exercised by the unit suite today (it is the
    /// single predicate that consumer will call).
    #[cfg(test)]
    pub(crate) fn is_hidden_in_projection(&self, id: u64) -> bool {
        if !self.folded.contains(&id) {
            return false;
        }
        // Find the lead of the span this id belongs to; visible iff expanded.
        for span in &self.spans {
            if span.member_ids.contains(&id) {
                return !self.expanded.contains(&span.lead_id);
            }
        }
        false
    }

    /// The lead id of the next fold span strictly after `after` (transcript
    /// order), wrapping to the first when `after` is the last lead or is not a
    /// lead. `None` only when there are no spans. Drives forward fold navigation;
    /// the overlay walks by cursor index today, so this id-anchored step is
    /// exercised by the unit suite until an id-anchored "next fold" verb lands.
    #[cfg(test)]
    pub(crate) fn next_lead(&self, after: Option<u64>) -> Option<u64> {
        if self.spans.is_empty() {
            return None;
        }
        let pos = after.and_then(|a| self.spans.iter().position(|s| s.lead_id == a));
        match pos {
            Some(p) => Some(self.spans[(p + 1) % self.spans.len()].lead_id),
            None => Some(self.spans[0].lead_id),
        }
    }

    /// The lead id of the previous fold span strictly before `before`
    /// (transcript order), wrapping to the last. `None` only when there are no
    /// spans. Drives backward fold navigation; the overlay walks forward today,
    /// so this is exercised by the unit suite until a "previous fold" verb lands.
    #[cfg(test)]
    pub(crate) fn prev_lead(&self, before: Option<u64>) -> Option<u64> {
        if self.spans.is_empty() {
            return None;
        }
        let pos = before.and_then(|b| self.spans.iter().position(|s| s.lead_id == b));
        match pos {
            Some(0) | None => self.spans.last().map(|s| s.lead_id),
            Some(p) => Some(self.spans[p - 1].lead_id),
        }
    }

    /// A compact one-line summary for the status line / overlay header, e.g.
    /// `"3 folds \u{00b7} 11 outputs hidden"`. Empty string when nothing folds.
    pub(crate) fn summary(&self) -> String {
        if self.spans.is_empty() {
            return String::new();
        }
        let folds = self.spans.len();
        let fold_word = if folds == 1 { "fold" } else { "folds" };
        let output_word = if self.hidden == 1 {
            "output"
        } else {
            "outputs"
        };
        format!(
            "{folds} {fold_word} \u{00b7} {} {output_word} hidden",
            self.hidden
        )
    }
}

#[cfg(test)]
#[path = "duplicate_fold_tests.rs"]
mod tests;
