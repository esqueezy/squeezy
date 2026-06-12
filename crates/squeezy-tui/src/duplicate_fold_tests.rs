//! Unit tests for the Duplicate-Output Folding model (§12.5.4). Pure: no
//! terminal, no rendering — they exercise normalization, run detection, the
//! error-never-folds rule, expand toggling, navigation, the staleness fast path,
//! and the summary.

use super::*;

fn cand(id: u64, output: u64) -> FoldableOutput {
    // Existing tests use slice-contiguous ids 1,2,3,…, so deriving `seq` from
    // `id` makes them transcript-adjacent by construction. Tests that need a gap
    // (intervening conversation) build the candidate explicitly with `seq`.
    FoldableOutput {
        seq: id as usize,
        id,
        revision: 0,
        output,
        is_error: false,
    }
}

fn err(id: u64, output: u64) -> FoldableOutput {
    FoldableOutput {
        seq: id as usize,
        id,
        revision: 0,
        output,
        is_error: true,
    }
}

#[test]
fn normalize_collapses_progress_rewrites_and_ansi() {
    // A spinner that overwrote itself with `\r`, plus ANSI color, plus trailing
    // whitespace and a blank line, all normalize to the same canonical form.
    let a = "downloading...\rdownloading... 50%\rdownloading... 100%\n\n  done  \n";
    let b = "\x1b[32mdownloading...\x1b[0m\ndownloading... 50%\ndownloading... 100%\ndone";
    assert_eq!(normalize_output(a), normalize_output(b));
    assert_eq!(output_fingerprint(a), output_fingerprint(b));
}

#[test]
fn normalize_distinguishes_real_content() {
    assert_ne!(
        output_fingerprint("built 3 targets"),
        output_fingerprint("built 4 targets")
    );
}

#[test]
fn strip_ansi_removes_csi_and_bare_escapes() {
    assert_eq!(normalize_output("\x1b[1;31mERR\x1b[0m"), "ERR");
    assert_eq!(normalize_output("a\x1bMb"), "ab");
}

#[test]
fn consecutive_duplicates_fold_into_one_span() {
    let mut folds = DuplicateFolds::new();
    let cands = vec![cand(1, 100), cand(2, 100), cand(3, 100)];
    let fp = DuplicateFolds::fingerprint_of(cands.iter());
    assert!(folds.rebuild_if_stale(fp, &cands));

    assert_eq!(folds.span_count(), 1);
    let span = &folds.spans()[0];
    assert_eq!(span.lead_id, 1);
    assert_eq!(span.count(), 3);
    assert_eq!(span.member_ids, vec![1, 2, 3]);
    assert_eq!(span.folded_ids(), &[2, 3]);

    assert!(folds.is_lead(1));
    assert!(!folds.is_folded(1));
    assert!(folds.is_folded(2));
    assert!(folds.is_folded(3));
    assert_eq!(folds.hidden_count(), 2);
}

#[test]
fn single_output_does_not_fold() {
    let mut folds = DuplicateFolds::new();
    let cands = vec![cand(1, 100), cand(2, 200), cand(3, 300)];
    let fp = DuplicateFolds::fingerprint_of(cands.iter());
    folds.rebuild_if_stale(fp, &cands);
    assert_eq!(folds.span_count(), 0);
    assert_eq!(folds.hidden_count(), 0);
    assert!(!folds.is_folded(1));
}

#[test]
fn two_separate_runs_make_two_spans() {
    let mut folds = DuplicateFolds::new();
    // 100,100 | 200 (break) | 100,100 — the trailing pair is a *new* run even
    // though it shares the first run's fingerprint, because 200 broke the run.
    let cands = vec![
        cand(1, 100),
        cand(2, 100),
        cand(3, 200),
        cand(4, 100),
        cand(5, 100),
    ];
    let fp = DuplicateFolds::fingerprint_of(cands.iter());
    folds.rebuild_if_stale(fp, &cands);
    assert_eq!(folds.span_count(), 2);
    assert_eq!(folds.spans()[0].lead_id, 1);
    assert_eq!(folds.spans()[1].lead_id, 4);
    assert_eq!(folds.hidden_count(), 2);
}

