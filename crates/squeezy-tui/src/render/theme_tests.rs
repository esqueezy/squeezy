use std::collections::BTreeMap;

use squeezy_core::{AppConfig, TUI_THEME_COLOR_TOKENS, TuiThemeSettings};

use super::*;

/// RAII guard that snapshots the process-global `ACTIVE_THEME` on construction and
/// restores it on drop. Mirrors the env-var guards used elsewhere in the suite so a
/// test that calls [`set_active_theme`] can never leak the swap into a concurrent
/// in-process test (e.g. `approval_tests.rs` reading `blue()`/`accent()`).
struct ActiveThemeGuard {
    prior: Theme,
}

impl ActiveThemeGuard {
    fn capture() -> Self {
        Self {
            prior: current_theme(),
        }
    }
}

impl Drop for ActiveThemeGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = active_theme().write() {
            *active = self.prior.clone();
        }
    }
}

#[test]
fn default_theme_resolves_every_token() {
    let cfg = AppConfig::default();
    let theme = resolve_theme(&cfg, "default");

    assert_eq!(theme.name, "default");
    assert_eq!(
        theme.colors().len(),
        TUI_THEME_COLOR_TOKENS.len(),
        "builtin default should define every public color token"
    );
    assert_eq!(theme.resolve(token::PALETTE_ACCENT), Some([216, 185, 112]));
    assert_eq!(theme.resolve(token::DIFF_ADDED), Some([143, 217, 176]));
}

#[test]
fn every_theme_keeps_rail_hues_distinct() {
    // The Quiet Rail tints plan nodes with the accent, subagent context with
    // magenta, and reasoning runs with blue. Those three must stay mutually
    // distinct in every builtin theme or the rail loses its at-a-glance meaning.
    let cfg = AppConfig::default();
    for name in squeezy_core::BUILTIN_TUI_THEME_NAMES {
        let theme = resolve_theme(&cfg, name);
        let accent = theme.resolve(token::PALETTE_ACCENT);
        let magenta = theme.resolve(token::PALETTE_MAGENTA);
        let blue = theme.resolve(token::PALETTE_BLUE);
        assert_ne!(
            accent, magenta,
            "{name}: plan (accent) vs subagent (magenta)"
        );
        assert_ne!(accent, blue, "{name}: plan (accent) vs reasoning (blue)");
        assert_ne!(
            magenta, blue,
            "{name}: subagent (magenta) vs reasoning (blue)"
        );
    }
}

#[test]
fn custom_theme_overlays_default_tokens() {
    let mut cfg = AppConfig::default();
    cfg.tui.themes.insert(
        "solarized".to_string(),
        TuiThemeSettings {
            colors: BTreeMap::from([(token::PALETTE_ACCENT.to_string(), [1, 2, 3])]),
        },
    );

    let theme = resolve_theme(&cfg, "solarized");
    let default = resolve_theme(&cfg, "default");
    assert_eq!(theme.name, "solarized");
    assert_eq!(theme.resolve(token::PALETTE_ACCENT), Some([1, 2, 3]));
    assert_eq!(
        theme.resolve(token::PALETTE_SECONDARY),
        default.resolve(token::PALETTE_SECONDARY)
    );
}

#[test]
fn builtin_theme_overrides_can_be_modified_by_settings() {
    let mut cfg = AppConfig::default();
    cfg.tui.themes.insert(
        "fun".to_string(),
        TuiThemeSettings {
            colors: BTreeMap::from([(token::UI_FOREGROUND.to_string(), [9, 8, 7])]),
        },
    );

    let theme = resolve_theme(&cfg, "fun");
    assert_eq!(theme.resolve(token::UI_FOREGROUND), Some([9, 8, 7]));
    assert_ne!(
        theme.resolve(token::PALETTE_ACCENT),
        resolve_theme(&cfg, "default").resolve(token::PALETTE_ACCENT),
        "fun still keeps its builtin palette for tokens that settings do not override"
    );
}

#[test]
fn setting_active_theme_swaps_snapshot_and_bumps_generation() {
    let mut cfg = AppConfig::default();
    cfg.tui.theme = "bright".to_string();
    let before_name = current_theme_name();
    let before = theme_generation();

    // Swap inside a scope guarded by the RAII restore so the bright theme can never
    // outlive this test and contaminate a concurrent in-process reader.
    {
        let _guard = ActiveThemeGuard::capture();
        set_active_theme(&cfg);

        assert_eq!(current_theme_name(), "bright");
        if before_name != "bright" {
            assert!(theme_generation() > before);
        }
        assert_eq!(
            current_theme().resolve(token::PALETTE_ACCENT),
            Some([255, 214, 85])
        );
    }

    // The guard restored the prior theme on drop — no global mutation leaks out
    // (deep-review #64).
    assert_eq!(current_theme_name(), before_name);
}
