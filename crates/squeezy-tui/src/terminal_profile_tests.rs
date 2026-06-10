//! Unit tests for the pure Per-Terminal Profiles model (§12.7.3).
//!
//! These cover the detection (capabilities), resolution (built-in defaults +
//! env-hint refinement), and the interactive editor cursor/cycle/reset rules in
//! isolation — no terminal, no `TuiApp`. The overlay's behaviour through the real
//! `render()` + key/mouse dispatch is covered by the capture-sink suite in
//! `lib_tests.rs`.

use super::*;
use crate::dogfood::{OsFamily, TerminalProfile as TerminalKind};

/// Build an `env_get` from a fixed list of `(key, value)` pairs.
fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |k: &str| {
        pairs
            .iter()
            .find(|(key, _)| *key == k)
            .map(|(_, v)| v.to_string())
    }
}

#[test]
fn detect_reuses_dogfood_terminal_kind() {
    let caps =
        TerminalCapabilities::detect_from(OsFamily::Macos, env(&[("TERM_PROGRAM", "iTerm.app")]));
    assert_eq!(caps.kind, TerminalKind::MacosIterm2);
}

#[test]
fn detect_colorterm_and_no_color_and_ssh_and_mux() {
    let caps = TerminalCapabilities::detect_from(
        OsFamily::Linux,
        env(&[
            ("TERM_PROGRAM", "vscode"),
            ("COLORTERM", "truecolor"),
            ("SSH_CONNECTION", "1.2.3.4 5 6.7.8.9 22"),
            ("TMUX", "/tmp/tmux-1000/default,1,0"),
        ]),
    );
    assert!(caps.truecolor_env);
    assert!(!caps.no_color);
    assert!(caps.over_ssh);
    assert!(caps.inside_multiplexer);

    let no_color = TerminalCapabilities::detect_from(OsFamily::Linux, env(&[("NO_COLOR", "")]));
    // `$NO_COLOR` disables colour even when empty (no-color.org convention).
    assert!(no_color.no_color);
}

#[test]
fn detect_multiplexer_via_term_screen() {
    let caps =
        TerminalCapabilities::detect_from(OsFamily::Linux, env(&[("TERM", "screen-256color")]));
    assert!(caps.inside_multiplexer);
}

#[test]
fn resolve_iterm2_is_unicode_truecolor_mouse() {
    let caps =
        TerminalCapabilities::detect_from(OsFamily::Macos, env(&[("TERM_PROGRAM", "iTerm.app")]));
    let profile = TerminalProfile::resolve(caps);
    assert_eq!(profile.glyphs, GlyphSet::Unicode);
    assert_eq!(profile.color, ColorDepth::TrueColor);
    assert_eq!(profile.mouse, MouseMode::Enabled);
}

#[test]
fn resolve_vscode_defaults_to_ascii_glyphs() {
    // xterm.js mis-renders wide glyphs, so the VS Code profile defaults to ASCII
    // even though it supports colour + mouse.
    let caps =
        TerminalCapabilities::detect_from(OsFamily::Macos, env(&[("TERM_PROGRAM", "vscode")]));
    let profile = TerminalProfile::resolve(caps);
    assert_eq!(profile.glyphs, GlyphSet::Ascii);
    assert_eq!(caps.kind, TerminalKind::MacosVscode);
}

#[test]
fn resolve_apple_terminal_caps_at_256() {
    let caps = TerminalCapabilities::detect_from(
        OsFamily::Macos,
        env(&[("TERM_PROGRAM", "apple_terminal")]),
    );
    let profile = TerminalProfile::resolve(caps);
    assert_eq!(profile.color, ColorDepth::Indexed256);
}

#[test]
fn resolve_unknown_terminal_is_safe_default() {
    let caps = TerminalCapabilities::detect_from(OsFamily::Other, env(&[]));
    assert_eq!(caps.kind, TerminalKind::Unknown);
    assert_eq!(TerminalProfile::resolve(caps), TerminalProfile::SAFE);
    assert_eq!(TerminalProfile::SAFE.glyphs, GlyphSet::Ascii);
}

#[test]
fn refine_no_color_forces_monochrome() {
    // A normally-truecolor terminal with `$NO_COLOR` set collapses to monochrome.
    let caps = TerminalCapabilities::detect_from(
        OsFamily::Macos,
        env(&[("TERM_PROGRAM", "iTerm.app"), ("NO_COLOR", "1")]),
    );
    let profile = TerminalProfile::resolve(caps);
    assert_eq!(profile.color, ColorDepth::Monochrome);
}

