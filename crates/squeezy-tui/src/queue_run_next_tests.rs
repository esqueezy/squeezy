//! Unit tests for the pure "Run Selected Queued Next" decision (§11G.9).

use super::*;

#[test]
fn empty_queue_is_a_noop() {
    assert_eq!(plan(0, 0, false), None);
    assert_eq!(plan(0, 0, true), None);
    // A stale cursor on an empty queue is still a no-op.
    assert_eq!(plan(3, 0, false), None);
}

#[test]
fn out_of_range_selection_is_a_noop() {
    // selected == len and selected > len both fall outside.
    assert_eq!(plan(2, 2, false), None);
    assert_eq!(plan(9, 3, true), None);
}

#[test]
fn idle_promotes_and_runs_now() {
    let p = plan(2, 4, false).expect("in-range selection");
    assert_eq!(p.from, 2);
    assert!(p.moves, "a non-front row must move to the front");
    assert!(p.run_now, "idle requests arm an immediate drain");
}

#[test]
fn busy_promotes_but_defers_run() {
    let p = plan(3, 5, true).expect("in-range selection");
    assert_eq!(p.from, 3);
    assert!(p.moves, "a non-front row still moves while busy");
    assert!(
        !p.run_now,
        "a running turn must not be pre-empted; the front prompt runs on finish"
    );
}

#[test]
fn front_row_does_not_move_but_idle_still_runs() {
    let p = plan(0, 3, false).expect("front row is in range");
    assert_eq!(p.from, 0);
    assert!(!p.moves, "the front row needs no reorder");
    assert!(
        p.run_now,
        "an already-front prompt still runs immediately when idle"
    );
}

#[test]
fn front_row_while_busy_is_inert() {
    // Front + busy: no move and no run. The caller surfaces this as a no-op.
    let p = plan(0, 4, true).expect("front row is in range");
    assert!(!p.moves);
    assert!(!p.run_now);
}

#[test]
fn single_item_queue_front_is_in_range() {
    // The only item is the front; idle runs it, busy is inert.
    let idle = plan(0, 1, false).expect("single item");
    assert!(!idle.moves);
    assert!(idle.run_now);
    let busy = plan(0, 1, true).expect("single item");
    assert!(!busy.moves);
    assert!(!busy.run_now);
}

#[test]
fn last_row_promotes_from_the_back() {
    let len = 6;
    let p = plan(len - 1, len, false).expect("last row in range");
    assert_eq!(p.from, len - 1);
    assert!(p.moves);
    assert!(p.run_now);
}
