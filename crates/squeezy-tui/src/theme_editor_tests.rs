//! Unit tests for the pure Theme Editor model (§12.7.2). These exercise the role
//! navigation, channel navigation/adjust, and the working-RGB rules directly,
//! with no terminal — the editor's keyboard/mouse/render integration through the
//! real `render()` is covered by the capture-sink suite in `lib_tests.rs`.

use super::*;

/// A simple seed closure for tests: distinct, deterministic colors per token so a
/// reseed is observable. Real callers pass `render::theme::rgb`.
fn seed(token: &'static str) -> TuiRgb {
    // Derive a stable triple from the token's length so different roles seed to
    // different colors without depending on the runtime theme registry.
    let n = (token.len() % 200) as u8;
    [n, n.wrapping_add(10), n.wrapping_add(20)]
}

#[test]
fn the_role_list_only_holds_real_theme_color_tokens() {
    assert!(
        !ROLES.is_empty(),
        "the editor must expose at least one role"
    );
    for role in ROLES {
        assert!(
            squeezy_core::is_tui_theme_color_token(role.token),
            "role {} maps to an unknown theme token {}",
            role.label,
            role.token
        );
        assert!(!role.label.is_empty(), "every role needs a label");
        assert!(
            !role.description.is_empty(),
            "every role needs a description"
        );
    }
}

#[test]
fn a_fresh_editor_focuses_the_first_role_and_seeds_its_rgb() {
    let state = ThemeEditorState::new([10, 20, 30]);
    assert_eq!(state.role_index(), 0);
    assert_eq!(state.current_role(), ROLES[0]);
    assert_eq!(state.channel(), Channel::Red);
    assert_eq!(state.rgb(), [10, 20, 30]);
    assert_eq!(state.channel_value(Channel::Red), 10);
    assert_eq!(state.channel_value(Channel::Green), 20);
    assert_eq!(state.channel_value(Channel::Blue), 30);
}

#[test]
fn focus_next_role_advances_and_reseeds_the_working_rgb() {
    let mut state = ThemeEditorState::new([0, 0, 0]);
    let moved = state.focus_next_role(seed);
    assert_eq!(moved, Some(ROLES[1]));
    assert_eq!(state.role_index(), 1);
    // Reseeded from the new role's seed color, and the channel reset to Red.
    assert_eq!(state.rgb(), seed(ROLES[1].token));
    assert_eq!(state.channel(), Channel::Red);
}

#[test]
fn focus_prev_role_at_the_top_is_a_no_op() {
    let mut state = ThemeEditorState::new([1, 2, 3]);
    assert_eq!(state.focus_prev_role(seed), None);
    assert_eq!(state.role_index(), 0);
    // The working RGB is untouched by a no-op move.
    assert_eq!(state.rgb(), [1, 2, 3]);
}

#[test]
fn focus_next_role_at_the_bottom_is_a_no_op() {
    let mut state = ThemeEditorState::new([0, 0, 0]);
    // Walk to the last role.
    while state.focus_next_role(seed).is_some() {}
    assert_eq!(state.role_index(), ROLES.len() - 1);
    let before = state.rgb();
    assert_eq!(state.focus_next_role(seed), None);
    assert_eq!(state.role_index(), ROLES.len() - 1);
    assert_eq!(state.rgb(), before);
}

#[test]
fn focus_role_by_index_jumps_and_reseeds_but_ignores_out_of_range_and_no_op() {
    let mut state = ThemeEditorState::new([0, 0, 0]);
    // Jump directly to role 2.
    let jumped = state.focus_role(2, seed);
    assert_eq!(jumped, Some(ROLES[2]));
    assert_eq!(state.role_index(), 2);
    assert_eq!(state.rgb(), seed(ROLES[2].token));
    // Re-focusing the already-current role is a no-op (no reseed signal).
    assert_eq!(state.focus_role(2, seed), None);
    // Out-of-range index is ignored.
    assert_eq!(state.focus_role(ROLES.len(), seed), None);
    assert_eq!(state.role_index(), 2);
}

#[test]
fn channel_focus_steps_and_clamps_at_both_ends() {
    let mut state = ThemeEditorState::new([0, 0, 0]);
    assert_eq!(state.channel(), Channel::Red);
    state.focus_prev_channel();
    assert_eq!(state.channel(), Channel::Red, "clamped at Red");
    state.focus_next_channel();
    assert_eq!(state.channel(), Channel::Green);
    state.focus_next_channel();
    assert_eq!(state.channel(), Channel::Blue);
    state.focus_next_channel();
    assert_eq!(state.channel(), Channel::Blue, "clamped at Blue");
    state.focus_prev_channel();
    assert_eq!(state.channel(), Channel::Green);
}

#[test]
fn adjust_channel_nudges_the_focused_channel_and_saturates() {
    let mut state = ThemeEditorState::new([100, 100, 100]);
    // Default focus is Red.
    let rgb = state.adjust_channel(5);
    assert_eq!(rgb, [105, 100, 100]);
    // Move to Green and adjust down.
    state.focus_next_channel();
    let rgb = state.adjust_channel(-40);
    assert_eq!(rgb, [105, 60, 100]);
    // Saturate at the floor.
    let rgb = state.adjust_channel(-1000);
    assert_eq!(rgb, [105, 0, 100]);
    // Move to Blue and saturate at the ceiling.
    state.focus_next_channel();
    let rgb = state.adjust_channel(1000);
    assert_eq!(rgb, [105, 0, 255]);
}

#[test]
fn set_channel_focuses_and_sets_an_absolute_value() {
    let mut state = ThemeEditorState::new([0, 0, 0]);
    let rgb = state.set_channel(Channel::Blue, 200);
    assert_eq!(rgb, [0, 0, 200]);
    // set_channel also moves the focus to the channel it set.
    assert_eq!(state.channel(), Channel::Blue);
    let rgb = state.set_channel(Channel::Green, 64);
    assert_eq!(rgb, [0, 64, 200]);
    assert_eq!(state.channel(), Channel::Green);
}

#[test]
fn set_rgb_replaces_the_whole_working_triple() {
    let mut state = ThemeEditorState::new([1, 2, 3]);
    state.set_rgb([9, 8, 7]);
    assert_eq!(state.rgb(), [9, 8, 7]);
}

#[test]
fn channel_index_and_label_are_stable() {
    assert_eq!(Channel::Red.index(), 0);
    assert_eq!(Channel::Green.index(), 1);
    assert_eq!(Channel::Blue.index(), 2);
    assert_eq!(Channel::Red.label(), 'R');
    assert_eq!(Channel::Green.label(), 'G');
    assert_eq!(Channel::Blue.label(), 'B');
    assert_eq!(
        Channel::ALL,
        [Channel::Red, Channel::Green, Channel::Blue],
        "the render/edit order is R,G,B"
    );
}