#[test]
fn refine_colorterm_promotes_256_to_truecolor() {
    // Apple Terminal defaults to 256, but `$COLORTERM=truecolor` promotes it.
    let caps = TerminalCapabilities::detect_from(
        OsFamily::Macos,
        env(&[("TERM_PROGRAM", "apple_terminal"), ("COLORTERM", "24bit")]),
    );
    let profile = TerminalProfile::resolve(caps);
    assert_eq!(profile.color, ColorDepth::TrueColor);
}

#[test]
fn refine_ssh_without_truecolor_caps_truecolor_at_256() {
    // A truecolor-default terminal reached over SSH (no truecolor advertisement)
    // is capped at 256 because truecolor escapes are the most likely to be
    // mangled across a relay.
    let caps = TerminalCapabilities::detect_from(
        OsFamily::Macos,
        env(&[("TERM_PROGRAM", "iTerm.app"), ("SSH_TTY", "/dev/pts/3")]),
    );
    let profile = TerminalProfile::resolve(caps);
    assert!(caps.over_ssh);
    assert_eq!(profile.color, ColorDepth::Indexed256);
}

#[test]
fn refine_ssh_keeps_truecolor_when_advertised() {
    let caps = TerminalCapabilities::detect_from(
        OsFamily::Macos,
        env(&[
            ("TERM_PROGRAM", "iTerm.app"),
            ("SSH_TTY", "/dev/pts/3"),
            ("COLORTERM", "truecolor"),
        ]),
    );
    let profile = TerminalProfile::resolve(caps);
    assert_eq!(profile.color, ColorDepth::TrueColor);
}

#[test]
fn color_depth_round_trips_and_cycles() {
    for depth in ColorDepth::ALL {
        assert_eq!(ColorDepth::from_str(depth.as_str()), Some(depth));
    }
    assert_eq!(ColorDepth::from_str("nonsense"), None);
    // Cycle wraps and visits every depth.
    let mut seen = Vec::new();
    let mut d = ColorDepth::TrueColor;
    for _ in 0..ColorDepth::ALL.len() {
        seen.push(d);
        d = d.next();
    }
    assert_eq!(d, ColorDepth::TrueColor, "cycle wraps");
    assert_eq!(seen.len(), ColorDepth::ALL.len());
}

#[test]
fn glyph_and_mouse_round_trip_and_toggle() {
    for g in GlyphSet::ALL {
        assert_eq!(GlyphSet::from_str(g.as_str()), Some(g));
    }
    assert_eq!(GlyphSet::Unicode.next(), GlyphSet::Ascii);
    assert_eq!(GlyphSet::Ascii.next(), GlyphSet::Unicode);
    for m in MouseMode::ALL {
        assert_eq!(MouseMode::from_str(m.as_str()), Some(m));
    }
    assert_eq!(MouseMode::Enabled.next(), MouseMode::Disabled);
    assert_eq!(MouseMode::Disabled.next(), MouseMode::Enabled);
}

#[test]
fn config_pairs_match_fields() {
    let profile = TerminalProfile {
        glyphs: GlyphSet::Ascii,
        mouse: MouseMode::Disabled,
        color: ColorDepth::Ansi16,
    };
    assert_eq!(
        profile.as_config_pairs(),
        [("glyphs", "ascii"), ("mouse", "off"), ("color", "ansi16")]
    );
}

#[test]
fn from_config_lookup_round_trips_as_config_pairs() {
    let saved = TerminalProfile {
        glyphs: GlyphSet::Ascii,
        mouse: MouseMode::Disabled,
        color: ColorDepth::Monochrome,
    };
    let pairs = saved.as_config_pairs();
    let fallback = TerminalProfile::SAFE;
    let read = TerminalProfile::from_config_lookup(fallback, |key| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.to_string())
    });
    assert_eq!(read, Some(saved), "a persisted profile reads back exactly");
}

