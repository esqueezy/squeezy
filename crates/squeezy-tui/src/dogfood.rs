//! Dogfood telemetry counters (§12.10.3).
//!
//! Phase 8 added [`crate::metrics::RenderMetrics`] (a per-*frame* render-budget
//! snapshot) and §12.10.1 added [`crate::latency::LatencyTracker`] (per-
//! *interaction* p95/p99 budgets). Both answer "is the *current* frame fast?".
//! Neither accumulates a *session-long, privacy-preserving* picture of renderer
//! quality across terminals — the data a maintainer dogfooding the TUI on many
//! machines needs to explain *why* one terminal feels worse than another.
//!
//! This module is that missing collector: [`TuiMetrics`], an in-memory bag of
//! purely numeric / enumerated counters, plus optional append-only JSONL
//! persistence. It is the §12.10.3 "Dogfood Telemetry Counters" backlog item.
//!
//! ## Privacy is the headline invariant
//!
//! The spec is emphatic: the schema **cannot** contain prompt / transcript /
//! command / path / env / clipboard / model text. Every field on [`TuiMetrics`]
//! and on the serialized [`MetricsRecord`] is therefore one of:
//!
//! - a `u64` / `usize` counter (frames, bytes, skipped frames, cache hits, input
//!   counts, resize storms, scroll deltas, …),
//! - a `Duration`-derived integer (micros), or
//! - a **bounded enum** rendered as a fixed, hand-audited string
//!   ([`TerminalProfile`], [`CopyProvider`]).
//!
//! There is *no* free-form `String` field that a payload could ever reach. The
//! terminal profile is an enum like `macos_iterm2` / `linux_tmux` /
//! `windows_terminal` (the spec's exact examples), **not** a raw `$TERM` /
//! `$TERM_PROGRAM` / hostname / username / path. The
//! [`tests`](mod@tests) module includes a "fake secret" privacy test that
//! shoves a sentinel string at every recording entry point and asserts it can
//! never appear in the serialized JSONL — the dogfood gate the spec calls for
//! before the inline renderer was deleted.
//!
//! ## Idle-redraw contract
//!
//! Like the render-budget metrics, [`TuiMetrics`] is only ever *recorded into*
//! at points that already do real work — a painted frame, a key/mouse event, a
//! copy, a resize. There is no background timer and no idle recording: an idle
//! frame that paints nothing records nothing, so the zero-idle-work invariant
//! holds. The collector default-constructs to all-zero and allocates nothing
//! until [`TuiMetrics::set_jsonl_path`] opts in to persistence.
//!
//! ## Cost
//!
//! Recording is a handful of integer adds / maxes and (for the bounded enums) a
//! tiny saturating counter bump — none of it scales with transcript size. JSONL
//! persistence, when enabled, appends one line per *flush* (not per frame); the
//! caller decides when to flush (e.g. at teardown), so the hot path never
//! touches the disk.

#![cfg_attr(not(unix), allow(dead_code))]

use std::time::Duration;

/// The on-disk JSONL schema version. Bumped whenever the [`MetricsRecord`]
/// shape changes so a later reader can migrate or reject old lines. The spec
/// calls out "JSONL versioning" explicitly; every persisted line carries this.
pub(crate) const SCHEMA_VERSION: u32 = 1;

/// A bounded, enum-like terminal profile. The spec is explicit that the
/// platform/terminal may be recorded **only** as bounded enum-like values such
/// as `macos_iterm2`, `linux_tmux`, or `windows_terminal` — never raw
/// environment variables, usernames, paths, shell commands, clipboard contents,
/// or hostnames. This enum *is* that bound: it is derived from `$TERM_PROGRAM` /
/// `$TERM` / platform by [`TerminalProfile::detect_from`], but the only thing
/// that ever escapes into a record is one of these hand-audited variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum TerminalProfile {
    MacosIterm2,
    MacosAppleTerminal,
    MacosWezterm,
    MacosGhostty,
    MacosKitty,
    MacosVscode,
    LinuxTmux,
    LinuxVscode,
    LinuxXterm,
    WindowsTerminal,
    WindowsConhost,
    /// Anything not matched above. Deliberately coarse — better an honest
    /// "unknown" than a free-form string that could leak `$TERM`.
    Unknown,
}

