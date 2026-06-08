//! The cartesian (scenario × backend) matrix runner and the feature-gated
//! `#[test]` entry point.
//!
//! Runs every shipped scenario against both always-on Rust emulator legs
//! (vt100, alacritty_terminal) in-process, so a contributor running
//! `--features term-matrix` exercises the matrix with no node and no PTY. The
//! xterm.js leg stays out-of-process (CI only) and is not invoked here.
//!
//! ## What is asserted where
//!
//! Two surfaces, two invariant sets (see [`super::driver::ScenarioRun`]):
//!
//! * The **fullscreen `render()`** snapshot (`final_frame`) is the main view in
//!   use at this stage of the migration. The §8.5 *content* invariants —
//!   ≤ 1 composer horizon, no duplicated turn divider, latest response present
//!   after resize — assert against it, because the inline path commits history
//!   to scrollback the emulator grid never surfaces.
//! * The **append-only inline** stream (`log`), replayed through each emulator
//!   leg, is what real terminals show. The cursor-in-bounds invariant asserts
//!   against *its* reconstructed grid, per backend, so an emulator that drifts
//!   the cursor below the live region (the actual xterm.js bug) is caught.

use super::assertions;
use super::driver::{ScenarioRun, run_scenario};
use super::emulator::{Emulator, all_backends};
use super::scenario::{Scenario, shipped_scenarios};
use super::types::{FrameMark, Grid};

/// Project a fullscreen `render()` snapshot's plain text into a [`Grid`] so the
/// content invariants can run against the active main-view surface uniformly
/// with the emulator-replayed grids.
fn frame_to_grid(frame: &crate::testing::FrameSnapshot) -> Grid {
    let viewport: Vec<String> = frame
        .plain_text
        .lines()
        .map(|line| line.trim_end().to_string())
        .collect();
    Grid {
        viewport,
        ..Grid::default()
    }
}

/// The most turn dividers a scenario can legitimately show at once in the
/// fullscreen view. Scenarios that commit no assistant turn show 0; a committed
/// turn can show its single divider.
fn max_turn_dividers(scenario: &Scenario) -> usize {
    if scenario.latest_response_tail().is_some() {
        1
    } else {
        0
    }
}

/// Run the content invariants (composer horizon / turn divider / latest
/// response) against the settled fullscreen `render()` surface.
fn assert_fullscreen_invariants(scenario: &Scenario, run: &ScenarioRun) {
    let grid = frame_to_grid(&run.final_frame);

    assertions::at_most_one_composer_horizon(&grid)
        .unwrap_or_else(|e| panic!("[{}] fullscreen: {e}", scenario.name));

    assertions::no_duplicate_turn_divider(&grid, max_turn_dividers(scenario))
        .unwrap_or_else(|e| panic!("[{}] fullscreen: {e}", scenario.name));

    assertions::latest_response_present(&grid, &run.latest_response_tail)
        .unwrap_or_else(|e| panic!("[{}] fullscreen: {e}", scenario.name));
}

/// Replay the captured inline stream through one backend and assert the
/// per-emulator invariants (cursor bounds against the final frame size, no
/// horizon stacking in the reconstructed live region).
fn assert_emulator_invariants(
    scenario: &Scenario,
    backend_name: &str,
    emulator: &dyn Emulator,
    run: &ScenarioRun,
) {
    let grid = emulator.replay(&run.log);

    // Cursor must stay within the final frame's height. The last recorded mark
    // carries the size in effect for the final paint; fall back to the
    // fullscreen frame height when a scenario somehow recorded no marks.
    let final_mark = run.log.frames.last().copied().unwrap_or(FrameMark {
        byte_offset: run.log.bytes.len(),
        w: run.final_frame.width,
        h: run.final_frame.height,
    });
    assertions::cursor_row_in_bounds(&grid, final_mark)
        .unwrap_or_else(|e| panic!("[{} / {backend_name}] inline replay: {e}", scenario.name));

    // The live region the append-only path leaves on screen is the footer
    // composer; it must never stack a second horizon.
    assertions::at_most_one_composer_horizon(&grid)
        .unwrap_or_else(|e| panic!("[{} / {backend_name}] inline replay: {e}", scenario.name));
}

/// Run every shipped scenario against both Rust emulator legs and the
/// fullscreen surface, asserting the §8.5 invariants.
fn run_matrix() {
    let backends = all_backends();
    for scenario in &shipped_scenarios() {
        let run = run_scenario(scenario);

        // Content invariants against the active main-view (fullscreen) surface.
        assert_fullscreen_invariants(scenario, &run);

        // Per-emulator invariants against the replayed inline stream.
        for backend in &backends {
            assert_emulator_invariants(scenario, backend.name, backend.emulator.as_ref(), &run);
        }
    }
}

#[cfg(test)]
#[path = "matrix_tests.rs"]
mod tests;
