//! Unit tests for the §12.10.3 dogfood telemetry collector.
//!
//! These exercise the collector primitives directly: counter recording, storm
//! detection, the copy histogram, bounded terminal-profile detection, the
//! accessibility tri-states, JSONL serialization (schema version + append-only
//! persistence), the `/metrics` snapshot lines, and — the headline invariant —
//! the privacy guarantee that no payload can ever reach a record. The end-to-end
//! wiring (a painted `draw_app` accumulates a frame, the keymap chord toggles
//! the overlay, the overlay paints through the real `render()`) is covered by
//! the integration tests in `lib_tests.rs` against the capture-sink guard.

use super::*;

/// A deterministic env lookup over a fixed table.
fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_string())
    }
}

#[test]
fn record_frame_accumulates_session_totals_and_maxes() {
    let mut m = TuiMetrics::default();
    m.record_frame(FrameSample {
        render_time: Duration::from_micros(500),
        bytes: 1000,
        cache_hits: 4,
        cache_misses: 1,
        longest_wrap: Duration::from_micros(200),
        coalesced_skip: false,
    });
    m.record_frame(FrameSample {
        render_time: Duration::from_micros(1500),
        bytes: 500,
        cache_hits: 6,
        cache_misses: 4,
        longest_wrap: Duration::from_micros(100),
        coalesced_skip: false,
    });
    let r = m.record();
    assert_eq!(r.frames, 2);
    assert_eq!(r.bytes_total, 1500, "bytes sum across frames");
    assert_eq!(r.cache_hits, 10);
    assert_eq!(r.cache_misses, 5);
    // mean = (500 + 1500) / 2 = 1000µs.
    assert_eq!(r.mean_render_micros, 1000);
    assert_eq!(r.max_render_micros, 1500, "slowest single frame");
    assert_eq!(
        r.longest_wrap_micros, 200,
        "longest wrap is a session-wide max, not the latest"
    );
    // 10 hits / 15 lookups = 66.67%.
    assert!((r.cache_hit_rate_pct - 66.67).abs() < 0.001, "{r:?}");
}

#[test]
fn mean_render_micros_is_zero_with_no_frames() {
    // No divide-by-zero on a fresh collector.
    let r = TuiMetrics::default().record();
    assert_eq!(r.frames, 0);
    assert_eq!(r.mean_render_micros, 0);
    assert_eq!(
        r.cache_hit_rate_pct, 100.0,
        "no lookups reads as a full hit rate, not NaN"
    );
}

#[test]
fn skipped_frames_count_independently_of_painted_frames() {
    let mut m = TuiMetrics::default();
    m.record_skipped_frame();
    m.record_skipped_frame();
    m.record_frame(FrameSample::default());
    let r = m.record();
    assert_eq!(r.frames, 1, "only the painted frame counts as a frame");
    assert_eq!(r.skipped_frames, 2);
}

#[test]
fn input_counters_track_each_event_class() {
    let mut m = TuiMetrics::default();
    m.record_key_input();
    m.record_key_input();
    m.record_mouse_input();
    m.record_paste_input();
    m.record_resize_input(0);
    let r = m.record();
    assert_eq!(r.key_inputs, 2);
    assert_eq!(r.mouse_inputs, 1);
    assert_eq!(r.paste_inputs, 1);
    assert_eq!(r.resize_inputs, 1);
}

#[test]
fn scroll_accumulates_delta_and_detects_storms() {
    let mut m = TuiMetrics::default();
    // First scroll: no prior, so no storm.
    m.record_scroll(3, 100);
    // Within the 50ms window of the previous → a storm.
    m.record_scroll(2, 120);
    // Outside the window → not a storm.
    m.record_scroll(1, 1000);
    let r = m.record();
    assert_eq!(r.scroll_delta_total, 6, "3 + 2 + 1 lines");
    assert_eq!(
        r.scroll_storms, 1,
        "only the close-following scroll counts as a storm"
    );
}

#[test]
fn resize_detects_storms_within_the_window() {
    let mut m = TuiMetrics::default();
    m.record_resize_input(0);
    m.record_resize_input(10); // within 50ms → storm
    m.record_resize_input(40); // within 50ms of 10 → storm
    m.record_resize_input(500); // outside → no storm
    let r = m.record();
    assert_eq!(r.resize_inputs, 4);
    assert_eq!(r.resize_storms, 2);
}

#[test]
fn copy_histogram_buckets_by_bounded_provider() {
    let mut m = TuiMetrics::default();
    m.record_copy(CopyProvider::Osc52);
    m.record_copy(CopyProvider::Osc52);
    m.record_copy(CopyProvider::Platform);
    m.record_copy(CopyProvider::TempFile);
    assert_eq!(m.copy_count(CopyProvider::Osc52), 2);
    assert_eq!(m.copy_count(CopyProvider::Platform), 1);
    assert_eq!(m.copy_count(CopyProvider::TempFile), 1);
    let r = m.record();
    assert_eq!(r.copy_osc52, 2);
    assert_eq!(r.copy_platform, 1);
    assert_eq!(r.copy_tempfile, 1);
}