/// Every terminal-profile variant, in display order. A free `const` (not an
/// inherent `ALL`) so it is reachable from both the privacy / exhaustiveness
/// tests and any future profile table without tripping the dead-code lint when
/// prod happens to reference it only here. Referenced by [`profile_labels`].
pub(crate) const TERMINAL_PROFILES: &[TerminalProfile] = &[
    TerminalProfile::MacosIterm2,
    TerminalProfile::MacosAppleTerminal,
    TerminalProfile::MacosWezterm,
    TerminalProfile::MacosGhostty,
    TerminalProfile::MacosKitty,
    TerminalProfile::MacosVscode,
    TerminalProfile::LinuxTmux,
    TerminalProfile::LinuxVscode,
    TerminalProfile::LinuxXterm,
    TerminalProfile::WindowsTerminal,
    TerminalProfile::WindowsConhost,
    TerminalProfile::Unknown,
];

/// The fixed allow-list of every terminal-profile label. Single-sourced from
/// [`TERMINAL_PROFILES`] so the privacy test can assert that no label is ever a
/// raw env value, and so a new profile cannot escape the audit. Exposed (and
/// used) in prod by the dogfood JSONL schema-documentation accessor.
pub(crate) fn profile_labels() -> Vec<&'static str> {
    TERMINAL_PROFILES.iter().map(|p| p.as_str()).collect()
}

impl TerminalProfile {
    /// The fixed, hand-audited enum label that goes into a record / overlay.
    /// These are the ONLY terminal-identifying strings that ever leave this
    /// module — there is no path from a raw env var to a record.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            TerminalProfile::MacosIterm2 => "macos_iterm2",
            TerminalProfile::MacosAppleTerminal => "macos_apple_terminal",
            TerminalProfile::MacosWezterm => "macos_wezterm",
            TerminalProfile::MacosGhostty => "macos_ghostty",
            TerminalProfile::MacosKitty => "macos_kitty",
            TerminalProfile::MacosVscode => "macos_vscode",
            TerminalProfile::LinuxTmux => "linux_tmux",
            TerminalProfile::LinuxVscode => "linux_vscode",
            TerminalProfile::LinuxXterm => "linux_xterm",
            TerminalProfile::WindowsTerminal => "windows_terminal",
            TerminalProfile::WindowsConhost => "windows_conhost",
            TerminalProfile::Unknown => "unknown",
        }
    }

    /// Classify a terminal from an injected environment lookup and the host OS
    /// family. `env_get` is injected (not `std::env`) so the mapping is testable
    /// without mutating process env, and — critically — so the *only* thing this
    /// function can do with the raw values is `==`-compare them against a fixed
    /// allow-list. A value it does not recognise collapses to [`Self::Unknown`];
    /// the raw string is dropped on the floor and never stored.
    pub(crate) fn detect_from<F>(os_family: OsFamily, env_get: F) -> TerminalProfile
    where
        F: Fn(&str) -> Option<String>,
    {
        // tmux overwrites $TERM_PROGRAM to "tmux" on 3.3+, so check it first and
        // independently of the OS-specific emulator matching below.
        let term_program = env_get("TERM_PROGRAM").map(|v| v.to_ascii_lowercase());
        let inside_tmux = env_get("TMUX").is_some()
            || term_program.as_deref() == Some("tmux")
            || env_get("TERM")
                .map(|t| t.to_ascii_lowercase().contains("tmux"))
                .unwrap_or(false);
        let prog = term_program.as_deref().unwrap_or("");
        match os_family {
            OsFamily::Macos => match prog {
                _ if prog.contains("iterm") => TerminalProfile::MacosIterm2,
                "apple_terminal" => TerminalProfile::MacosAppleTerminal,
                "wezterm" => TerminalProfile::MacosWezterm,
                "ghostty" => TerminalProfile::MacosGhostty,
                "kitty" => TerminalProfile::MacosKitty,
                "vscode" => TerminalProfile::MacosVscode,
                _ => TerminalProfile::Unknown,
            },
            OsFamily::Linux => {
                if inside_tmux {
                    TerminalProfile::LinuxTmux
                } else if prog == "vscode" {
                    TerminalProfile::LinuxVscode
                } else if env_get("TERM")
                    .map(|t| t.to_ascii_lowercase().contains("xterm"))
                    .unwrap_or(false)
                {
                    TerminalProfile::LinuxXterm
                } else {
                    TerminalProfile::Unknown
                }
            }
            OsFamily::Windows => {
                if env_get("WT_SESSION").is_some() {
                    TerminalProfile::WindowsTerminal
                } else {
                    TerminalProfile::WindowsConhost
                }
            }
            OsFamily::Other => TerminalProfile::Unknown,
        }
    }
}

