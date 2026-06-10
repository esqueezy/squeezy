//! Per-Terminal Profiles (§12.7.3).
//!
//! Different terminals have different quirks: xterm.js (the VS Code / browser
//! terminal) renders some wide glyphs at the wrong cell width and is conservative
//! about truecolor, Apple Terminal tops out at 256 colours, a bare Linux console
//! has no Unicode box-drawing, and a terminal reached over SSH or behind a policy
//! may have mouse reporting disabled. The §12.7.3 spec's answer is to *adapt UX
//! policy to the terminal without forking the renderer*: split the **detected**
//! [`TerminalCapabilities`] (what the terminal can do, sniffed from the
//! environment) from a **resolved** [`TerminalProfile`] (the UX policy we apply —
//! glyph set, mouse mode, colour depth), with built-in per-terminal defaults that
//! the user can override and that persist.
//!
//! ## Model, not chrome
//!
//! Like its peer leaf modules ([`crate::theme_editor`], [`crate::keybinding_editor`])
//! this file owns only the *pure* model — the detected capabilities, the resolved
//! policy, the built-in default table, and the interactive editor cursor — so
//! every detection / resolution / navigation / toggle rule is unit-testable
//! without standing up a `TuiApp` or a terminal. `lib.rs` owns the side effects:
//! the keybinding, the open/close flag, the per-frame render call through the
//! single fullscreen `render()`, and the persist-to-config commit.
//!
//! It reuses the §12.10.3 capability-probe machinery wholesale: the *detected
//! terminal kind* comes from [`crate::dogfood::TerminalProfile::detect_from`] (the
//! same bounded, env-injected, payload-free classifier the dogfood counters use),
//! so there is exactly one place that maps `$TERM` / `$TERM_PROGRAM` / tmux / OS to
//! a terminal identity. This module layers a *policy* on top of that identity.
//!
//! ## Bounds & idle cost
//!
//! The built-in default table is a compile-time match; the resolved profile is
//! three small enums plus a flag; the editor cursor is one `usize`. The overlay is
//! closed by default (a single `Option` on `TuiApp`) and at rest paints nothing
//! and schedules no redraw, so an idle session pays one enum-tag check and nothing
//! more. Detection runs once at open from injected env lookups — there is no
//! background timer and no per-frame probe.

#![cfg_attr(not(unix), allow(dead_code))]

use crate::dogfood::{OsFamily, TerminalProfile as TerminalKind};

/// How many distinct colours the terminal can render. The resolved profile clamps
/// the renderer's colour choices to this depth so a 256-colour terminal never gets
/// a truecolor escape it would mangle, and a monochrome console gets none. Listed
/// widest-first for the editor's cycle; "wider than" comparisons go through
/// [`ColorDepth::rank`] (higher = more capable) rather than the variant order so the
/// intent is explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ColorDepth {
    /// 24-bit direct colour (`Color::Rgb`). iTerm2, WezTerm, Ghostty, Kitty,
    /// Windows Terminal, modern xterm.
    TrueColor,
    /// 256-colour indexed palette. Apple Terminal, older xterm-256color.
    Indexed256,
    /// The 16 ANSI colours only. Legacy conhost, bare Linux consoles.
    Ansi16,
    /// No colour at all (a `NO_COLOR` environment, a dumb terminal, a pipe).
    Monochrome,
}

impl ColorDepth {
    /// Every depth, widest-first — the editor's cycle order and the exhaustive set
    /// the tests sweep.
    pub(crate) const ALL: [ColorDepth; 4] = [
        ColorDepth::TrueColor,
        ColorDepth::Indexed256,
        ColorDepth::Ansi16,
        ColorDepth::Monochrome,
    ];