#[test]
fn accessibility_signals_are_tri_state_until_recorded() {
    let mut m = TuiMetrics::default();
    let before = m.record();
    assert_eq!(before.reduced_motion, None);
    assert_eq!(before.high_contrast, None);
    m.set_accessibility(true, false);
    let after = m.record();
    assert_eq!(after.reduced_motion, Some(true));
    assert_eq!(after.high_contrast, Some(false));
}

#[test]
fn terminal_profile_macos_maps_emulators_to_bounded_labels() {
    let detect = |prog: &str| {
        TerminalProfile::detect_from(OsFamily::Macos, env_from(&[("TERM_PROGRAM", prog)]))
    };
    assert_eq!(detect("iTerm.app"), TerminalProfile::MacosIterm2);
    assert_eq!(
        detect("Apple_Terminal"),
        TerminalProfile::MacosAppleTerminal
    );
    assert_eq!(detect("WezTerm"), TerminalProfile::MacosWezterm);
    assert_eq!(detect("ghostty"), TerminalProfile::MacosGhostty);
    assert_eq!(detect("kitty"), TerminalProfile::MacosKitty);
    assert_eq!(detect("vscode"), TerminalProfile::MacosVscode);
    assert_eq!(detect("some-unknown-emulator"), TerminalProfile::Unknown);
}

#[test]
fn terminal_profile_linux_distinguishes_tmux_vscode_xterm() {
    assert_eq!(
        TerminalProfile::detect_from(
            OsFamily::Linux,
            env_from(&[("TMUX", "/tmp/tmux-1000/default")])
        ),
        TerminalProfile::LinuxTmux,
        "an active $TMUX wins regardless of TERM_PROGRAM"
    );
    assert_eq!(
        TerminalProfile::detect_from(OsFamily::Linux, env_from(&[("TERM_PROGRAM", "tmux")])),
        TerminalProfile::LinuxTmux,
        "tmux 3.3+ overwrites TERM_PROGRAM to tmux"
    );
    assert_eq!(
        TerminalProfile::detect_from(OsFamily::Linux, env_from(&[("TERM_PROGRAM", "vscode")])),
        TerminalProfile::LinuxVscode
    );
    assert_eq!(
        TerminalProfile::detect_from(OsFamily::Linux, env_from(&[("TERM", "xterm-256color")])),
        TerminalProfile::LinuxXterm
    );
    assert_eq!(
        TerminalProfile::detect_from(OsFamily::Linux, env_from(&[])),
        TerminalProfile::Unknown
    );
}

#[test]
fn terminal_profile_windows_distinguishes_terminal_from_conhost() {
    assert_eq!(
        TerminalProfile::detect_from(OsFamily::Windows, env_from(&[("WT_SESSION", "abc")])),
        TerminalProfile::WindowsTerminal
    );
    assert_eq!(
        TerminalProfile::detect_from(OsFamily::Windows, env_from(&[])),
        TerminalProfile::WindowsConhost
    );
}

#[test]
fn every_terminal_profile_label_is_a_fixed_bounded_string() {
    // The spec's hard rule: the terminal profile is recorded ONLY as a bounded
    // enum-like value, never a raw env var / hostname / path. Assert every label
    // is one of the audited slugs and contains no separators a path/host would.
    let labels = profile_labels();
    assert_eq!(labels.len(), TERMINAL_PROFILES.len());
    for label in &labels {
        assert!(
            label
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "profile label {label:?} must be a lowercase_snake slug, never raw env"
        );
        assert!(!label.contains('/'), "no path separators: {label:?}");
        assert!(!label.contains('.'), "no host dots: {label:?}");
    }
    // The exact bounded set the spec names is present.
    for expected in ["macos_iterm2", "linux_tmux", "windows_terminal", "unknown"] {
        assert!(labels.contains(&expected), "missing {expected:?}");
    }
}

#[test]
fn jsonl_line_carries_the_schema_version_and_is_a_single_object() {
    let mut m = TuiMetrics::default();
    m.set_terminal_profile(TerminalProfile::MacosIterm2);
    m.record_frame(FrameSample {
        render_time: Duration::from_micros(900),
        bytes: 42,
        cache_hits: 1,
        cache_misses: 0,
        longest_wrap: Duration::ZERO,
        coalesced_skip: false,
    });
    let line = m.to_jsonl();
    assert!(!line.contains('\n'), "one JSONL record is a single line");
    let v: serde_json::Value = serde_json::from_str(&line).expect("valid JSON object");
    assert_eq!(v["schema_version"], SCHEMA_VERSION);
    assert_eq!(v["frames"], 1);
    assert_eq!(v["bytes_total"], 42);
    assert_eq!(v["terminal_profile"], "macos_iterm2");
}