/// The host OS family, passed to [`TerminalProfile::detect_from`] so the OS leg
/// of the mapping is testable without `cfg!` at the call site. Resolved from
/// `std::env::consts::OS` at startup by [`OsFamily::current`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OsFamily {
    Macos,
    Linux,
    Windows,
    Other,
}

impl OsFamily {
    /// The family of the OS this binary was built for.
    pub(crate) fn current() -> OsFamily {
        match std::env::consts::OS {
            "macos" => OsFamily::Macos,
            "linux" | "android" => OsFamily::Linux,
            "windows" => OsFamily::Windows,
            _ => OsFamily::Other,
        }
    }
}

/// Which clipboard provider serviced a copy. A bounded enum (not the provider's
/// free-form label) so the copy-provider histogram can never carry a payload or
/// a platform command line. Mirrors the win/fallback shape of
/// [`crate::clipboard::ClipboardProviderKind`] but collapses its `&'static str`
/// platform label to a single bounded `Platform` variant — the label is a
/// hard-coded command name, but recording only the *kind* keeps the schema
/// trivially payload-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum CopyProvider {
    /// OSC 52 escape (in-band, terminal clipboard).
    Osc52,
    /// A platform command (pbcopy / wl-copy / xclip / clip.exe …).
    Platform,
    /// The temp-file fallback when every provider failed.
    TempFile,
}

impl CopyProvider {
    pub(crate) const ALL: &'static [CopyProvider] = &[
        CopyProvider::Osc52,
        CopyProvider::Platform,
        CopyProvider::TempFile,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            CopyProvider::Osc52 => "osc52",
            CopyProvider::Platform => "platform",
            CopyProvider::TempFile => "tempfile",
        }
    }

    /// Index into the fixed-size copy-provider histogram on [`TuiMetrics`].
    fn index(self) -> usize {
        match self {
            CopyProvider::Osc52 => 0,
            CopyProvider::Platform => 1,
            CopyProvider::TempFile => 2,
        }
    }
}

/// Whether successive scroll events arrive close enough together to count as a
/// "storm" (the spec's "resize storms" / rapid-input signal). The collector
/// counts a storm whenever an input of the same class lands within
/// [`STORM_WINDOW`] of the previous one of that class. Pure integer bookkeeping
/// over a caller-supplied monotonic millisecond clock — no `Instant` of its
/// own, so it stays testable and idle-free.
const STORM_WINDOW_MS: u64 = 50;

/// One painted frame's worth of inputs to [`TuiMetrics::record_frame`]. Grouping
/// them in a struct keeps the call site at the `draw_app` chokepoint readable
/// and makes it obvious every field is a plain number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FrameSample {
    /// Wall time the frame spent building + emitting (micros are what we store).
    pub(crate) render_time: Duration,
    /// Bytes emitted to the terminal this frame.
    pub(crate) bytes: u64,
    /// Cache lookups that hit this frame.
    pub(crate) cache_hits: u64,
    /// Cache lookups that missed (recomputed) this frame.
    pub(crate) cache_misses: u64,
    /// The slowest single-entry wrap `compute` this frame.
    pub(crate) longest_wrap: Duration,
    /// Whether the redraw gate skipped a *would-be* frame before this one (i.e.
    /// the loop coalesced redraw requests). Counts toward `skipped_frames`.
    pub(crate) coalesced_skip: bool,
}