    /// The fixed, hand-audited label shown in the editor / persisted to config.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ColorDepth::TrueColor => "truecolor",
            ColorDepth::Indexed256 => "256",
            ColorDepth::Ansi16 => "ansi16",
            ColorDepth::Monochrome => "mono",
        }
    }

    /// Friendly one-word label for the editor row.
    pub(crate) fn label(self) -> &'static str {
        match self {
            ColorDepth::TrueColor => "Truecolor",
            ColorDepth::Indexed256 => "256-color",
            ColorDepth::Ansi16 => "16-color",
            ColorDepth::Monochrome => "Monochrome",
        }
    }

    /// Parse a persisted / config label back to a depth. Unknown labels return
    /// `None` so a stale config silently falls back to the detected default rather
    /// than guessing.
    pub(crate) fn from_str(s: &str) -> Option<ColorDepth> {
        ColorDepth::ALL.iter().copied().find(|d| d.as_str() == s)
    }

    /// Capability rank — higher is more capable. Used for "wider than" comparisons
    /// in [`TerminalProfile::refine`] so the intent does not depend on the variant
    /// declaration order.
    pub(crate) fn rank(self) -> u8 {
        match self {
            ColorDepth::Monochrome => 0,
            ColorDepth::Ansi16 => 1,
            ColorDepth::Indexed256 => 2,
            ColorDepth::TrueColor => 3,
        }
    }

    /// The next depth in the cycle (wraps), used by the editor's left/right and a
    /// click on the row.
    pub(crate) fn next(self) -> ColorDepth {
        let idx = ColorDepth::ALL.iter().position(|d| *d == self).unwrap_or(0);
        ColorDepth::ALL[(idx + 1) % ColorDepth::ALL.len()]
    }
}

/// Which glyph repertoire the renderer should draw with. Terminals that lie about
/// wide-glyph cell widths (xterm.js) or lack Unicode box-drawing (Linux console)
/// get the ASCII fallback so the layout never tears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum GlyphSet {
    /// Full Unicode: rounded borders, block swatches, arrows, box-drawing.
    Unicode,
    /// ASCII-safe fallback: `+-|` borders, `>` markers, no wide glyphs. The
    /// "Minimal Glyph Mode" policy a quirky terminal opts into.
    Ascii,
}

impl GlyphSet {
    pub(crate) const ALL: [GlyphSet; 2] = [GlyphSet::Unicode, GlyphSet::Ascii];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            GlyphSet::Unicode => "unicode",
            GlyphSet::Ascii => "ascii",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            GlyphSet::Unicode => "Unicode",
            GlyphSet::Ascii => "ASCII",
        }
    }

    pub(crate) fn from_str(s: &str) -> Option<GlyphSet> {
        GlyphSet::ALL.iter().copied().find(|g| g.as_str() == s)
    }

    /// Toggle (only two states), used by the editor's left/right and a click.
    pub(crate) fn next(self) -> GlyphSet {
        match self {
            GlyphSet::Unicode => GlyphSet::Ascii,
            GlyphSet::Ascii => GlyphSet::Unicode,
        }
    }
}

/// Whether mouse reporting / capture should be enabled for this terminal. Off for
/// terminals where mouse events are unreliable or where a policy disables them;
/// the keyboard always works regardless, so this is purely a default for the
/// pointer affordances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum MouseMode {
    /// Enable mouse capture (clicks, hover, drag, wheel).
    Enabled,
    /// Disable mouse capture; rely on the keyboard equivalents.
    Disabled,
}

impl MouseMode {
    pub(crate) const ALL: [MouseMode; 2] = [MouseMode::Enabled, MouseMode::Disabled];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            MouseMode::Enabled => "on",
            MouseMode::Disabled => "off",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            MouseMode::Enabled => "Enabled",
            MouseMode::Disabled => "Disabled",
        }
    }

    pub(crate) fn from_str(s: &str) -> Option<MouseMode> {
        MouseMode::ALL.iter().copied().find(|m| m.as_str() == s)
    }

    pub(crate) fn next(self) -> MouseMode {
        match self {
            MouseMode::Enabled => MouseMode::Disabled,
            MouseMode::Disabled => MouseMode::Enabled,
        }
    }
}

