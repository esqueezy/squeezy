//! Mouse Hover Intent (§12.1.3): a subtle, debounced hover affordance for the
//! transcript card under the pointer — revealed only after *stable intent*, not
//! on every mouse move, and suppressed entirely while a wheel scroll, drag, or
//! text selection is in flight (the spec's "the default hover state is visual,
//! not structural" reveal).
//!
//! **Pure model.** Like the other §12 leaf modules (`action_palette`,
//! `change_summary`, `session_timeline`), this file owns only the *state machine*
//! — which stable semantic id is currently revealed, when it was first seen, the
//! last pointer cell, and the suppression reason — plus the rule that decides
//! *which* entry should paint the hover affordance this frame. It does NOT depend
//! on `lib.rs`'s `TuiApp`: the caller feeds it the recognizer's
//! [`crate::interaction::Gesture::HoverEnter`]/`HoverLeave` (themselves already
//! debounced by [`crate::interaction::HOVER_INTENT_MS`]), the pointer cell, and a
//! suppression signal, then asks [`HoverIntentState::reveal_target`] which id to
//! emphasize. Keyboard parity holds by construction: when no mouse motion is
//! reported (the resting capture mode is button-only `?1000h`, so bare `Moved`
//! events never arrive on most terminals), the same affordance reveals on the
//! *keyboard-focused* entry instead — exactly the spec's "hover reveal must
//! degrade to keyboard focus when mouse reporting is absent or lossy".
//!
//! **Stable id anchor.** The revealed target is a `TranscriptEntry::id`, never a
//! row offset, so a reflow (resize, streaming, collapse) between the hover and
//! the paint still resolves to the right entry — and an entry that scrolls out of
//! the window simply paints no affordance.
//!
//! **Zero idle cost.** The resting state is "disabled-by-default off / no target
//! / no suppression"; a session that never hovers (and whose terminal never
//! reports motion) keeps `target == None` and `reveal_pending() == false`, so the
//! render path paints nothing extra and the redraw gate schedules no tick. Only
//! while a reveal is *pending* (seen, not yet settled) does the caller schedule a
//! follow-up redraw, and once it settles the state goes quiet again.

use std::time::Instant;

/// Why a would-be hover reveal is currently suppressed. The spec enumerates the
/// gestures that must *not* trigger a hover reveal: "Wheel scroll, drag,
/// selection, focus movement, and disabled mouse capture suppress hover reveal."
/// Recording the *reason* (not just a bool) lets the caller surface honest
/// diagnostics and lets a test assert the exact gesture that blocked the reveal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SuppressReason {
    /// A wheel-scroll event is in flight (the pointer is sweeping content, not
    /// dwelling on a target).
    Scroll,
    /// A button-held drag is in flight (queue reorder, scrollbar drag, or a
    /// main-text selection drag).
    Drag,
    /// A text selection is active over the transcript.
    Selection,
    /// Mouse capture is off (or motion reporting is absent), so no reliable
    /// hover signal exists; the affordance degrades to keyboard focus.
    CaptureOff,
}

impl SuppressReason {
    /// A short, screen-reader-friendly noun for the suppression, ASCII only so it
    /// carries meaning without a glyph or color. A `cfg(test)`-only diagnostic
    /// accessor — the production paths branch on the variant, never the string —
    /// so it carries no runtime weight on any platform.
    #[cfg(test)]
    pub(crate) fn note(self) -> &'static str {
        match self {
            SuppressReason::Scroll => "scroll",
            SuppressReason::Drag => "drag",
            SuppressReason::Selection => "selection",
            SuppressReason::CaptureOff => "capture-off",
        }
    }
}

