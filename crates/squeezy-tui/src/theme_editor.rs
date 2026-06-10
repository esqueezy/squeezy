//! Theme Editor UI (§12.7.2).
//!
//! An interactive, live-preview color picker over the active TUI theme's
//! semantic palette roles (accent, error, dim, selection, …). It is a
//! fullscreen overlay the user opens over the transcript: a left rail lists the
//! editable palette ROLES, and a right panel shows the focused role's R/G/B
//! channels plus a live preview swatch. Adjusting a channel re-applies the
//! in-progress override to the *active runtime theme immediately* (so the whole
//! UI behind the overlay previews the change), and committing persists the
//! override to the user-scope config through the existing
//! [`squeezy_core::settings_writer`] theme-color edit path that `/config`'s
//! theme editor already uses.
//!
//! ## Model, not chrome
//!
//! Like its peer leaf modules ([`crate::scratchpad`], [`crate::snippet_store`])
//! this file owns only the *pure* editor model — the curated role list, the
//! focus/channel cursors, and the working RGB triple — so every navigation /
//! channel-adjust / dirty rule is unit-testable without standing up a `TuiApp`
//! or a terminal. `lib.rs` owns the side effects: the keybinding, the open/close
//! flag, the per-frame render call through the single fullscreen `render()`, the
//! live-preview re-apply, and the persist-to-config commit/reset verbs.
//!
//! It owns:
//!
//!   - [`Role`]: one editable palette role — a stable theme color token plus a
//!     friendly label and a one-line description of where the role shows up.
//!   - [`ROLES`]: the curated, ordered role list the editor exposes. A deliberate
//!     subset of [`squeezy_core::TUI_THEME_COLOR_TOKENS`] (the user-facing
//!     semantic roles the spec names), so the editor stays approachable rather
//!     than dumping all ~43 tokens at once.
//!   - [`ThemeEditorState`]: the focused role, the focused channel (R/G/B), and
//!     the working RGB triple, with the navigation + channel-adjust primitives.
//!
//! ## Bounds & idle cost
//!
//! The role list is a compile-time constant; the working state is three `u8`s
//! plus two small cursors. The overlay is closed by default (a single `bool`
//! flag on `TuiApp`) and at rest paints nothing and schedules no redraw, so an
//! idle session pays one enum-tag check and nothing more.

use squeezy_core::TuiRgb;

/// One editable palette role surfaced by the theme editor. `token` is the stable
/// theme color token (one of [`squeezy_core::TUI_THEME_COLOR_TOKENS`]) the role
/// maps to; `label` / `description` are the human-facing strings shown in the
/// role rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Role {
    /// The theme color token this role edits, e.g. `"palette.accent"`.
    pub(crate) token: &'static str,
    /// Short, friendly role name shown in the rail, e.g. `"Accent"`.
    pub(crate) label: &'static str,
    /// One-line note on where the role shows up, shown beside the preview.
    pub(crate) description: &'static str,
}

/// The curated, ordered palette roles the editor exposes. A deliberate subset of
/// [`squeezy_core::TUI_THEME_COLOR_TOKENS`] chosen to match the roles the §12.7.2
/// spec calls out (palette, selection, status warnings/errors, dim, …) so the
/// editor stays approachable. Every `token` here is a real theme color token, so
/// a persist/preview round-trips through the same registry the rest of the TUI
/// reads.
pub(crate) const ROLES: &[Role] = &[
    Role {
        token: "palette.accent",
        label: "Accent",
        description: "Brand accent — titles, focus rings, active rails",
    },
    Role {
        token: "palette.secondary",
        label: "Secondary",
        description: "Secondary accent — chevrons, hints",
    },
    Role {
        token: "status.err",
        label: "Error",
        description: "Errors and failed-tool markers",
    },
    Role {
        token: "status.warn",
        label: "Warning",
        description: "Warnings and caution status",
    },
    Role {
        token: "status.ok",
        label: "Success",
        description: "Success / passing status",
    },
    Role {
        token: "status.info",
        label: "Info / Queue",
        description: "Informational status and queue badges",
    },
    Role {
        token: "ui.surface",
        label: "Selection",
        description: "Selection and hovered-row surface",
    },
    Role {
        token: "ui.quiet",
        label: "Dim",
        description: "Dim / quiet text — fold indicators, pointer hints",
    },
    Role {
        token: "ui.muted",
        label: "Muted",
        description: "Muted secondary text",
    },
    Role {
        token: "ui.foreground",
        label: "Foreground",
        description: "Primary transcript text",
    },
    Role {
        token: "ui.border",
        label: "Border",
        description: "Card and overlay borders",
    },
    Role {
        token: "path.hint",
        label: "Path hint",
        description: "Clickable path / link hint text",
    },
];

/// The three RGB channels, in render/edit order. The focused channel is the one a
/// left/right move steps between and an up/down move (or a click on its bar)
/// adjusts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Channel {
    Red,
    Green,
    Blue,
}

impl Channel {
    /// The three channels in render order — used by the renderer to lay out the
    /// channel bars and by the mouse path to map a clicked bar to a channel.
    pub(crate) const ALL: [Channel; 3] = [Channel::Red, Channel::Green, Channel::Blue];

    /// Single-letter label painted at the head of the channel's bar.
    pub(crate) fn label(self) -> char {
        match self {
            Channel::Red => 'R',
            Channel::Green => 'G',
            Channel::Blue => 'B',
        }
    }

    /// Index of this channel into a `[u8; 3]` RGB triple.
    pub(crate) fn index(self) -> usize {
        match self {
            Channel::Red => 0,
            Channel::Green => 1,
            Channel::Blue => 2,
        }
    }
}