/// What the terminal can *do*, sniffed from the environment. The spec is explicit
/// that this **detected** state must be kept separate from the **resolved**
/// [`TerminalProfile`] (the UX policy) so detection stays probabilistic and the
/// policy stays overridable. These are the raw capability hints; the built-in
/// default table turns them into a policy in [`TerminalProfile::resolve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalCapabilities {
    /// The bounded terminal identity, reused verbatim from the §12.10.3 dogfood
    /// classifier so there is one mapping from `$TERM`/`$TERM_PROGRAM`/tmux/OS to
    /// a terminal kind.
    pub(crate) kind: TerminalKind,
    /// Whether the environment advertises truecolor (`$COLORTERM` is `truecolor`
    /// or `24bit`).
    pub(crate) truecolor_env: bool,
    /// Whether `$NO_COLOR` is set (the de-facto opt-out; any value disables
    /// colour).
    pub(crate) no_color: bool,
    /// Whether the session is reached over SSH (`$SSH_CONNECTION` / `$SSH_TTY`).
    /// Mouse + truecolor are more likely to be unreliable across an SSH hop.
    pub(crate) over_ssh: bool,
    /// Whether the session is running inside tmux/screen (`$TMUX` or a `screen`/
    /// `tmux` `$TERM`). Affects mouse pass-through reliability.
    pub(crate) inside_multiplexer: bool,
}

impl TerminalCapabilities {
    /// Detect the terminal's capabilities from an injected environment lookup and
    /// the host OS family. `env_get` is injected (not `std::env`) so detection is
    /// testable without mutating process env. Reuses
    /// [`crate::dogfood::TerminalProfile::detect_from`] for the terminal-kind leg,
    /// then layers the colour / SSH / multiplexer hints on top.
    pub(crate) fn detect_from<F>(os_family: OsFamily, env_get: F) -> TerminalCapabilities
    where
        F: Fn(&str) -> Option<String>,
    {
        let kind = TerminalKind::detect_from(os_family, &env_get);
        let colorterm = env_get("COLORTERM").map(|v| v.to_ascii_lowercase());
        let truecolor_env = matches!(colorterm.as_deref(), Some("truecolor") | Some("24bit"));
        // `$NO_COLOR`: presence (even empty) disables colour per the no-color.org
        // convention.
        let no_color = env_get("NO_COLOR").is_some();
        let over_ssh = env_get("SSH_CONNECTION").is_some() || env_get("SSH_TTY").is_some();
        let term = env_get("TERM").map(|t| t.to_ascii_lowercase());
        let inside_multiplexer = env_get("TMUX").is_some()
            || term
                .as_deref()
                .map(|t| t.contains("tmux") || t.contains("screen"))
                .unwrap_or(false);
        TerminalCapabilities {
            kind,
            truecolor_env,
            no_color,
            over_ssh,
            inside_multiplexer,
        }
    }

    /// Convenience: detect from the real process environment and host OS. The one
    /// call site that reads `std::env`; everything else flows through the injected
    /// form so it stays testable.
    pub(crate) fn detect() -> TerminalCapabilities {
        Self::detect_from(OsFamily::current(), |k| std::env::var(k).ok())
    }
}

/// The **resolved** per-terminal UX policy — glyph set, mouse mode, colour depth.
/// Built from [`TerminalCapabilities`] via a built-in default table the user can
/// override; the override persists. This is what the renderer / input loop would
/// consult to decide whether to draw Unicode borders, enable mouse capture, or
/// emit truecolor escapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalProfile {
    pub(crate) glyphs: GlyphSet,
    pub(crate) mouse: MouseMode,
    pub(crate) color: ColorDepth,
}

impl TerminalProfile {
    /// The conservative fallback policy — ASCII glyphs, mouse on, 16-colour. Used
    /// for an unknown terminal where guessing wrong is costly (a torn layout is
    /// worse than a plain one).
    pub(crate) const SAFE: TerminalProfile = TerminalProfile {
        glyphs: GlyphSet::Ascii,
        mouse: MouseMode::Enabled,
        color: ColorDepth::Ansi16,
    };