#[test]
fn intervening_conversation_breaks_a_run() {
    // Two identical, non-error tool outputs that are *not* transcript-adjacent:
    // a conversation turn sits between them (seq 0 then seq 5), so the
    // tool-only candidate slice makes them slice-adjacent while the transcript
    // does not. They must NOT fold — a fold would hide the second output far
    // from its lead. A trailing distinct output rounds out the slice.
    let mut folds = DuplicateFolds::new();
    let cands = vec![
        FoldableOutput {
            seq: 0,
            id: 1,
            revision: 0,
            output: 100,
            is_error: false,
        },
        FoldableOutput {
            seq: 5,
            id: 2,
            revision: 0,
            output: 100,
            is_error: false,
        },
        FoldableOutput {
            seq: 6,
            id: 3,
            revision: 0,
            output: 200,
            is_error: false,
        },
    ];
    let fp = DuplicateFolds::fingerprint_of(cands.iter());
    folds.rebuild_if_stale(fp, &cands);
    // The seq gap (0 -> 5) breaks the run: no span, nothing hidden, neither
    // member projects as hidden.
    assert_eq!(folds.span_count(), 0);
    assert_eq!(folds.hidden_count(), 0);
    assert!(!folds.is_folded(1));
    assert!(!folds.is_folded(2));
    assert!(!folds.is_hidden_in_projection(1));
    assert!(!folds.is_hidden_in_projection(2));
}

#[test]
fn error_never_folds_and_breaks_a_run() {
    let mut folds = DuplicateFolds::new();
    // Two equal outputs, then a same-fingerprinted ERROR, then two more equal.
    let cands = vec![
        cand(1, 100),
        cand(2, 100),
        err(3, 100),
        cand(4, 100),
        cand(5, 100),
    ];
    let fp = DuplicateFolds::fingerprint_of(cands.iter());
    folds.rebuild_if_stale(fp, &cands);
    // The error is its own boundary: spans [1,2] and [4,5]; id 3 is never folded.
    assert_eq!(folds.span_count(), 2);
    assert!(!folds.is_folded(3));
    assert!(!folds.is_lead(3));
    assert!(folds.is_folded(2));
    assert!(folds.is_folded(5));
}

#[test]
fn lone_error_amid_duplicates_stays_visible() {
    let mut folds = DuplicateFolds::new();
    let cands = vec![cand(1, 100), err(2, 100), cand(3, 100)];
    let fp = DuplicateFolds::fingerprint_of(cands.iter());
    folds.rebuild_if_stale(fp, &cands);
    // No run of >= 2 non-error duplicates anywhere, so nothing folds.
    assert_eq!(folds.span_count(), 0);
    assert!(!folds.is_folded(2));
}

#[test]
fn expand_toggle_reveals_folded_members() {
    let mut folds = DuplicateFolds::new();
    let cands = vec![cand(1, 100), cand(2, 100), cand(3, 100)];
    let fp = DuplicateFolds::fingerprint_of(cands.iter());
    folds.rebuild_if_stale(fp, &cands);

    // Collapsed: folded members are hidden from projection.
    assert!(folds.is_hidden_in_projection(2));
    assert!(folds.is_hidden_in_projection(3));
    assert!(!folds.is_hidden_in_projection(1)); // lead always visible
    assert!(!folds.is_expanded(1));

    // Expand: raw retention — members project as visible again.
    assert_eq!(folds.toggle_expanded(1), Some(true));
    assert!(folds.is_expanded(1));
    assert!(!folds.is_hidden_in_projection(2));
    assert!(!folds.is_hidden_in_projection(3));
    // But they are still folded *members* (the model still owns them).
    assert!(folds.is_folded(2));

    // Collapse again.
    assert_eq!(folds.toggle_expanded(1), Some(false));
    assert!(folds.is_hidden_in_projection(2));

    // Toggling a non-lead is a no-op.
    assert_eq!(folds.toggle_expanded(2), None);
    assert_eq!(folds.toggle_expanded(999), None);
}

