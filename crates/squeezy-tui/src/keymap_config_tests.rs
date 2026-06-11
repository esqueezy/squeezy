use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyModifiers};

use super::*;
use crate::keymap::{Action, KeymapResolver};

fn unique_temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("squeezy_keymap_cfg_{label}_{nonce}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir.join("keybindings.toml")
}

#[test]
fn default_keybindings_path_uses_home_from_env() {
    let path = default_keybindings_path_from_env(|key| {
        (key == "HOME").then(|| OsString::from("/home/alice"))
    });

    assert_eq!(
        path,
        Some(PathBuf::from("/home/alice/.squeezy/keybindings.toml"))
    );
}

#[cfg(windows)]
#[test]
fn default_keybindings_path_falls_back_to_userprofile_on_windows() {
    let path = default_keybindings_path_from_env(|key| {
        (key == "USERPROFILE").then(|| OsString::from(r"C:\Users\Alice"))
    });

    assert_eq!(
        path,
        Some(
            PathBuf::from(r"C:\Users\Alice")
                .join(".squeezy")
                .join("keybindings.toml")
        )
    );
}

#[cfg(not(windows))]
#[test]
fn default_keybindings_path_ignores_userprofile_off_windows() {
    let path = default_keybindings_path_from_env(|key| {
        (key == "USERPROFILE").then(|| OsString::from("/Users/Alice"))
    });

    assert_eq!(path, None);
}

#[test]
fn default_keybindings_path_treats_empty_home_as_unset() {
    // Guard against a `HOME=""` regression: an empty env var used to
    // resolve to `Some(PathBuf::from("/.squeezy/keybindings.toml"))`,
    // which would silently look for the file at the filesystem root.
    // On Unix, `USERPROFILE` is ignored, so we expect `None`.
    #[cfg(not(windows))]
    {
        let path =
            default_keybindings_path_from_env(|key| (key == "HOME").then(|| OsString::from("")));
        assert_eq!(path, None);
    }
    // On Windows, empty `HOME` should also be ignored, with fall-through
    // to `USERPROFILE` (or `None` when it is missing/empty as well).
    #[cfg(windows)]
    {
        let none = default_keybindings_path_from_env(|key| match key {
            "HOME" => Some(OsString::from("")),
            "USERPROFILE" => Some(OsString::from("")),
            _ => None,
        });
        assert_eq!(none, None);

        let path = default_keybindings_path_from_env(|key| match key {
            "HOME" => Some(OsString::from("")),
            "USERPROFILE" => Some(OsString::from(r"C:\Users\Alice")),
            _ => None,
        });
        assert_eq!(
            path,
            Some(
                PathBuf::from(r"C:\Users\Alice")
                    .join(".squeezy")
                    .join("keybindings.toml")
            )
        );
    }
}

#[test]
fn empty_file_keeps_default_bindings() {
    let file = KeybindingsFile::from_toml_str("").expect("empty toml parses");
    let overrides = file.into_override_map().expect("no validation errors");
    assert!(overrides.is_empty());

    let resolver = KeymapResolver::from_overrides(&overrides);
    for action in Action::ALL.iter().copied() {
        assert_eq!(
            resolver.binding(action),
            action.default_binding(),
            "{} should fall back to its compiled-in default",
            action.slug(),
        );
    }
    assert_eq!(
        resolver.lookup(KeyCode::PageUp, KeyModifiers::NONE),
        Some(Action::ScrollTranscriptPageUp),
    );
    assert_eq!(
        resolver.lookup(KeyCode::Char('t'), KeyModifiers::CONTROL),
        Some(Action::ToggleTranscriptOverlay),
    );
}

#[test]
fn missing_user_file_returns_base_unchanged() {
    let mut base = BTreeMap::new();
    base.insert("transcript_overlay".to_string(), "Ctrl+y".to_string());
    let path = unique_temp_path("missing").with_file_name("does-not-exist.toml");
    let merged = merge_user_overrides(base.clone(), Some(&path)).expect("missing file is ok");
    assert_eq!(merged, base);
}

#[test]
fn none_user_path_returns_base_unchanged() {
    let mut base = BTreeMap::new();
    base.insert("page_up".to_string(), "Alt+u".to_string());
    let merged = merge_user_overrides(base.clone(), None).expect("None path is ok");
    assert_eq!(merged, base);
}