    /// Resolve the built-in default policy for the detected capabilities. This is
    /// the per-terminal default table the spec calls for: each known terminal gets
    /// glyph / mouse / colour defaults chosen to match its real quirks, then the
    /// environment hints (`$NO_COLOR`, `$COLORTERM`, SSH) refine the colour depth.
    /// Pure and total: every [`TerminalKind`] has an arm.
    pub(crate) fn resolve(caps: TerminalCapabilities) -> TerminalProfile {
        let base = match caps.kind {
            // Modern macOS emulators: full Unicode + truecolor + mouse.
            TerminalKind::MacosIterm2
            | TerminalKind::MacosWezterm
            | TerminalKind::MacosGhostty
            | TerminalKind::MacosKitty => TerminalProfile {
                glyphs: GlyphSet::Unicode,
                mouse: MouseMode::Enabled,
                color: ColorDepth::TrueColor,
            },
            // Apple Terminal tops out at 256 colours and has no truecolor.
            TerminalKind::MacosAppleTerminal => TerminalProfile {
                glyphs: GlyphSet::Unicode,
                mouse: MouseMode::Enabled,
                color: ColorDepth::Indexed256,
            },
            // VS Code / xterm.js mis-renders some wide glyphs (the very bug that
            // motivated the append-only renderer), so default to ASCII glyphs even
            // though it supports colour and mouse.
            TerminalKind::MacosVscode | TerminalKind::LinuxVscode => TerminalProfile {
                glyphs: GlyphSet::Ascii,
                mouse: MouseMode::Enabled,
                color: ColorDepth::TrueColor,
            },
            // tmux passes mouse + colour through but is conservative about
            // truecolor unless the user opted in; default to 256 and let the env
            // hint promote it.
            TerminalKind::LinuxTmux => TerminalProfile {
                glyphs: GlyphSet::Unicode,
                mouse: MouseMode::Enabled,
                color: ColorDepth::Indexed256,
            },
            // A generic Linux xterm: Unicode + mouse, 256 colours by default.
            TerminalKind::LinuxXterm => TerminalProfile {
                glyphs: GlyphSet::Unicode,
                mouse: MouseMode::Enabled,
                color: ColorDepth::Indexed256,
            },
            // Windows Terminal is fully capable.
            TerminalKind::WindowsTerminal => TerminalProfile {
                glyphs: GlyphSet::Unicode,
                mouse: MouseMode::Enabled,
                color: ColorDepth::TrueColor,
            },
            // Legacy conhost: ASCII glyphs, 16 colours.
            TerminalKind::WindowsConhost => TerminalProfile {
                glyphs: GlyphSet::Ascii,
                mouse: MouseMode::Enabled,
                color: ColorDepth::Ansi16,
            },
            // Unknown: the conservative safe default.
            TerminalKind::Unknown => TerminalProfile::SAFE,
        };
        base.refine(caps)
    }

    /// Apply the environment colour hints on top of the per-terminal base policy.
    /// `$NO_COLOR` forces monochrome (it is an explicit user opt-out and trumps the
    /// table); `$COLORTERM=truecolor` promotes a 256-colour default up to
    /// truecolor; an SSH hop without a truecolor advertisement caps colour at 256
    /// (truecolor escapes are the most likely to be mangled across a relay).
    fn refine(self, caps: TerminalCapabilities) -> TerminalProfile {
        let mut color = self.color;
        if caps.truecolor_env && color.rank() < ColorDepth::TrueColor.rank() {
            color = ColorDepth::TrueColor;
        }
        if caps.over_ssh && !caps.truecolor_env && color == ColorDepth::TrueColor {
            color = ColorDepth::Indexed256;
        }
        if caps.no_color {
            color = ColorDepth::Monochrome;
        }
        TerminalProfile { color, ..self }
    }