#[test]
fn flush_jsonl_is_a_noop_until_a_path_is_set() {
    let m = TuiMetrics::default();
    assert!(!m.jsonl_enabled(), "persistence is opt-in / off by default");
    assert!(
        !m.flush_jsonl().expect("noop flush"),
        "no path → no write, no error"
    );
}

#[test]
fn flush_jsonl_appends_one_line_per_flush() {
    let dir = std::env::temp_dir().join(format!("dogfood-jsonl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mk tmp dir");
    let path = dir.join("metrics.jsonl");
    let _ = std::fs::remove_file(&path);

    let mut m = TuiMetrics::default();
    m.set_terminal_profile(TerminalProfile::LinuxTmux);
    m.set_jsonl_path(path.clone());
    assert!(m.jsonl_enabled());

    m.record_key_input();
    assert!(m.flush_jsonl().expect("first append"));
    m.record_key_input();
    assert!(m.flush_jsonl().expect("second append"));

    let contents = std::fs::read_to_string(&path).expect("read back");
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 2, "append-only: one line per flush");
    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).expect("each line is JSON");
        assert_eq!(v["schema_version"], SCHEMA_VERSION);
    }
    // The second snapshot reflects the second key input (2 vs 1).
    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(first["key_inputs"], 1);
    assert_eq!(second["key_inputs"], 2);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn snapshot_lines_cover_every_counter_group() {
    let mut m = TuiMetrics::default();
    m.set_terminal_profile(TerminalProfile::MacosIterm2);
    m.set_accessibility(true, false);
    m.record_frame(FrameSample {
        render_time: Duration::from_micros(1500),
        bytes: 2048,
        cache_hits: 9,
        cache_misses: 1,
        longest_wrap: Duration::from_micros(250),
        coalesced_skip: false,
    });
    m.record_key_input();
    m.record_copy(CopyProvider::Osc52);
    let joined = m.snapshot_lines().join("\n");
    assert!(joined.contains("dogfood telemetry"), "{joined}");
    assert!(joined.contains("frames  1"), "{joined}");
    assert!(joined.contains("bytes   2048"), "{joined}");
    assert!(joined.contains("cache   9/10 (90%)"), "{joined}");
    assert!(joined.contains("osc52 1"), "copy histogram: {joined}");
    assert!(joined.contains("macos_iterm2"), "{joined}");
    assert!(joined.contains("motion on contrast off"), "a11y: {joined}");
}

#[test]
fn emergency_teardown_counter_is_process_wide_and_monotonic() {
    // The teardown count lives in a process-wide atomic (the panic/Drop paths
    // have no app handle). It is monotonic; assert a recorded bump shows up in a
    // fresh collector's snapshot. (Process-wide, so we compare a delta rather
    // than an absolute value to stay robust to other tests in the binary.)
    let before = emergency_teardowns();
    record_emergency_teardown();
    let after = TuiMetrics::default().record().emergency_teardowns;
    assert!(
        after > before,
        "a recorded emergency teardown is visible in the snapshot ({before} -> {after})"
    );
}

#[test]
fn fake_secret_never_reaches_a_record_or_jsonl_line() {
    // The §12.10.3 privacy gate: shove a sentinel "secret" at every entry point
    // that takes external data and assert it can never appear in the serialized
    // record. The collector's API is numeric / bounded-enum only, so there is no
    // entry point that *accepts* a string payload — this test documents and
    // locks that: even a malicious terminal profile env value collapses to a
    // bounded label, and the JSONL never contains the sentinel.
    const SECRET: &str = "S3CR3T-prompt-and-/home/user/path-and-token";
    let mut m = TuiMetrics::default();
    // A profile detected from an env var carrying the secret must NOT store it.
    let profile = TerminalProfile::detect_from(
        OsFamily::Linux,
        env_from(&[("TERM_PROGRAM", SECRET), ("TERM", SECRET)]),
    );
    m.set_terminal_profile(profile);
    // Drive every numeric recorder; none takes a payload, but exercise them so
    // the serialized line is non-trivial.
    m.record_frame(FrameSample {
        render_time: Duration::from_micros(10),
        bytes: 1,
        cache_hits: 1,
        cache_misses: 1,
        longest_wrap: Duration::from_micros(1),
        coalesced_skip: true,
    });
    m.record_key_input();
    m.record_mouse_input();
    m.record_paste_input();
    m.record_resize_input(0);
    m.record_scroll(5, 0);
    m.record_copy(CopyProvider::Platform);
    m.set_accessibility(true, true);

    let line = m.to_jsonl();
    assert!(
        !line.contains("S3CR3T") && !line.contains("/home/user") && !line.contains("token"),
        "no fragment of the secret may reach the JSONL: {line}"
    );
    // And the profile collapsed to a bounded label, not the raw env value.
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        v["terminal_profile"], "unknown",
        "an unrecognised env value collapses to the bounded `unknown`, never the raw string"
    );
    assert!(profile_labels().contains(&v["terminal_profile"].as_str().unwrap()));
}
