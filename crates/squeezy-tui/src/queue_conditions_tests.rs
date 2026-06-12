//! Unit tests for the pure Conditional Queue Items (§12.3.5) state surface.
//!
//! These pin the condition model, the cycle order, every evaluation against a
//! synthetic [`TurnOutcome`], and the [`plan_drain`] policy — all without a live
//! queue or a running terminal. The lib-level integration (keyboard `v`, the
//! mouse twin, the real `render()`, and the drain pump) is covered in
//! `lib_tests.rs`.

use super::*;

fn outcome(succeeded: bool, had_edits: bool) -> TurnOutcome {
    TurnOutcome {
        succeeded,
        had_edits,
    }
}

#[test]
fn default_condition_is_always_and_is_recognised() {
    assert_eq!(QueueCondition::default(), QueueCondition::Always);
    assert!(QueueCondition::Always.is_always());
    assert!(!QueueCondition::Manual.is_always());
    assert!(!QueueCondition::IfPrevSucceeded.is_always());
}

#[test]
fn cycle_visits_every_state_in_order_and_wraps() {
    // Stepping `next` six times from Always returns to Always, visiting each
    // condition exactly once in the documented order.
    let mut seen = Vec::new();
    let mut cond = QueueCondition::Always;
    for _ in 0..6 {
        seen.push(cond);
        cond = cond.next();
    }
    assert_eq!(
        seen,
        vec![
            QueueCondition::Always,
            QueueCondition::IfPrevSucceeded,
            QueueCondition::IfPrevFailed,
            QueueCondition::IfPrevEdited,
            QueueCondition::IfPrevNoEdits,
            QueueCondition::Manual,
        ]
    );
    // One more step wraps back to the start.
    assert_eq!(cond, QueueCondition::Always);
}

#[test]
fn always_is_runnable_regardless_of_outcome_or_absence() {
    assert_eq!(
        evaluate(QueueCondition::Always, None),
        ConditionEval::Runnable
    );
    assert_eq!(
        evaluate(QueueCondition::Always, Some(outcome(false, false))),
        ConditionEval::Runnable
    );
}

#[test]
fn manual_is_always_blocked_never_auto_drains() {
    // Manual never runs from the pump, whatever the outcome — it waits for the
    // user to run it by hand.
    assert_eq!(
        evaluate(QueueCondition::Manual, None),
        ConditionEval::Blocked
    );
    assert_eq!(
        evaluate(QueueCondition::Manual, Some(outcome(true, true))),
        ConditionEval::Blocked
    );
}

#[test]
fn previous_turn_conditions_block_when_no_outcome_yet() {
    // A fresh session (no finished turn) holds every "if previous X" prompt rather
    // than skipping it or running it blind.
    for cond in [
        QueueCondition::IfPrevSucceeded,
        QueueCondition::IfPrevFailed,
        QueueCondition::IfPrevEdited,
        QueueCondition::IfPrevNoEdits,
    ] {
        assert_eq!(evaluate(cond, None), ConditionEval::Blocked, "{cond:?}");
    }
}

#[test]
fn if_prev_succeeded_runs_on_success_skips_on_failure() {
    assert_eq!(
        evaluate(QueueCondition::IfPrevSucceeded, Some(outcome(true, false))),
        ConditionEval::Runnable
    );
    assert_eq!(
        evaluate(QueueCondition::IfPrevSucceeded, Some(outcome(false, false))),
        ConditionEval::Skipped
    );
}

#[test]
fn if_prev_failed_runs_on_failure_skips_on_success() {
    assert_eq!(
        evaluate(QueueCondition::IfPrevFailed, Some(outcome(false, false))),
        ConditionEval::Runnable
    );
    assert_eq!(
        evaluate(QueueCondition::IfPrevFailed, Some(outcome(true, false))),
        ConditionEval::Skipped
    );
}

#[test]
fn edit_conditions_key_off_had_edits() {
    assert_eq!(
        evaluate(QueueCondition::IfPrevEdited, Some(outcome(true, true))),
        ConditionEval::Runnable
    );
    assert_eq!(
        evaluate(QueueCondition::IfPrevEdited, Some(outcome(true, false))),
        ConditionEval::Skipped
    );
    assert_eq!(
        evaluate(QueueCondition::IfPrevNoEdits, Some(outcome(true, false))),
        ConditionEval::Runnable
    );
    assert_eq!(
        evaluate(QueueCondition::IfPrevNoEdits, Some(outcome(true, true))),
        ConditionEval::Skipped
    );
    // Edit conditions ignore the success flag — a failed-but-edited turn still
    // satisfies "if previous edited".
    assert_eq!(
        evaluate(QueueCondition::IfPrevEdited, Some(outcome(false, true))),
        ConditionEval::Runnable
    );
}

#[test]
fn map_stores_only_non_always_entries() {
    let mut conds = QueueConditions::new();
    assert!(conds.is_empty());
    assert_eq!(conds.get(7), QueueCondition::Always);

    conds.set(7, QueueCondition::Manual);
    assert!(!conds.is_empty());
    assert_eq!(conds.get(7), QueueCondition::Manual);

    // Setting back to Always clears the entry, so the map only ever holds
    // genuinely-conditional rows.
    conds.set(7, QueueCondition::Always);
    assert!(conds.is_empty());
    assert_eq!(conds.get(7), QueueCondition::Always);
}