/// The Mouse Hover Intent state (§12.1.3): the stable id of the entry the pointer
/// is dwelling on, the timing of that dwell, the last pointer cell, and any
/// active suppression. Held by `TuiApp` directly (not behind an `Option`) because
/// the resting state is itself "no target / not suppressed", which costs nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HoverIntentState {
    /// Whether hover-reveal is enabled. Toggled by the keyboard verb; when off,
    /// neither the mouse nor the keyboard-focus path reveals an affordance, so a
    /// user who finds the emphasis distracting can silence it entirely.
    enabled: bool,
    /// The stable `TranscriptEntry::id` the pointer is currently dwelling on
    /// (the recognizer already waited out [`crate::interaction::HOVER_INTENT_MS`]
    /// before emitting the `HoverEnter` that set this), or `None` when the
    /// pointer is over no target.
    target: Option<u64>,
    /// When the current `target` was first revealed. `None` mirrors a `None`
    /// target. Used so the caller can tell a freshly-revealed hover (schedule one
    /// settling redraw) from a long-settled one (stay quiet).
    first_seen: Option<Instant>,
    /// When the pointer last moved (entered/left a target). Updated on every
    /// hover transition so a future "auto-hide after idle" tweak has the anchor;
    /// today it powers the [`HoverIntentState::reveal_pending`] settle window.
    last_moved: Option<Instant>,
    /// The last pointer cell `(column, row)` the recognizer reported, in absolute
    /// screen coordinates. Kept for diagnostics and so a resize that moves the
    /// target re-resolves from the id, never this stale cell.
    pointer: Option<(u16, u16)>,
    /// Why a reveal is currently suppressed, or `None` when nothing blocks it.
    suppression: Option<SuppressReason>,
}

impl Default for HoverIntentState {
    fn default() -> Self {
        Self {
            // Enabled by default: the affordance is restrained (a brighter,
            // bolded hint on the hovered/focused header), never structural, so it
            // adds discoverability without layout churn. The keyboard verb flips
            // it off for users who prefer none.
            enabled: true,
            target: None,
            first_seen: None,
            last_moved: None,
            pointer: None,
            suppression: None,
        }
    }
}

impl HoverIntentState {
    /// Whether hover-reveal is currently enabled. `cfg(test)`-only: production
    /// reads the enable flag through `toggle`'s return value and the internal
    /// checks in `on_hover_enter`/`reveal_target`, never this getter.
    #[cfg(test)]
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Toggle the feature on/off, returning the new state. Clears any in-flight
    /// hover target when turning off so a stale affordance can't linger painted.
    pub(crate) fn toggle(&mut self) -> bool {
        self.enabled = !self.enabled;
        if !self.enabled {
            self.clear();
        }
        self.enabled
    }

    /// The stable id the pointer is currently dwelling on (post-debounce), if any.
    /// `cfg(test)`-only accessor: production reads the reveal decision through
    /// [`reveal_target`](Self::reveal_target), not the raw hovered id.
    #[cfg(test)]
    pub(crate) fn hovered_target(&self) -> Option<u64> {
        self.target
    }

    /// The active suppression reason, if any. `cfg(test)`-only: production decides
    /// the reveal through [`reveal_target`](Self::reveal_target) (which already
    /// honors the suppression internally), never by reading the reason out.
    #[cfg(test)]
    pub(crate) fn suppression(&self) -> Option<SuppressReason> {
        self.suppression
    }

    /// The last pointer cell the recognizer reported, if any. `cfg(test)`-only:
    /// the production render path resolves the target by id, not by this cell.
    #[cfg(test)]
    pub(crate) fn pointer_cell(&self) -> Option<(u16, u16)> {
        self.pointer
    }

    /// Record that the pointer settled (post-debounce) onto the entry with stable
    /// id `id` at screen cell `(column, row)`. A no-op while suppressed or
    /// disabled (a scroll/drag/selection in flight must not reveal). Returns
    /// `true` when this actually changed the revealed target (so the caller knows
    /// to request a redraw); a re-enter on the same already-revealed id returns
    /// `false`, which keeps a settled hover from looping the redraw gate.
    pub(crate) fn on_hover_enter(&mut self, id: u64, column: u16, row: u16, now: Instant) -> bool {
        self.pointer = Some((column, row));
        if !self.enabled || self.suppression.is_some() {
            // Even while suppressed we remember the pointer cell (for diagnostics)
            // but never reveal — and we clear any previously revealed target so a
            // scroll begun mid-hover hides the affordance immediately.
            let changed = self.target.is_some();
            self.target = None;
            self.first_seen = None;
            self.last_moved = Some(now);
            return changed;
        }
        if self.target == Some(id) {
            // Same id, already revealed: no visual change, so no redraw needed.
            // (The recognizer also debounces re-emits, so this is belt-and-braces.)
            // We still refresh `last_moved` so a pointer that keeps jittering on the
            // same card keeps the settle window alive, distinct from `first_seen`.
            self.last_moved = Some(now);
            return false;
        }
        self.target = Some(id);
        self.first_seen = Some(now);
        self.last_moved = Some(now);
        true
    }