#[test]
fn navigation_walks_and_wraps() {
    let mut folds = DuplicateFolds::new();
    let cands = vec![
        cand(1, 100),
        cand(2, 100),
        cand(3, 200),
        cand(4, 300),
        cand(5, 300),
    ];
    let fp = DuplicateFolds::fingerprint_of(cands.iter());
    folds.rebuild_if_stale(fp, &cands);
    // Spans led by 1 and 4.
    assert_eq!(folds.next_lead(None), Some(1));
    assert_eq!(folds.next_lead(Some(1)), Some(4));
    assert_eq!(folds.next_lead(Some(4)), Some(1)); // wrap
    assert_eq!(folds.next_lead(Some(999)), Some(1)); // unknown -> first

    assert_eq!(folds.prev_lead(None), Some(4));
    assert_eq!(folds.prev_lead(Some(4)), Some(1));
    assert_eq!(folds.prev_lead(Some(1)), Some(4)); // wrap
}

#[test]
fn navigation_empty_is_none() {
    let folds = DuplicateFolds::new();
    assert_eq!(folds.next_lead(None), None);
    assert_eq!(folds.prev_lead(None), None);
}

#[test]
fn rebuild_is_skipped_when_fingerprint_unchanged() {
    let mut folds = DuplicateFolds::new();
    let cands = vec![cand(1, 100), cand(2, 100)];
    let fp = DuplicateFolds::fingerprint_of(cands.iter());
    assert!(folds.rebuild_if_stale(fp, &cands)); // first build runs
    assert!(!folds.rebuild_if_stale(fp, &cands)); // unchanged -> fast path
    assert_eq!(folds.fingerprint(), fp);

    // A revision bump moves the fingerprint and forces a rebuild.
    let bumped: Vec<FoldableOutput> = cands
        .iter()
        .cloned()
        .map(|mut c| {
            c.revision += 1;
            c
        })
        .collect();
    let fp2 = DuplicateFolds::fingerprint_of(bumped.iter());
    assert_ne!(fp, fp2);
    assert!(folds.rebuild_if_stale(fp2, &bumped));
}

#[test]
fn expand_state_drops_when_lead_stops_folding() {
    let mut folds = DuplicateFolds::new();
    let cands = vec![cand(1, 100), cand(2, 100)];
    let fp = DuplicateFolds::fingerprint_of(cands.iter());
    folds.rebuild_if_stale(fp, &cands);
    folds.toggle_expanded(1);
    assert!(folds.is_expanded(1));

    // The run breaks up: id 1 no longer leads a fold.
    let cands2 = vec![cand(1, 100), cand(2, 200)];
    let fp2 = DuplicateFolds::fingerprint_of(cands2.iter());
    folds.rebuild_if_stale(fp2, &cands2);
    assert_eq!(folds.span_count(), 0);
    assert!(!folds.is_expanded(1));
}

#[test]
fn empty_transcript_indexes_without_repeated_rebuild() {
    let mut folds = DuplicateFolds::new();
    let cands: Vec<FoldableOutput> = vec![];
    let fp = DuplicateFolds::fingerprint_of(cands.iter());
    assert!(folds.rebuild_if_stale(fp, &cands)); // first build
    assert!(!folds.rebuild_if_stale(fp, &cands)); // empty stays cached
    assert_eq!(folds.span_count(), 0);
    assert_eq!(folds.summary(), "");
}

#[test]
fn summary_reads_naturally() {
    let mut folds = DuplicateFolds::new();
    let cands = vec![
        cand(1, 100),
        cand(2, 100),
        cand(3, 100),
        cand(4, 200),
        cand(5, 200),
    ];
    let fp = DuplicateFolds::fingerprint_of(cands.iter());
    folds.rebuild_if_stale(fp, &cands);
    // 2 folds; hidden = (3-1) + (2-1) = 3.
    assert_eq!(folds.hidden_count(), 3);
    assert_eq!(folds.summary(), "2 folds \u{00b7} 3 outputs hidden");
}

#[test]
fn summary_singular_forms() {
    let mut folds = DuplicateFolds::new();
    let cands = vec![cand(1, 100), cand(2, 100)];
    let fp = DuplicateFolds::fingerprint_of(cands.iter());
    folds.rebuild_if_stale(fp, &cands);
    assert_eq!(folds.summary(), "1 fold \u{00b7} 1 output hidden");
}