/// The in-memory dogfood collector. All fields are numeric or a bounded-enum
/// histogram; there is deliberately no free-form string field. Lives on
/// `TuiApp`; the event loop and `draw_app` call the `record_*` methods at points
/// that already do real work.
#[derive(Clone, Debug, Default)]
pub(crate) struct TuiMetrics {
    // ---- frame / flush ----
    /// Painted frames recorded.
    frames: u64,
    /// Frames the redraw gate coalesced away (the spec's "skipped frames").
    skipped_frames: u64,
    /// Sum of per-frame render time, in micros (for a mean; no payload).
    render_micros_total: u64,
    /// Slowest single frame's render time, in micros.
    render_micros_max: u64,
    /// Total bytes emitted across all recorded frames.
    bytes_total: u64,
    /// Cache hits / misses summed across frames.
    cache_hits: u64,
    cache_misses: u64,
    /// Slowest single-entry wrap observed all session, in micros (wrap cost).
    longest_wrap_micros: u64,

    // ---- input ----
    /// Key events seen.
    key_inputs: u64,
    /// Mouse events seen.
    mouse_inputs: u64,
    /// Paste events seen.
    paste_inputs: u64,
    /// Resize events seen.
    resize_inputs: u64,
    /// Resize events that arrived within the storm window of the previous one.
    resize_storms: u64,
    /// Sum of absolute scroll deltas (lines), a cheap "how much scrolling".
    scroll_delta_total: u64,
    /// Scroll events that arrived within the storm window of the previous one.
    scroll_storms: u64,

    // ---- copy ----
    /// Per-provider copy-success histogram, indexed by [`CopyProvider::index`].
    copy_by_provider: [u64; 3],

    // ---- terminal / a11y / lifecycle ----
    /// The bounded terminal profile, set once at startup. `None` until detected.
    terminal_profile: Option<TerminalProfile>,
    /// Whether reduced-motion was active (a11y signal). `None` until recorded.
    reduced_motion: Option<bool>,
    /// Whether high-contrast was active (a11y signal). `None` until recorded.
    high_contrast: Option<bool>,

    // ---- storm clocks (not serialized) ----
    /// Last scroll event time (ms, caller clock) for storm detection.
    last_scroll_ms: Option<u64>,
    /// Last resize event time (ms, caller clock) for storm detection.
    last_resize_ms: Option<u64>,

    // ---- persistence (not serialized into the record itself) ----
    /// Where to append a JSONL line on [`Self::flush_jsonl`]. `None` = disabled
    /// (the default), so persistence is strictly opt-in.
    jsonl_path: Option<std::path::PathBuf>,
}

impl TuiMetrics {
    /// Record one painted frame's budget. Called from the `draw_app` chokepoint
    /// after the frame's [`crate::metrics::RenderMetrics`] is stamped, so the
    /// numbers are exactly what the HUD shows — just accumulated session-long.
    pub(crate) fn record_frame(&mut self, sample: FrameSample) {
        self.frames = self.frames.saturating_add(1);
        if sample.coalesced_skip {
            self.skipped_frames = self.skipped_frames.saturating_add(1);
        }
        let micros = duration_micros(sample.render_time);
        self.render_micros_total = self.render_micros_total.saturating_add(micros);
        self.render_micros_max = self.render_micros_max.max(micros);
        self.bytes_total = self.bytes_total.saturating_add(sample.bytes);
        self.cache_hits = self.cache_hits.saturating_add(sample.cache_hits);
        self.cache_misses = self.cache_misses.saturating_add(sample.cache_misses);
        self.longest_wrap_micros = self
            .longest_wrap_micros
            .max(duration_micros(sample.longest_wrap));
    }

    /// Record a coalesced (skipped) frame on its own — used when the loop drops
    /// a redraw request without ever reaching `record_frame` for it.
    pub(crate) fn record_skipped_frame(&mut self) {
        self.skipped_frames = self.skipped_frames.saturating_add(1);
    }