    /// The fixed three-field config representation, in editor order. Single-sourced
    /// so the persist path and the editor render can never name the same field
    /// differently.
    pub(crate) fn as_config_pairs(self) -> [(&'static str, &'static str); 3] {
        [
            ("glyphs", self.glyphs.as_str()),
            ("mouse", self.mouse.as_str()),
            ("color", self.color.as_str()),
        ]
    }

    /// Rebuild a profile from a persisted config lookup, layered onto `fallback`
    /// (the built-in default) so a partial / stale `[tui.terminal_profiles.<kind>]`
    /// table only overrides the fields it actually names and an unrecognised value
    /// silently keeps the default. `get` reads a field string by its config key
    /// (`"glyphs"` / `"mouse"` / `"color"`); the symmetric inverse of
    /// [`Self::as_config_pairs`]. Returns `None` when the table named no recognised
    /// field at all (so the caller can treat "no override" distinctly from "default
    /// override").
    pub(crate) fn from_config_lookup<F>(
        fallback: TerminalProfile,
        get: F,
    ) -> Option<TerminalProfile>
    where
        F: Fn(&str) -> Option<String>,
    {
        let glyphs = get("glyphs").and_then(|s| GlyphSet::from_str(&s));
        let mouse = get("mouse").and_then(|s| MouseMode::from_str(&s));
        let color = get("color").and_then(|s| ColorDepth::from_str(&s));
        if glyphs.is_none() && mouse.is_none() && color.is_none() {
            return None;
        }
        Some(TerminalProfile {
            glyphs: glyphs.unwrap_or(fallback.glyphs),
            mouse: mouse.unwrap_or(fallback.mouse),
            color: color.unwrap_or(fallback.color),
        })
    }
}

/// The editable fields of a [`TerminalProfile`], in render/edit order. The editor
/// cursor steps between these; left/right (or a click on the row) cycles the
/// focused field's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProfileField {
    Glyphs,
    Mouse,
    Color,
}

impl ProfileField {
    /// Every field, in render order — the editor's row order and the exhaustive
    /// set the tests sweep.
    pub(crate) const ALL: [ProfileField; 3] = [
        ProfileField::Glyphs,
        ProfileField::Mouse,
        ProfileField::Color,
    ];

    /// The row label shown in the editor.
    pub(crate) fn label(self) -> &'static str {
        match self {
            ProfileField::Glyphs => "Glyph set",
            ProfileField::Mouse => "Mouse mode",
            ProfileField::Color => "Color depth",
        }
    }

    /// A one-line note on what the field controls, shown beside the value.
    pub(crate) fn description(self) -> &'static str {
        match self {
            ProfileField::Glyphs => "Unicode borders/swatches vs ASCII-safe fallback",
            ProfileField::Mouse => "Pointer affordances (keyboard always works)",
            ProfileField::Color => "Clamp render colors to what the terminal renders",
        }
    }

    /// The current value of this field on `profile`, as its display label.
    pub(crate) fn value_label(self, profile: TerminalProfile) -> &'static str {
        match self {
            ProfileField::Glyphs => profile.glyphs.label(),
            ProfileField::Mouse => profile.mouse.label(),
            ProfileField::Color => profile.color.label(),
        }
    }

    /// Cycle this field's value forward on `profile`, returning the updated
    /// profile. Used by the editor's left/right and a click on the row.
    pub(crate) fn cycle(self, profile: TerminalProfile) -> TerminalProfile {
        match self {
            ProfileField::Glyphs => TerminalProfile {
                glyphs: profile.glyphs.next(),
                ..profile
            },
            ProfileField::Mouse => TerminalProfile {
                mouse: profile.mouse.next(),
                ..profile
            },
            ProfileField::Color => TerminalProfile {
                color: profile.color.next(),
                ..profile
            },
        }
    }
}