#[test]
fn user_override_applies() {
    let toml_content = r#"
        [[bindings]]
        key = "Ctrl+o"
        action = "transcript_overlay"

        [[bindings]]
        key = "Alt+k"
        action = "page_up"
    "#;
    let file = KeybindingsFile::from_toml_str(toml_content).expect("toml parses");
    let overrides = file.into_override_map().expect("validates");
    let resolver = KeymapResolver::from_overrides(&overrides);

    assert_eq!(
        resolver.lookup(KeyCode::Char('o'), KeyModifiers::CONTROL),
        Some(Action::ToggleTranscriptOverlay),
    );
    assert_eq!(
        resolver.lookup(KeyCode::Char('k'), KeyModifiers::ALT),
        Some(Action::ScrollTranscriptPageUp),
    );
    assert_ne!(
        resolver.lookup(KeyCode::Char('t'), KeyModifiers::CONTROL),
        Some(Action::ToggleTranscriptOverlay),
    );
    assert_ne!(
        resolver.lookup(KeyCode::PageUp, KeyModifiers::NONE),
        Some(Action::ScrollTranscriptPageUp),
    );
}

#[test]
fn reserved_binding_override_returns_error() {
    let toml_content = r#"
        [[bindings]]
        key = "Ctrl+C"
        action = "transcript_overlay"
    "#;
    let file = KeybindingsFile::from_toml_str(toml_content).expect("toml parses");
    let err = file
        .into_override_map()
        .expect_err("reserved bindings rejected");
    match err {
        KeybindingsError::ReservedKey {
            action, reserved, ..
        } => {
            assert_eq!(action, Action::ToggleTranscriptOverlay);
            assert_eq!(reserved, "Ctrl+C");
        }
        other => panic!("expected ReservedKey error, got {other:?}"),
    }
}

#[test]
fn reserved_binding_check_is_case_insensitive() {
    let lower = r#"
        [[bindings]]
        key = "ctrl+c"
        action = "transcript_overlay"
    "#;
    let err = KeybindingsFile::from_toml_str(lower)
        .expect("parses")
        .into_override_map()
        .expect_err("Ctrl+c rejected");
    assert!(matches!(
        err,
        KeybindingsError::ReservedKey {
            reserved: "Ctrl+C",
            ..
        }
    ));
}

#[test]
fn esc_is_reserved() {
    let toml_content = r#"
        [[bindings]]
        key = "Esc"
        action = "transcript_overlay"
    "#;
    let err = KeybindingsFile::from_toml_str(toml_content)
        .expect("parses")
        .into_override_map()
        .expect_err("Esc rejected");
    assert!(matches!(
        err,
        KeybindingsError::ReservedKey {
            reserved: "Esc",
            ..
        }
    ));
}

#[test]
fn ctrl_d_is_reserved() {
    let toml_content = r#"
        [[bindings]]
        key = "Ctrl+D"
        action = "transcript_overlay"
    "#;
    let err = KeybindingsFile::from_toml_str(toml_content)
        .expect("parses")
        .into_override_map()
        .expect_err("Ctrl+D rejected");
    assert!(matches!(
        err,
        KeybindingsError::ReservedKey {
            reserved: "Ctrl+D",
            ..
        }
    ));
}

#[test]
fn invalid_keyspec_returns_error() {
    let toml_content = r#"
        [[bindings]]
        key = "totally-not-a-key"
        action = "transcript_overlay"
    "#;
    let err = KeybindingsFile::from_toml_str(toml_content)
        .expect("parses")
        .into_override_map()
        .expect_err("garbage keyspec rejected");
    assert!(matches!(err, KeybindingsError::InvalidKeyspec { .. }));
}

#[test]
fn unknown_action_rejected_at_parse_time() {
    let toml_content = r#"
        [[bindings]]
        key = "Ctrl+o"
        action = "not_a_real_action"
    "#;
    let err = KeybindingsFile::from_toml_str(toml_content).expect_err("unknown action rejected");
    assert!(matches!(err, KeybindingsError::Parse { .. }));
}

#[test]
fn merge_user_overrides_loads_from_disk() {
    let path = unique_temp_path("merge");
    fs::write(
        &path,
        r#"
[[bindings]]
key = "Ctrl+o"
action = "transcript_overlay"
"#,
    )
    .expect("write temp file");
    let merged = merge_user_overrides(BTreeMap::new(), Some(&path)).expect("merges from disk");
    assert_eq!(
        merged.get("transcript_overlay").map(String::as_str),
        Some("Ctrl+o"),
    );
}