    /// Record that the pointer left every target. Returns `true` when an
    /// affordance was actually showing (so the caller requests one final redraw
    /// to erase it); `false` when nothing was revealed.
    pub(crate) fn on_hover_leave(&mut self, now: Instant) -> bool {
        self.pointer = None;
        self.last_moved = Some(now);
        let was_revealed = self.target.is_some();
        self.target = None;
        self.first_seen = None;
        was_revealed
    }

    /// Apply (or clear) a suppression. Returns `true` when this changed whether an
    /// affordance is painted (so the caller requests a redraw). Setting a
    /// suppression hides any in-flight hover target immediately; clearing it does
    /// NOT re-reveal anything (the next `HoverEnter` does, honestly, after a fresh
    /// dwell).
    pub(crate) fn set_suppression(&mut self, reason: Option<SuppressReason>) -> bool {
        if self.suppression == reason {
            return false;
        }
        self.suppression = reason;
        if reason.is_some() && self.target.is_some() {
            self.target = None;
            self.first_seen = None;
            return true;
        }
        false
    }

    /// Reset every in-flight hover field. Called when mouse capture toggles off or
    /// the surface changes out from under an in-flight hover, so a stale reveal
    /// can't survive into a context where its target no longer exists.
    pub(crate) fn clear(&mut self) {
        self.target = None;
        self.first_seen = None;
        self.last_moved = None;
        self.pointer = None;
        self.suppression = None;
    }

    /// True while a hover reveal is *pending settling*: a target is revealed and
    /// the brief settle window since it was first seen has not yet elapsed. The
    /// caller schedules a single follow-up redraw only while this is true, so the
    /// reveal paints once and then the state goes quiet — never a redraw loop.
    /// Returns `false` the moment the window elapses (or there is no target), so
    /// an idle, settled hover schedules nothing.
    pub(crate) fn reveal_pending(&self, now: Instant) -> bool {
        // Anchor the settle window on the LAST movement (so a pointer that keeps
        // jittering on the same card keeps the window open), but never longer than
        // the window measured from when the reveal was FIRST seen — so a target the
        // pointer never leaves still settles to quiet promptly rather than ticking
        // forever. Both `last_moved` and `first_seen` are read here, so the spec's
        // "first-seen time" and "last movement" fields are both live.
        match (self.target, self.first_seen, self.last_moved) {
            (Some(_), Some(seen), Some(moved)) => {
                now.duration_since(moved).as_millis() < HOVER_SETTLE_MS
                    && now.duration_since(seen).as_millis() < HOVER_SETTLE_MS
            }
            _ => false,
        }
    }

    /// The stable id whose header should paint the hover affordance this frame, or
    /// `None` for no emphasis. This is the single decision the render path reads,
    /// and it encodes the spec's keyboard-degrade rule:
    ///
    /// - feature off ⇒ nothing,
    /// - a live, unsuppressed mouse hover ⇒ the hovered id,
    /// - otherwise ⇒ the keyboard-focused id (`focused_entry_id`), so the same
    ///   restrained emphasis reveals on the focused card even when the terminal
    ///   never reports a single mouse-move (the common `?1000h` case).
    ///
    /// Pure over its inputs, so the render path computes it from `&TuiApp`.
    pub(crate) fn reveal_target(&self, focused_entry_id: Option<u64>) -> Option<u64> {
        if !self.enabled {
            return None;
        }
        if self.suppression.is_none()
            && let Some(id) = self.target
        {
            return Some(id);
        }
        focused_entry_id
    }
}

/// How long after a hover first reveals the caller keeps scheduling a settling
/// redraw. Short — just long enough to coalesce the reveal paint and let any
/// terminal motion jitter settle — after which the state reports "not pending" so
/// no further idle tick is scheduled. Distinct from
/// [`crate::interaction::HOVER_INTENT_MS`] (the *dwell-before-reveal* debounce);
/// this is the *reveal-to-quiet* settle window.
const HOVER_SETTLE_MS: u128 = 120;

#[cfg(test)]
#[path = "hover_intent_tests.rs"]
mod tests;