/// The pure interactive per-terminal-profile editor model (§12.7.3). Holds the
/// detected capabilities (shown read-only as context), the built-in default
/// (so a reset can restore it), and the working profile the user is shaping. All
/// persistence side effects live in `lib.rs`; this struct is the terminal-free,
/// fully unit-testable core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalProfileEditor {
    /// The detected terminal capabilities — read-only context shown in the header.
    caps: TerminalCapabilities,
    /// The built-in default policy for `caps` — what a reset restores.
    default: TerminalProfile,
    /// The working profile the user is editing — what a commit persists and what
    /// the live preview would apply.
    working: TerminalProfile,
    /// Cursor into [`ProfileField::ALL`]. Always in bounds (the constructor and
    /// movers clamp it), so [`Self::focused_field`] never panics.
    field: usize,
}

impl TerminalProfileEditor {
    /// Open the editor for `caps`, seeding the working profile from any persisted
    /// `override_profile` (a manual pin from a previous session) or, when none, the
    /// built-in default for the detected terminal.
    pub(crate) fn new(
        caps: TerminalCapabilities,
        override_profile: Option<TerminalProfile>,
    ) -> Self {
        let default = TerminalProfile::resolve(caps);
        Self {
            caps,
            default,
            working: override_profile.unwrap_or(default),
            field: 0,
        }
    }

    /// The detected capabilities (read-only context).
    pub(crate) fn capabilities(&self) -> TerminalCapabilities {
        self.caps
    }

    /// The built-in default policy for the detected terminal.
    pub(crate) fn default_profile(&self) -> TerminalProfile {
        self.default
    }

    /// The working profile — the live value, what a commit persists.
    pub(crate) fn working(&self) -> TerminalProfile {
        self.working
    }

    /// The focused field. Always valid: [`ProfileField::ALL`] is non-empty and
    /// `field` is clamped on every move.
    pub(crate) fn focused_field(&self) -> ProfileField {
        ProfileField::ALL[self.field.min(ProfileField::ALL.len() - 1)]
    }

    /// Index of the focused field into [`ProfileField::ALL`].
    pub(crate) fn field_index(&self) -> usize {
        self.field.min(ProfileField::ALL.len() - 1)
    }

    /// Whether the working profile differs from the built-in default (a manual
    /// override is in effect). Drives the "overridden" marker and whether a reset
    /// is meaningful.
    pub(crate) fn is_overridden(&self) -> bool {
        self.working != self.default
    }

    /// Move the field focus up one row (clamped at the top). Returns `true` when
    /// the focus moved.
    pub(crate) fn focus_prev_field(&mut self) -> bool {
        if self.field == 0 {
            return false;
        }
        self.field -= 1;
        true
    }

    /// Move the field focus down one row (clamped at the bottom). Returns `true`
    /// when the focus moved.
    pub(crate) fn focus_next_field(&mut self) -> bool {
        if self.field + 1 >= ProfileField::ALL.len() {
            return false;
        }
        self.field += 1;
        true
    }

    /// Focus a field directly by its [`ProfileField::ALL`] index (the mouse twin of
    /// ↑/↓ over a field row). Out-of-range indices are ignored. Returns `true` when
    /// the focus actually moved.
    pub(crate) fn focus_field(&mut self, index: usize) -> bool {
        if index >= ProfileField::ALL.len() || index == self.field {
            return false;
        }
        self.field = index;
        true
    }

    /// Cycle the focused field's value forward (the keyboard ←/→/Space and a click
    /// on the row). Returns the updated working profile so the caller can apply a
    /// live preview.
    pub(crate) fn cycle_focused(&mut self) -> TerminalProfile {
        self.working = self.focused_field().cycle(self.working);
        self.working
    }

    /// Reset the working profile to the built-in default for the detected terminal
    /// (the keyboard `r`/Delete and the "Reset" button). Returns the restored
    /// profile.
    pub(crate) fn reset_to_default(&mut self) -> TerminalProfile {
        self.working = self.default;
        self.working
    }
}

#[cfg(test)]
#[path = "terminal_profile_tests.rs"]
mod tests;