    /// Record one key input.
    pub(crate) fn record_key_input(&mut self) {
        self.key_inputs = self.key_inputs.saturating_add(1);
    }

    /// Record one mouse input.
    pub(crate) fn record_mouse_input(&mut self) {
        self.mouse_inputs = self.mouse_inputs.saturating_add(1);
    }

    /// Record one paste input.
    pub(crate) fn record_paste_input(&mut self) {
        self.paste_inputs = self.paste_inputs.saturating_add(1);
    }

    /// Record one resize input at caller-clock millisecond `now_ms`, counting a
    /// storm when it follows the previous resize within [`STORM_WINDOW_MS`].
    pub(crate) fn record_resize_input(&mut self, now_ms: u64) {
        self.resize_inputs = self.resize_inputs.saturating_add(1);
        if let Some(prev) = self.last_resize_ms
            && now_ms.saturating_sub(prev) <= STORM_WINDOW_MS
        {
            self.resize_storms = self.resize_storms.saturating_add(1);
        }
        self.last_resize_ms = Some(now_ms);
    }

    /// Record one scroll input of `lines` magnitude at caller-clock millisecond
    /// `now_ms`, accumulating the absolute delta and counting a storm when it
    /// follows the previous scroll within [`STORM_WINDOW_MS`].
    pub(crate) fn record_scroll(&mut self, lines: u64, now_ms: u64) {
        self.scroll_delta_total = self.scroll_delta_total.saturating_add(lines);
        if let Some(prev) = self.last_scroll_ms
            && now_ms.saturating_sub(prev) <= STORM_WINDOW_MS
        {
            self.scroll_storms = self.scroll_storms.saturating_add(1);
        }
        self.last_scroll_ms = Some(now_ms);
    }

    /// Record one successful copy serviced by `provider`.
    pub(crate) fn record_copy(&mut self, provider: CopyProvider) {
        let slot = &mut self.copy_by_provider[provider.index()];
        *slot = slot.saturating_add(1);
    }

    /// Set the bounded terminal profile (once, at startup).
    pub(crate) fn set_terminal_profile(&mut self, profile: TerminalProfile) {
        self.terminal_profile = Some(profile);
    }

    /// Record the accessibility signals (reduced motion / high contrast).
    pub(crate) fn set_accessibility(&mut self, reduced_motion: bool, high_contrast: bool) {
        self.reduced_motion = Some(reduced_motion);
        self.high_contrast = Some(high_contrast);
    }

    /// Opt in to append-only JSONL persistence at `path`. Until this is called
    /// (the default), [`Self::flush_jsonl`] is a no-op and nothing touches disk.
    pub(crate) fn set_jsonl_path(&mut self, path: std::path::PathBuf) {
        self.jsonl_path = Some(path);
    }

    /// Whether JSONL persistence is enabled.
    pub(crate) fn jsonl_enabled(&self) -> bool {
        self.jsonl_path.is_some()
    }

    /// The recorded success count for a single bounded copy provider.
    pub(crate) fn copy_count(&self, provider: CopyProvider) -> u64 {
        self.copy_by_provider[provider.index()]
    }