#[test]
fn cycle_advances_a_single_item_and_returns_the_new_value() {
    let mut conds = QueueConditions::new();
    assert_eq!(conds.cycle(3), QueueCondition::IfPrevSucceeded);
    assert_eq!(conds.get(3), QueueCondition::IfPrevSucceeded);
    // Other ids are untouched.
    assert_eq!(conds.get(4), QueueCondition::Always);
    // Cycling through the whole ring returns to Always and clears the entry.
    for _ in 0..5 {
        conds.cycle(3);
    }
    assert_eq!(conds.get(3), QueueCondition::Always);
    assert!(conds.is_empty());
}

#[test]
fn retain_live_drops_conditions_for_drained_ids() {
    let mut conds = QueueConditions::new();
    conds.set(1, QueueCondition::Manual);
    conds.set(2, QueueCondition::IfPrevSucceeded);
    conds.set(3, QueueCondition::IfPrevFailed);
    // Id 2 drained out of the live queue.
    conds.retain_live(&[1, 3]);
    assert_eq!(conds.get(1), QueueCondition::Manual);
    assert_eq!(conds.get(2), QueueCondition::Always); // pruned
    assert_eq!(conds.get(3), QueueCondition::IfPrevFailed);
    // Retain over an empty live set clears everything.
    conds.retain_live(&[]);
    assert!(conds.is_empty());
}

fn item(paused: bool, condition: QueueCondition) -> DrainItem {
    DrainItem { paused, condition }
}

#[test]
fn plan_drain_empty_queue_stops() {
    assert_eq!(
        plan_drain(&[], Some(outcome(true, false))),
        DrainAction::Stop
    );
}

#[test]
fn plan_drain_all_unconditional_runs_front() {
    let items = [
        item(false, QueueCondition::Always),
        item(false, QueueCondition::Always),
    ];
    assert_eq!(plan_drain(&items, None), DrainAction::Run(0));
}

#[test]
fn plan_drain_parks_paused_and_runs_behind() {
    // A paused front item is stepped over; the loose item behind it runs.
    let items = [
        item(true, QueueCondition::Always),
        item(false, QueueCondition::Always),
    ];
    assert_eq!(
        plan_drain(&items, Some(outcome(true, false))),
        DrainAction::Run(1)
    );
}

#[test]
fn plan_drain_drops_a_skip_bound_front_item() {
    // "if previous succeeded" after a failed turn: the front is skip-bound, so the
    // pump drops it (one Drop per call) before reaching the runnable one behind it.
    let items = [
        item(false, QueueCondition::IfPrevSucceeded),
        item(false, QueueCondition::Always),
    ];
    assert_eq!(
        plan_drain(&items, Some(outcome(false, false))),
        DrainAction::Drop(0)
    );
}

#[test]
fn plan_drain_runs_a_satisfied_conditional_front() {
    let items = [item(false, QueueCondition::IfPrevSucceeded)];
    assert_eq!(
        plan_drain(&items, Some(outcome(true, false))),
        DrainAction::Run(0)
    );
}

#[test]
fn plan_drain_parks_manual_and_blocked_then_stops() {
    // A manual item and a not-yet-evaluable conditional both park; with nothing
    // runnable behind them the pump stops (it does not drop or run anything).
    let items = [
        item(false, QueueCondition::Manual),
        item(false, QueueCondition::IfPrevSucceeded),
    ];
    assert_eq!(plan_drain(&items, None), DrainAction::Stop);
}

#[test]
fn plan_drain_skip_bound_ahead_of_runnable_wins_first() {
    // A skip-bound item ahead of a runnable one is the first actionable item, so
    // it is dropped first (the pump re-plans and reaches the runnable one next).
    let items = [
        item(false, QueueCondition::IfPrevFailed), // succeeded → skip-bound
        item(false, QueueCondition::IfPrevSucceeded), // succeeded → runnable
    ];
    assert_eq!(
        plan_drain(&items, Some(outcome(true, false))),
        DrainAction::Drop(0)
    );
}

#[test]
fn condition_marker_glyphs_are_distinct_and_blank_for_always() {
    // Always paints invisible blanks (aligned with the other markers); every
    // conditional kind has a distinct non-blank glyph.
    assert_eq!(QueueCondition::Always.marker_glyph().trim(), "");
    let glyphs = [
        QueueCondition::IfPrevSucceeded.marker_glyph(),
        QueueCondition::IfPrevFailed.marker_glyph(),
        QueueCondition::IfPrevEdited.marker_glyph(),
        QueueCondition::IfPrevNoEdits.marker_glyph(),
        QueueCondition::Manual.marker_glyph(),
    ];
    for g in glyphs {
        assert!(!g.trim().is_empty());
    }
    // All distinct.
    let mut sorted = glyphs.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), glyphs.len(), "glyphs must be distinct");
}

#[test]
fn marker_span_width_is_constant_across_conditions() {
    // The marker is an inline prefix; every variant must paint the same column
    // width so the row layout (and the hit-test geometry) never shifts.
    let always = condition_marker_span(QueueCondition::Always, None);
    let manual = condition_marker_span(QueueCondition::Manual, None);
    let succeeded =
        condition_marker_span(QueueCondition::IfPrevSucceeded, Some(outcome(true, false)));
    let width = |s: &ratatui::text::Span<'_>| s.content.chars().count();
    assert_eq!(width(&always), width(&manual));
    assert_eq!(width(&always), width(&succeeded));
}