/// The pure interactive theme-editor model. Holds the focused role, the focused
/// channel, and the working RGB triple the user is shaping for that role. All
/// persistence / live-preview side effects live in `lib.rs`; this struct is the
/// terminal-free, fully unit-testable core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemeEditorState {
    /// Cursor into [`ROLES`]. Always in bounds (the constructor and movers clamp
    /// it), so `current_role()` never panics.
    role: usize,
    /// The channel a vertical adjust currently targets.
    channel: Channel,
    /// The working RGB for the focused role — what the live preview shows and what
    /// a commit persists.
    rgb: TuiRgb,
}

impl ThemeEditorState {
    /// Open the editor focused on the first role, seeding the working RGB from
    /// `seed` — the focused role's current resolved color in the active theme — so
    /// the editor opens showing the live value rather than a blank swatch.
    pub(crate) fn new(seed: TuiRgb) -> Self {
        Self {
            role: 0,
            channel: Channel::Red,
            rgb: seed,
        }
    }

    /// The focused role. Always valid: [`ROLES`] is non-empty and `role` is
    /// clamped on every move.
    pub(crate) fn current_role(&self) -> Role {
        ROLES[self.role.min(ROLES.len() - 1)]
    }

    /// Index of the focused role into [`ROLES`].
    pub(crate) fn role_index(&self) -> usize {
        self.role.min(ROLES.len() - 1)
    }

    /// The focused channel.
    pub(crate) fn channel(&self) -> Channel {
        self.channel
    }

    /// The working RGB triple — the live-preview color and what a commit persists.
    pub(crate) fn rgb(&self) -> TuiRgb {
        self.rgb
    }

    /// The current value (0..=255) of the focused channel.
    pub(crate) fn channel_value(&self, channel: Channel) -> u8 {
        self.rgb[channel.index()]
    }

    /// Move the role focus up one row (clamped at the top). Reseeds the working
    /// RGB from `seed` — the newly-focused role's live color — and returns the
    /// newly-focused role so the caller can refresh the preview. Returns `None`
    /// when already at the top (no row change, no reseed).
    pub(crate) fn focus_prev_role(
        &mut self,
        seed: impl Fn(&'static str) -> TuiRgb,
    ) -> Option<Role> {
        if self.role == 0 {
            return None;
        }
        self.role -= 1;
        let role = self.current_role();
        self.rgb = seed(role.token);
        self.channel = Channel::Red;
        Some(role)
    }

    /// Move the role focus down one row (clamped at the bottom). Reseeds the
    /// working RGB from the newly-focused role's live color. Returns `None` when
    /// already at the bottom.
    pub(crate) fn focus_next_role(
        &mut self,
        seed: impl Fn(&'static str) -> TuiRgb,
    ) -> Option<Role> {
        if self.role + 1 >= ROLES.len() {
            return None;
        }
        self.role += 1;
        let role = self.current_role();
        self.rgb = seed(role.token);
        self.channel = Channel::Red;
        Some(role)
    }

    /// Focus a role directly by its [`ROLES`] index (the mouse twin of ↑/↓ over a
    /// role row). Out-of-range indices are ignored. Reseeds the working RGB from
    /// the targeted role's live color and returns it; returns `None` for an
    /// out-of-range index or a no-op re-focus of the already-current role.
    pub(crate) fn focus_role(
        &mut self,
        index: usize,
        seed: impl Fn(&'static str) -> TuiRgb,
    ) -> Option<Role> {
        if index >= ROLES.len() || index == self.role {
            return None;
        }
        self.role = index;
        let role = self.current_role();
        self.rgb = seed(role.token);
        self.channel = Channel::Red;
        Some(role)
    }

    /// Step the channel focus left (R←G←B), clamped at Red. The mouse twin is a
    /// click on a channel bar, which targets that channel directly via
    /// [`Self::set_channel`].
    pub(crate) fn focus_prev_channel(&mut self) {
        self.channel = match self.channel {
            Channel::Red => Channel::Red,
            Channel::Green => Channel::Red,
            Channel::Blue => Channel::Green,
        };
    }

    /// Step the channel focus right (R→G→B), clamped at Blue.
    pub(crate) fn focus_next_channel(&mut self) {
        self.channel = match self.channel {
            Channel::Red => Channel::Green,
            Channel::Green => Channel::Blue,
            Channel::Blue => Channel::Blue,
        };
    }

    /// Nudge the focused channel by `delta` (saturating at 0 and 255). The
    /// keyboard up/down arrows pass ±1; Shift/Page variants pass a larger step.
    /// Returns the new working RGB so the caller can re-apply the live preview.
    pub(crate) fn adjust_channel(&mut self, delta: i16) -> TuiRgb {
        let idx = self.channel.index();
        let next = (self.rgb[idx] as i16 + delta).clamp(0, 255) as u8;
        self.rgb[idx] = next;
        self.rgb
    }

    /// Set the focused channel to an absolute value (the mouse twin of dragging /
    /// clicking a point on the channel bar). Returns the new working RGB.
    pub(crate) fn set_channel(&mut self, channel: Channel, value: u8) -> TuiRgb {
        self.channel = channel;
        self.rgb[channel.index()] = value;
        self.rgb
    }

    /// Replace the whole working RGB (used when a reset restores the role's
    /// builtin/base color so the preview tracks the cleared value).
    pub(crate) fn set_rgb(&mut self, rgb: TuiRgb) {
        self.rgb = rgb;
    }
}

#[cfg(test)]
#[path = "theme_editor_tests.rs"]
mod tests;