#[test]
fn merge_user_overrides_lets_user_file_win_over_base() {
    let path = unique_temp_path("precedence");
    fs::write(
        &path,
        r#"
[[bindings]]
key = "Alt+k"
action = "page_up"
"#,
    )
    .expect("write temp file");
    let mut base = BTreeMap::new();
    base.insert("page_up".to_string(), "Ctrl+u".to_string());
    base.insert("page_down".to_string(), "Alt+j".to_string());
    let merged = merge_user_overrides(base, Some(&path)).expect("merges from disk");
    assert_eq!(merged.get("page_up").map(String::as_str), Some("Alt+k"));
    assert_eq!(merged.get("page_down").map(String::as_str), Some("Alt+j"));
}

#[test]
fn merge_user_overrides_propagates_reserved_error() {
    let path = unique_temp_path("reserved_disk");
    fs::write(
        &path,
        r#"
[[bindings]]
key = "Ctrl+C"
action = "transcript_overlay"
"#,
    )
    .expect("write temp file");
    let err = merge_user_overrides(BTreeMap::new(), Some(&path))
        .expect_err("reserved binding bubbles up");
    assert!(matches!(
        err,
        KeybindingsError::ReservedKey {
            reserved: "Ctrl+C",
            ..
        }
    ));
}

#[test]
fn squeezy_keybindings_env_override_pins_scratch_path_over_home() {
    // deep-review #70: the eval `TuiHarness` and any out-of-crate driver
    // build their keymap from `default_keybindings_path()`, which (before
    // this fix) read the operator's real `~/.squeezy/keybindings.toml`
    // unconditionally. `SQUEEZY_KEYBINDINGS` must override the home path so
    // key routing resolves from a scratch file instead. Drive the env
    // resolver directly (no process-global mutation) to prove the override
    // wins and `$HOME` is never consulted when it is set.
    let scratch = unique_temp_path("env_override");
    fs::write(
        &scratch,
        r#"
[[bindings]]
key = "Ctrl+o"
action = "transcript_overlay"
"#,
    )
    .expect("write scratch keybindings");

    let scratch_os = scratch.clone().into_os_string();
    let resolved = default_keybindings_path_from_env(|key| match key {
        "SQUEEZY_KEYBINDINGS" => Some(scratch_os.clone()),
        // A real $HOME is present, yet must be ignored while the override
        // is set — this is the operator's machine in the eval driver.
        "HOME" => Some(OsString::from("/home/operator")),
        _ => None,
    });
    assert_eq!(
        resolved.as_deref(),
        Some(scratch.as_path()),
        "the env override must win over the home-derived path",
    );
    assert_ne!(
        resolved,
        Some(PathBuf::from("/home/operator/.squeezy/keybindings.toml")),
        "resolution must NOT fall through to the operator's real ~/.squeezy",
    );

    // And the resolver built from the scratch file honors the rebind:
    // transcript_overlay moves from its Ctrl+T default to Ctrl+O.
    let merged = merge_user_overrides(BTreeMap::new(), resolved.as_deref())
        .expect("scratch keybindings merge cleanly");
    let resolver = KeymapResolver::from_overrides(&merged);
    assert_eq!(
        resolver.lookup(KeyCode::Char('o'), KeyModifiers::CONTROL),
        Some(Action::ToggleTranscriptOverlay),
        "the scratch rebind is in effect",
    );
    assert_ne!(
        resolver.lookup(KeyCode::Char('t'), KeyModifiers::CONTROL),
        Some(Action::ToggleTranscriptOverlay),
        "the default Ctrl+T binding was displaced by the scratch file",
    );
}

#[test]
fn squeezy_keybindings_empty_env_sentinel_bypasses_home() {
    // An empty `SQUEEZY_KEYBINDINGS` is the "no user overrides" sentinel:
    // it bypasses the home file entirely (resolves to None) rather than
    // looking under a real `$HOME`. This lets the harness fully neutralize
    // operator keybindings without pointing at a placeholder file.
    let resolved = default_keybindings_path_from_env(|key| match key {
        "SQUEEZY_KEYBINDINGS" => Some(OsString::from("")),
        "HOME" => Some(OsString::from("/home/operator")),
        _ => None,
    });
    assert_eq!(
        resolved, None,
        "empty SQUEEZY_KEYBINDINGS bypasses the home file with no user overrides",
    );
}

#[test]
fn unset_squeezy_keybindings_env_falls_through_to_home() {
    // Production fallback: with no `SQUEEZY_KEYBINDINGS`, resolution must
    // still derive from `$HOME` so real users keep their hand-edited file.
    let resolved = default_keybindings_path_from_env(|key| {
        (key == "HOME").then(|| OsString::from("/home/operator"))
    });
    assert_eq!(
        resolved,
        Some(PathBuf::from("/home/operator/.squeezy/keybindings.toml")),
        "unset override keeps the home-derived path",
    );
}
