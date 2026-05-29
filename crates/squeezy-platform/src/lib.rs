//! Platform-specific input shims for recovering modifier information
//! the terminal emulator dropped before crossterm saw it.
//!
//! # Background
//!
//! Many macOS terminal emulators — Apple Terminal, iTerm2 with its
//! default profile, Ghostty with its default profile, Terminal.app —
//! deliver `Shift+Enter` as a bare `\r` byte. They do not negotiate
//! the Kitty keyboard protocol and do not opt into xterm
//! modifyOtherKeys. The byte that reaches crossterm is therefore
//! indistinguishable from a plain `Enter`, and the TUI submits the
//! prompt instead of inserting a newline. Pi-mono solved this with a
//! native helper (`darwin-modifiers.node`) that asks the macOS HID
//! subsystem whether Shift is physically held at the moment the
//! terminal generated the carriage return.
//!
//! # API
//!
//! [`detect_modifier_state`] is the only entry point. Callers pass
//! the crossterm [`KeyEvent`] they just received; on macOS, when the
//! event is a bare `Enter`, the shim consults CoreGraphics'
//! `CGEventSourceKeyState` and reports a [`ModifierShim`] carrying any
//! modifier bits it recovered. On every other target the function is
//! a zero-cost `None`.
//!
//! Callers OR [`ModifierShim::modifiers`] into the event's existing
//! modifier bitset before dispatching the key downstream. The shim
//! never reports a modifier that the event already carried.
//!
//! # Permissions
//!
//! `CGEventSourceKeyState` reads HID state passively. It does *not*
//! require Accessibility or Input Monitoring entitlements — the
//! returned value is a snapshot of the current hardware state, not
//! a tap on someone else's events.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Modifier bits the platform shim recovered from the OS.
///
/// Callers should OR [`Self::modifiers`] into the event's existing
/// modifier set before downstream dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModifierShim {
    /// Modifier bits the OS reports as held that the terminal failed
    /// to deliver.
    pub modifiers: KeyModifiers,
}

impl Default for ModifierShim {
    fn default() -> Self {
        // crossterm's `KeyModifiers` doesn't impl `Default`, so spell
        // out the empty bitset rather than `#[derive(Default)]`.
        Self {
            modifiers: KeyModifiers::empty(),
        }
    }
}

impl ModifierShim {
    /// `true` when the shim recovered Shift.
    pub fn shift(self) -> bool {
        self.modifiers.contains(KeyModifiers::SHIFT)
    }

    /// Apply the recovered bits to an existing modifier set,
    /// returning the union. Pure helper kept here so callers don't
    /// re-implement the OR convention.
    pub fn apply(self, existing: KeyModifiers) -> KeyModifiers {
        existing | self.modifiers
    }
}

/// Consult the OS for modifier bits the terminal emulator dropped
/// before crossterm saw `event`.
///
/// Returns `Some(ModifierShim)` only when a missing modifier was
/// recovered. Returns `None` when:
///
/// - The event isn't a key shape the shim cares about (today: only
///   bare `Enter`).
/// - The event already carries the modifier the shim would have
///   recovered (avoids redundant work for terminals that *did*
///   negotiate Kitty / modifyOtherKeys).
/// - The OS reported no held modifier.
/// - The target platform has no native implementation.
pub fn detect_modifier_state(event: &KeyEvent) -> Option<ModifierShim> {
    if !shim_applicable(event) {
        return None;
    }
    let probed = ProbedState::probe();
    interpret_probe(event, probed)
}

/// Pure-logic decision shared by [`detect_modifier_state`] and the
/// test suite. Splitting it out lets us cover the matrix of
/// `(event, probed_state)` cases without faking out CoreGraphics.
fn interpret_probe(event: &KeyEvent, probed: ProbedState) -> Option<ModifierShim> {
    if !shim_applicable(event) {
        return None;
    }
    let mut recovered = KeyModifiers::empty();
    if probed.shift {
        recovered |= KeyModifiers::SHIFT;
    }
    // Don't report a modifier the event already carries — that just
    // generates redundant work for callers that already saw the bit
    // via Kitty / modifyOtherKeys.
    recovered.remove(event.modifiers);
    if recovered.is_empty() {
        None
    } else {
        Some(ModifierShim {
            modifiers: recovered,
        })
    }
}