    /// Build the serializable snapshot of the current counters. This is the
    /// `/metrics` snapshot AND the JSONL line shape — single-sourced so the two
    /// can never diverge. Every field is a number or a bounded-enum string.
    pub(crate) fn record(&self) -> MetricsRecord {
        let mean_render_micros = self
            .render_micros_total
            .checked_div(self.frames)
            .unwrap_or(0);
        let lookups = self.cache_hits + self.cache_misses;
        let cache_hit_rate_pct = if lookups == 0 {
            100.0
        } else {
            (self.cache_hits as f64 / lookups as f64) * 100.0
        };
        MetricsRecord {
            schema_version: SCHEMA_VERSION,
            frames: self.frames,
            skipped_frames: self.skipped_frames,
            mean_render_micros,
            max_render_micros: self.render_micros_max,
            bytes_total: self.bytes_total,
            cache_hits: self.cache_hits,
            cache_misses: self.cache_misses,
            cache_hit_rate_pct: round2(cache_hit_rate_pct),
            longest_wrap_micros: self.longest_wrap_micros,
            key_inputs: self.key_inputs,
            mouse_inputs: self.mouse_inputs,
            paste_inputs: self.paste_inputs,
            resize_inputs: self.resize_inputs,
            resize_storms: self.resize_storms,
            scroll_delta_total: self.scroll_delta_total,
            scroll_storms: self.scroll_storms,
            copy_osc52: self.copy_by_provider[CopyProvider::Osc52.index()],
            copy_platform: self.copy_by_provider[CopyProvider::Platform.index()],
            copy_tempfile: self.copy_by_provider[CopyProvider::TempFile.index()],
            terminal_profile: self
                .terminal_profile
                .unwrap_or(TerminalProfile::Unknown)
                .as_str(),
            reduced_motion: self.reduced_motion,
            high_contrast: self.high_contrast,
            // Process-wide: the panic / signal / Drop teardown paths have no
            // `&mut TuiApp`, so the count lives in a lock-free atomic that those
            // paths bump and the snapshot folds in here.
            emergency_teardowns: emergency_teardowns(),
        }
    }

    /// Serialize the current snapshot to a single JSONL line (no trailing
    /// newline). Public for tests and for [`Self::flush_jsonl`].
    pub(crate) fn to_jsonl(&self) -> String {
        // `MetricsRecord` is `Serialize` and contains only numbers / bools /
        // `&'static str` enum labels, so this can never emit a payload. The
        // `unwrap_or` keeps a serialization hiccup from ever panicking the TUI.
        serde_json::to_string(&self.record()).unwrap_or_else(|_| "{}".to_string())
    }

    /// Append the current snapshot as one JSONL line to the configured path.
    /// A no-op (returns `Ok(false)`) when persistence is disabled. Returns
    /// `Ok(true)` when a line was written. Append-only and best-effort: it never
    /// reads, rewrites, or truncates an existing file.
    pub(crate) fn flush_jsonl(&self) -> std::io::Result<bool> {
        let Some(path) = self.jsonl_path.as_ref() else {
            return Ok(false);
        };
        let record = self.record();
        // Privacy guard (§12.10.3): the terminal profile is the only string in
        // the record, and it must always be one of the bounded, hand-audited
        // labels — never a raw env value. This can only fail if a future change
        // wired a non-enum string into the field; refuse to persist rather than
        // leak. A `debug_assert` makes a test catch it loudly; release degrades
        // to a no-op write skip.
        debug_assert!(
            profile_labels().contains(&record.terminal_profile),
            "dogfood terminal_profile must be a bounded label, got {:?}",
            record.terminal_profile,
        );
        if !profile_labels().contains(&record.terminal_profile) {
            return Ok(false);
        }
        use std::io::Write as _;
        let mut line = self.to_jsonl();
        line.push('\n');
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(line.as_bytes())?;
        Ok(true)
    }

