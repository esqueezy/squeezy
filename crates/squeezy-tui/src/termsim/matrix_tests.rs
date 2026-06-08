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

/// Redact the per-run scratch workspace path the status line embeds so the
/// plain snapshot captures layout, not the unique tempdir nonce. The status
/// formatter may truncate the middle of the path before the `squeezy_termsim_*`
/// marker appears, so replace the whole model/workspace/status segment instead
/// of matching only the raw tempdir name.
fn redact_workspace(text: &str) -> String {
    const PREFIX: &str = "termsim-stub:termsim-model · ";
    text.split('\n')
        .map(|line| {
            if let Some(start) = line.find(PREFIX) {
                let workspace_start = start + PREFIX.len();
                if let Some(rel_end) = line[workspace_start..].find(" · ") {
                    let workspace_end = workspace_start + rel_end;
                    let mut redacted = String::with_capacity(line.len());
                    redacted.push_str(&line[..workspace_start]);
                    redacted.push_str("<workspace>");
                    redacted.push_str(&line[workspace_end..]);
                    return redacted;
                }
            }
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

#[test]
fn redact_workspace_handles_truncated_status_path() {
    let text = concat!(
        "termsim-stub:termsim-model · /var/folders/xx/T... · ru... Build mode\n",
        "Enter send"
    );

    assert_eq!(
        redact_workspace(text),
        "termsim-stub:termsim-model · <workspace> · ru... Build mode\nEnter send"
    );
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
