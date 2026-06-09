use super::*;
use crate::termsim::scenario::Scenario;
use std::sync::Arc;

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

/// The fullscreen main-view path shows ZERO stacked composer horizons across
/// the two resize storms (`width_drag_storm`, `height_storm`) — the exact bug
/// the alt-screen migration fixes — and the latest assistant response is still
/// present afterward.
///
/// This is the strengthened, non-vacuous form of the matrix's content
/// invariants, asserted directly against the settled fullscreen `render()`
/// surface at each storm's FINAL size:
///
/// * **Exactly one** live composer horizon (one live composer, zero stacked).
///   The fullscreen grid always pins the composer, so a healthy frame has
///   exactly one; `> 1` is the stacked-divider regression, `0` would mean the
///   composer vanished. Asserting `== 1` is strictly stronger than the matrix's
///   `<= 1` upper bound.
/// * **Latest response present, non-vacuously.** We first assert the scenario
///   actually commits a non-empty response tail (so the needle is real, not the
///   empty string that `latest_response_present` passes vacuously), then assert
///   that needle survives the storm in the fullscreen grid. A scenario whose
///   tail silently went empty would now fail here instead of passing for free.
#[test]
fn fullscreen_main_view_survives_resize_storms_without_stacking() {
    for name in ["width_drag_storm", "height_storm"] {
        let scenario = scenario_named(name);

        // Non-vacuity guard: the scenario must commit a real, non-empty tail,
        // otherwise the latest-response check below would pass for free.
        let tail = scenario
            .latest_response_tail()
            .unwrap_or_else(|| panic!("[{name}] scenario must commit an assistant response tail"));
        assert!(
            !tail.is_empty(),
            "[{name}] latest-response needle must be non-empty (non-vacuous check)"
        );

        let run = run_scenario(&scenario);
        let grid = frame_to_grid(&run.final_frame);

        // Exactly one live composer horizon: one live composer, zero stacked.
        let horizons = assertions::composer_horizon_rows(&grid);
        assert_eq!(
            horizons.len(),
            1,
            "[{name}] fullscreen main view must show exactly one composer horizon \
             (one live, zero stacked), found rows {horizons:?}\n--- grid ---\n{}",
            grid.viewport.join("\n"),
        );

        // The committed response tail survives the storm on the fullscreen
        // surface — and `run.latest_response_tail` is the very tail we just
        // proved non-empty, so this assertion is genuinely load-bearing.
        assert_eq!(
            run.latest_response_tail, tail,
            "[{name}] run tail should match the scenario's committed tail"
        );
        assertions::latest_response_present(&grid, &run.latest_response_tail).unwrap_or_else(|e| {
            panic!(
                "[{name}] fullscreen: {e}\n--- grid ---\n{}",
                grid.viewport.join("\n")
            )
        });
    }
}

/// A `LlmProvider` that never streams: the scenario drives the transcript
/// directly via `AssistantDelta`, so the provider only has to exist and name
/// itself. Mirrors `driver::StubProvider` (which is private to that module).
struct MirrorStubProvider;

impl squeezy_llm::LlmProvider for MirrorStubProvider {
    fn name(&self) -> &'static str {
        "termsim-stub"
    }

    fn stream_response(
        &self,
        _request: squeezy_llm::LlmRequest,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> squeezy_llm::LlmStream {
        Box::pin(futures_util::stream::empty())
    }
}

/// Drive a settled session through the real clean-exit path
/// (`TerminalGuard::finish_fullscreen`) and assert, on the captured byte stream,
/// that `LeaveAlternateScreen` precedes the mirrored response text — so the
/// collapsed transcript lands in real scrollback, not the alternate screen.
///
/// This is the term-matrix scenario form of the Phase 2 byte-order contract: it
/// replays the shipped `single_turn` scenario (a streamed delta that settles
/// into a committed assistant turn) through the same headless `TuiHarness` the
/// matrix uses, then hands the settled `TuiApp` to a fullscreen guard wired to a
/// `TerminalWriter::Capture` sink and runs the production `finish_fullscreen`.
#[test]
fn clean_exit_mirror() {
    let scenario = scenario_named("single_turn");

    // The committed response tail is the concrete needle we assert survives into
    // scrollback; the scenario derives it from its own script, so it can't drift.
    let tail = scenario
        .latest_response_tail()
        .expect("single_turn commits an assistant response tail");
    assert!(!tail.is_empty(), "needle must be non-empty (non-vacuous)");

    let (w, h) = scenario.initial_size;
    let config = squeezy_core::AppConfig {
        model: "termsim-model".to_string(),
        workspace_root: std::env::temp_dir().join(format!(
            "squeezy_termsim_clean_exit_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )),
        ..squeezy_core::AppConfig::default()
    };
    let _ = std::fs::create_dir_all(&config.workspace_root);
    let provider: Arc<dyn squeezy_llm::LlmProvider> = Arc::new(MirrorStubProvider);
    let mut harness = crate::testing::TuiHarness::new(
        config,
        squeezy_core::SessionMode::Build,
        provider,
        w,
        h,
        None,
    )
    .expect("termsim harness builds with stub provider");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("termsim tokio runtime");
    // Drive the scenario to a settled, committed transcript (the AssistantDelta +
    // SettleTurn steps land and flush the assistant turn into history).
    runtime
        .block_on(harness.drive_scenario(&scenario.steps))
        .expect("drive_scenario settles the single_turn session");

    // Sanity: the settled session actually holds the committed response, so the
    // mirror assertion below is non-vacuous.
    let settled_text = harness.last_assistant_text();
    assert!(
        settled_text.contains(&tail),
        "settled session must hold the committed response tail {tail:?}; got {settled_text:?}",
    );

    // Clean exit: a fullscreen guard on a capture sink, pointed at the settled
    // app, run through the production `finish_fullscreen`. The injected
    // `FixedSize` feeds the mirror width with no real TTY.
    let (mut guard, sink) =
        crate::TerminalGuard::for_capture_test(/* inline_repro = */ false, w, h);
    guard.set_exit_hint(Some("Resume: squeezy sessions resume cafef00d".to_string()));
    guard
        .finish_fullscreen(harness.app_mut())
        .expect("clean exit mirrors the settled transcript");

    let ansi = {
        let bytes = sink.lock().expect("capture sink lock").clone();
        String::from_utf8(bytes).expect("captured ANSI is valid utf8")
    };

    // The defining contract: LeaveAlternateScreen precedes the mirrored response.
    let leave_pos = ansi
        .find("\x1b[?1049l")
        .expect("clean exit must leave the alternate screen");
    let response_pos = ansi
        .find(&tail)
        .expect("clean exit must mirror the committed assistant response into scrollback");
    assert!(
        leave_pos < response_pos,
        "LeaveAlternateScreen (offset {leave_pos}) must precede the mirrored response \
         {tail:?} (offset {response_pos}) so the mirror lands in real scrollback",
    );
    // A clean exit never purges the user's pre-launch scrollback.
    assert!(
        !ansi.contains("\x1b[3J"),
        "clean exit must not purge scrollback (\\x1b[3J)",
    );
}
