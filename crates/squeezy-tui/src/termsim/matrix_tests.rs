use super::*;
use crate::termsim::scenario::Scenario;

/// The full matrix gate: every scenario × surface × backend passes the
/// §8.5 invariants.
#[test]
fn term_matrix() {
    run_matrix();
}

/// Find a shipped scenario by name for the focused snapshot tests.
fn scenario_named(name: &str) -> Scenario {
    shipped_scenarios()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("scenario {name:?} is shipped"))
}

/// Redact the per-run scratch workspace path the status line embeds
/// (`…/squeezy_termsim_<nonce>…`, truncated with an ellipsis) so the plain
/// snapshot captures layout, not the unique nonce. insta's `filters`
/// feature is not enabled in this workspace, so we redact by hand: replace
/// any whitespace-delimited token containing `squeezy_termsim_` with a
/// stable placeholder.
fn redact_workspace(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            line.split(' ')
                .map(|tok| {
                    if tok.contains("squeezy_termsim_") {
                        "<workspace>"
                    } else {
                        tok
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Snapshot the fullscreen grid with the volatile scratch-workspace path
/// redacted, so the plain snapshot is deterministic across runs.
fn assert_grid_snapshot(name: &str, grid: &Grid) {
    let redacted = redact_workspace(&grid.viewport.join("\n"));
    insta::assert_snapshot!(name, redacted);
}

/// Plain grid snapshot of a settled fullscreen frame for `single_turn`.
/// Catches layout drift the contains-assertions miss (§8.5 soft check).
/// We snapshot the fullscreen `render()` surface (the active main view),
/// trimmed per row, so the snapshot is the human-visible screen.
#[test]
fn snapshot_single_turn_fullscreen() {
    let run = run_scenario(&scenario_named("single_turn"));
    let grid = frame_to_grid(&run.final_frame);
    assert_grid_snapshot("single_turn_fullscreen", &grid);
}

/// Plain grid snapshot of the settled fullscreen frame after the overlay
/// round trip, proving the Ctrl+T alt-screen path returns cleanly to one
/// composer horizon.
#[test]
fn snapshot_overlay_round_trip_fullscreen() {
    let run = run_scenario(&scenario_named("overlay_round_trip"));
    let grid = frame_to_grid(&run.final_frame);
    assert_grid_snapshot("overlay_round_trip_fullscreen", &grid);
}