/// Today the shim only triggers on bare `Enter`. The classic pi-mono
/// failure mode is Apple Terminal emitting `\r` for `Shift+Enter`;
/// other modified-Enter shapes (Ctrl+Enter, Alt+Enter) arrive with
/// their modifier intact even on legacy terminals because the legacy
/// escape sequence carries the modifier in the byte stream
/// (`\x1b\r`, `\x00`, etc.).
fn shim_applicable(event: &KeyEvent) -> bool {
    matches!(event.code, KeyCode::Enter)
        && (event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT)
}

/// Snapshot of the OS-reported modifier state.
///
/// Today we only care about Shift; the struct exists so we can grow
/// to Option, Command, Control without churning the call sites.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ProbedState {
    shift: bool,
}

impl ProbedState {
    fn probe() -> Self {
        Self {
            shift: probe_shift(),
        }
    }
}

#[cfg(target_os = "macos")]
fn probe_shift() -> bool {
    macos::shift_is_held()
}

#[cfg(not(target_os = "macos"))]
fn probe_shift() -> bool {
    false
}

#[cfg(target_os = "macos")]
mod macos {
    //! Direct FFI to `CGEventSourceKeyState` in CoreGraphics.framework.
    //!
    //! We bypass the `core-graphics` crate intentionally: that crate
    //! pulls `core-foundation`, `core-graphics-types`, `foreign-types`,
    //! `bitflags`, and `libc` for a one-function need. Direct FFI keeps
    //! the dependency tree zero-cost and avoids `multiple-versions`
    //! churn in `deny.toml`.
    //!
    //! See <https://developer.apple.com/documentation/coregraphics/cgeventsourcekeystate>
    //! and the virtual-keycode list in `<Carbon/HIToolbox/Events.h>`.
    use std::os::raw::c_int;
    use std::sync::atomic::{AtomicBool, Ordering};

    type CGEventSourceStateID = c_int;
    type CGKeyCode = u16;

    /// `kCGEventSourceStateHIDSystemState` — live hardware state
    /// before any pending event tap modifications.
    const HID_SYSTEM_STATE: CGEventSourceStateID = 1;

    /// `kVK_Shift` — virtual keycode for the left Shift key.
    const KVK_SHIFT: CGKeyCode = 0x38;
    /// `kVK_RightShift` — virtual keycode for the right Shift key.
    const KVK_RIGHT_SHIFT: CGKeyCode = 0x3C;

    // SAFETY (link annotation): `CGEventSourceKeyState` is a stable,
    // public CoreGraphics symbol since macOS 10.4. It takes a state
    // ID and a virtual keycode by value and returns a `bool` (mapped
    // to a 1-byte ABI on Darwin). No pointers, no allocations, no
    // thread-affinity. The framework is auto-loaded at process
    // start, so we link against it directly rather than dlsym-loading
    // it.
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceKeyState(state_id: CGEventSourceStateID, key: CGKeyCode) -> bool;
    }

    /// Test seam: production code leaves this `false` so we call into
    /// CoreGraphics; tests can flip it on to short-circuit the FFI
    /// during unit runs that exercise the macOS path on a non-macOS
    /// CI host. (Not currently used — kept for future hermetic tests
    /// that want to assert the macOS branch compiles and is reachable.)
    static SHIFT_OVERRIDE: AtomicBool = AtomicBool::new(false);

    pub(super) fn shift_is_held() -> bool {
        if SHIFT_OVERRIDE.load(Ordering::Relaxed) {
            return true;
        }
        // SAFETY: see the link block above. The call is reentrant
        // and thread-safe per Apple's documentation.
        unsafe {
            CGEventSourceKeyState(HID_SYSTEM_STATE, KVK_SHIFT)
                || CGEventSourceKeyState(HID_SYSTEM_STATE, KVK_RIGHT_SHIFT)
        }
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
