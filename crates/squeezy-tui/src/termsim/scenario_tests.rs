//! Unit tests for scenario-derived needles.
//!
//! Included into [`crate::termsim::scenario`] via `#[path]` per the repo test
//! layout. Gated with the rest of the term-matrix tree behind `term-matrix`.

use super::*;

/// Build a one-delta scenario whose last `AssistantDelta` is `body`.
fn scenario_with_delta(body: &str) -> Scenario {
    Scenario {
        name: "test",
        initial_size: (80, 40),
        steps: vec![Step::AssistantDelta(body.to_string()), Step::Frame],
    }
}

#[test]
fn wide_run_needle_extracts_longest_cjk_run() {
    // The fullwidth run is pulled out whole, with the ASCII tail dropped.
    let s = scenario_with_delta("你好世界你好世界 widereflowdone");
    assert_eq!(s.wide_run_needle().as_deref(), Some("你好世界你好世界"));
}

#[test]
fn wide_run_needle_picks_the_longest_of_several_runs() {
    // Two wide runs separated by ASCII: the longer one wins.
    let s = scenario_with_delta("你好 middle 世界中文字符串");
    assert_eq!(s.wide_run_needle().as_deref(), Some("世界中文字符串"));
}

#[test]
fn wide_run_needle_is_none_for_ascii_only_body() {
    let s = scenario_with_delta("plain ascii answer widthstormdone");
    assert_eq!(s.wide_run_needle(), None);
}

#[test]
fn wide_run_needle_uses_the_last_assistant_delta() {
    // A scenario with several deltas keys on the most recent one (the committed
    // tail), matching `latest_response_tail`.
    let s = Scenario {
        name: "test",
        initial_size: (80, 40),
        steps: vec![
            Step::AssistantDelta("早期文本 first".to_string()),
            Step::Frame,
            Step::AssistantDelta("最新的宽字符 last".to_string()),
            Step::Frame,
        ],
    };
    assert_eq!(s.wide_run_needle().as_deref(), Some("最新的宽字符"));
}

#[test]
fn shipped_wide_glyph_reflow_scenario_has_a_wide_needle() {
    // The shipped scenario the matrix gate keys on must actually expose a wide
    // run, or the new wide-run assertion would silently no-op.
    let s = shipped_scenarios()
        .into_iter()
        .find(|s| s.name == "wide_glyph_reflow")
        .expect("wide_glyph_reflow is shipped");
    let needle = s.wide_run_needle().expect("scenario commits a wide run");
    assert!(
        needle.chars().count() >= 8,
        "the wide run must be long enough to straddle the wrap column: {needle:?}"
    );
}