    /// The `/metrics` overlay block: a compact, fixed-label snapshot of the
    /// live counters. One metric per line, short labels so the box stays narrow
    /// in the top-right corner. Mirrors the render-budget HUD's style.
    pub(crate) fn snapshot_lines(&self) -> Vec<String> {
        let r = self.record();
        vec![
            "dogfood telemetry".to_string(),
            format!("frames  {} (skip {})", r.frames, r.skipped_frames),
            format!(
                "render  ~{} / max {}",
                fmt_micros(r.mean_render_micros),
                fmt_micros(r.max_render_micros)
            ),
            format!("bytes   {}", r.bytes_total),
            format!(
                "cache   {}/{} ({:.0}%)",
                r.cache_hits,
                r.cache_hits + r.cache_misses,
                r.cache_hit_rate_pct
            ),
            format!("wrap    {}", fmt_micros(r.longest_wrap_micros)),
            format!(
                "input   k{} m{} p{} r{}",
                r.key_inputs, r.mouse_inputs, r.paste_inputs, r.resize_inputs
            ),
            format!(
                "storms  scroll {} resize {}",
                r.scroll_storms, r.resize_storms
            ),
            format!("scroll  {} lines", r.scroll_delta_total),
            // Build the copy histogram by iterating the bounded provider set so
            // every variant is shown and a new provider can never be silently
            // dropped from the snapshot.
            format!(
                "copy    {}",
                CopyProvider::ALL
                    .iter()
                    .map(|p| format!("{} {}", p.as_str(), self.copy_count(*p)))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            format!("term    {}", r.terminal_profile),
            format!(
                "a11y    motion {} contrast {}",
                fmt_tri(r.reduced_motion),
                fmt_tri(r.high_contrast)
            ),
            format!("teardown {}", r.emergency_teardowns),
        ]
    }
}

/// The serializable / persisted snapshot. `Serialize` is derived; every field is
/// a number, a bool/`Option<bool>`, or a `&'static str` enum label. There is no
/// `String`/payload field by construction, which the privacy tests enforce.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(crate) struct MetricsRecord {
    pub(crate) schema_version: u32,
    pub(crate) frames: u64,
    pub(crate) skipped_frames: u64,
    pub(crate) mean_render_micros: u64,
    pub(crate) max_render_micros: u64,
    pub(crate) bytes_total: u64,
    pub(crate) cache_hits: u64,
    pub(crate) cache_misses: u64,
    pub(crate) cache_hit_rate_pct: f64,
    pub(crate) longest_wrap_micros: u64,
    pub(crate) key_inputs: u64,
    pub(crate) mouse_inputs: u64,
    pub(crate) paste_inputs: u64,
    pub(crate) resize_inputs: u64,
    pub(crate) resize_storms: u64,
    pub(crate) scroll_delta_total: u64,
    pub(crate) scroll_storms: u64,
    pub(crate) copy_osc52: u64,
    pub(crate) copy_platform: u64,
    pub(crate) copy_tempfile: u64,
    /// Bounded enum label only — see [`TerminalProfile::as_str`].
    pub(crate) terminal_profile: &'static str,
    pub(crate) reduced_motion: Option<bool>,
    pub(crate) high_contrast: Option<bool>,
    pub(crate) emergency_teardowns: u64,
}

/// `Duration` → micros, saturating at `u64::MAX`.
fn duration_micros(d: Duration) -> u64 {
    d.as_micros().min(u128::from(u64::MAX)) as u64
}

/// Round a percentage to two decimals so the JSONL number stays compact and the
/// snapshot is deterministic across platforms.
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Format a micros count compactly: microseconds under 1 ms, else milliseconds
/// with one decimal. Mirrors `metrics::fmt_dur` so the debug surfaces read the
/// same; kept module-local to avoid widening `metrics`' visibility.
fn fmt_micros(us: u64) -> String {
    if us < 1000 {
        format!("{us}µs")
    } else {
        format!("{:.1}ms", us as f64 / 1000.0)
    }
}

/// Format an `Option<bool>` accessibility tri-state for the overlay: `?` until
/// recorded, then `on` / `off`.
fn fmt_tri(v: Option<bool>) -> &'static str {
    match v {
        None => "?",
        Some(true) => "on",
        Some(false) => "off",
    }
}

/// Process-wide emergency-teardown counter. The panic hook / signal handler /
/// `Drop` emergency-teardown paths run *without* a `&mut TuiApp` (they fire on a
/// half-torn-down process), so the count cannot live on the collector struct.
/// It lives here in a lock-free atomic those paths bump and the snapshot folds
/// in via [`emergency_teardowns`]. Stored process-wide for exactly the reason
/// `metrics::LONGEST_WRAP_NANOS` is: the recording site has no app handle.
static EMERGENCY_TEARDOWNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Record one emergency (panic / signal / Drop) teardown. Lock-free and safe to
/// call from a panic hook or signal-adjacent path — it only bumps an atomic.
pub(crate) fn record_emergency_teardown() {
    EMERGENCY_TEARDOWNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Read the process-wide emergency-teardown count.
pub(crate) fn emergency_teardowns() -> u64 {
    EMERGENCY_TEARDOWNS.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
#[path = "dogfood_tests.rs"]
mod tests;