#[test]
fn from_config_lookup_partial_table_keeps_fallback_for_missing_fields() {
    // A table that only names `glyphs` overrides just that field; the rest keep the
    // fallback (built-in default) value.
    let fallback = TerminalProfile {
        glyphs: GlyphSet::Unicode,
        mouse: MouseMode::Enabled,
        color: ColorDepth::TrueColor,
    };
    let read = TerminalProfile::from_config_lookup(fallback, |key| {
        (key == "glyphs").then(|| "ascii".to_string())
    });
    assert_eq!(
        read,
        Some(TerminalProfile {
            glyphs: GlyphSet::Ascii,
            ..fallback
        })
    );
}

#[test]
fn from_config_lookup_empty_or_unknown_is_none() {
    let fallback = TerminalProfile::SAFE;
    // No recognised field at all -> None (so the caller knows there's no override).
    assert_eq!(
        TerminalProfile::from_config_lookup(fallback, |_| None),
        None
    );
    // An unrecognised value on the only present key is also "no override".
    assert_eq!(
        TerminalProfile::from_config_lookup(fallback, |key| (key == "color")
            .then(|| "nonsense".to_string())),
        None
    );
}

fn iterm_editor() -> TerminalProfileEditor {
    let caps =
        TerminalCapabilities::detect_from(OsFamily::Macos, env(&[("TERM_PROGRAM", "iTerm.app")]));
    TerminalProfileEditor::new(caps, None)
}

#[test]
fn editor_seeds_from_default_when_no_override() {
    let editor = iterm_editor();
    assert_eq!(editor.working(), editor.default_profile());
    assert!(!editor.is_overridden());
    assert_eq!(editor.focused_field(), ProfileField::Glyphs);
    assert_eq!(editor.field_index(), 0);
}

#[test]
fn editor_seeds_from_override_when_pinned() {
    let caps =
        TerminalCapabilities::detect_from(OsFamily::Macos, env(&[("TERM_PROGRAM", "iTerm.app")]));
    let pin = TerminalProfile {
        glyphs: GlyphSet::Ascii,
        mouse: MouseMode::Disabled,
        color: ColorDepth::Ansi16,
    };
    let editor = TerminalProfileEditor::new(caps, Some(pin));
    assert_eq!(editor.working(), pin);
    assert!(
        editor.is_overridden(),
        "pinned profile differs from default"
    );
}

#[test]
fn editor_field_focus_clamps_at_ends() {
    let mut editor = iterm_editor();
    // Already at the top: prev is a no-op.
    assert!(!editor.focus_prev_field());
    assert!(editor.focus_next_field());
    assert_eq!(editor.focused_field(), ProfileField::Mouse);
    assert!(editor.focus_next_field());
    assert_eq!(editor.focused_field(), ProfileField::Color);
    // At the bottom: next is a no-op.
    assert!(!editor.focus_next_field());
    assert_eq!(editor.focused_field(), ProfileField::Color);
}

#[test]
fn editor_focus_field_by_index_is_mouse_twin() {
    let mut editor = iterm_editor();
    assert!(editor.focus_field(2));
    assert_eq!(editor.focused_field(), ProfileField::Color);
    // Re-focusing the same row is a no-op.
    assert!(!editor.focus_field(2));
    // Out of range is ignored.
    assert!(!editor.focus_field(99));
    assert_eq!(editor.focused_field(), ProfileField::Color);
}

#[test]
fn editor_cycle_focused_changes_only_focused_field() {
    let mut editor = iterm_editor();
    let before = editor.working();
    // Focus the mouse row and toggle it.
    editor.focus_field(1);
    let after = editor.cycle_focused();
    assert_ne!(after.mouse, before.mouse, "mouse toggled");
    assert_eq!(after.glyphs, before.glyphs, "glyphs untouched");
    assert_eq!(after.color, before.color, "color untouched");
    assert!(editor.is_overridden());
}

#[test]
fn editor_reset_restores_default() {
    let mut editor = iterm_editor();
    editor.focus_field(0);
    editor.cycle_focused();
    editor.focus_field(2);
    editor.cycle_focused();
    assert!(editor.is_overridden());
    let restored = editor.reset_to_default();
    assert_eq!(restored, editor.default_profile());
    assert!(!editor.is_overridden());
}

#[test]
fn profile_field_value_labels_track_working() {
    let editor = iterm_editor();
    let p = editor.working();
    assert_eq!(ProfileField::Glyphs.value_label(p), p.glyphs.label());
    assert_eq!(ProfileField::Mouse.value_label(p), p.mouse.label());
    assert_eq!(ProfileField::Color.value_label(p), p.color.label());
}
