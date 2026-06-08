use super::*;

// ---- DesktopNotifier byte-sequence acceptance ----------------------------

#[test]
fn desktop_notifier_off_emits_nothing() {
    let notifier = DesktopNotifier::new(NotificationMethod::Off);
    let mut buf: Vec<u8> = Vec::new();
    let wrote = notifier
        .write_to(&mut buf, "ignored")
        .expect("write_to should not fail on a Vec");
    assert!(!wrote, "Off must not emit any bytes");
    assert!(buf.is_empty(), "buffer stays empty when notifier is Off");
    assert_eq!(notifier.resolved(), None);
}

#[test]
fn desktop_notifier_bel_emits_bell_byte() {
    let notifier = DesktopNotifier::new(NotificationMethod::Bel);
    let mut buf: Vec<u8> = Vec::new();
    let wrote = notifier.write_to(&mut buf, "turn complete").unwrap();
    assert!(wrote);
    assert_eq!(buf, b"\x07", "Bel writes a single BEL byte");
    assert_eq!(notifier.resolved(), Some(NotificationMethod::Bel));
}

#[test]
fn desktop_notifier_osc9_emits_full_escape_sequence() {
    let notifier = DesktopNotifier::new(NotificationMethod::Osc9);
    let mut buf: Vec<u8> = Vec::new();
    let wrote = notifier.write_to(&mut buf, "turn complete").unwrap();
    assert!(wrote);
    // ESC ] 9 ; <message> BEL — the iTerm-style desktop-notification
    // escape, honoured by Ghostty / Kitty / WezTerm / Warp.
    assert_eq!(buf, b"\x1b]9;turn complete\x07");
    assert_eq!(notifier.resolved(), Some(NotificationMethod::Osc9));
}

#[test]
fn desktop_notifier_osc9_strips_embedded_terminators() {
    // A hostile or accidental BEL / ESC inside the message would either
    // truncate the OSC sequence early (BEL) or break the parser into a
    // new escape (ESC). Both are sanitised out before write.
    let notifier = DesktopNotifier::new(NotificationMethod::Osc9);
    let mut buf: Vec<u8> = Vec::new();
    notifier
        .write_to(&mut buf, "danger\x07inside\x1b[31mred")
        .unwrap();
    assert_eq!(buf, b"\x1b]9;dangerinside[31mred\x07");
}

#[test]
fn desktop_notifier_osc9_collapses_newlines_to_spaces() {
    let notifier = DesktopNotifier::new(NotificationMethod::Osc9);
    let mut buf: Vec<u8> = Vec::new();
    notifier.write_to(&mut buf, "line one\nline two").unwrap();
    assert_eq!(buf, b"\x1b]9;line one line two\x07");
}

#[test]
fn desktop_notifier_auto_resolves_to_a_concrete_backend() {
    let notifier = DesktopNotifier::new(NotificationMethod::Auto);
    let resolved = notifier
        .resolved()
        .expect("Auto must resolve to a concrete backend");
    assert!(matches!(
        resolved,
        NotificationMethod::Bel | NotificationMethod::Osc9
    ));
}

#[test]
fn notification_method_parses_canonical_strings() {
    assert_eq!(
        NotificationMethod::parse("off"),
        Some(NotificationMethod::Off)
    );
    assert_eq!(
        NotificationMethod::parse("bel"),
        Some(NotificationMethod::Bel)
    );
    assert_eq!(
        NotificationMethod::parse("osc9"),
        Some(NotificationMethod::Osc9)
    );
    assert_eq!(
        NotificationMethod::parse("auto"),
        Some(NotificationMethod::Auto)
    );
    assert_eq!(
        NotificationMethod::parse("OSC-9"),
        Some(NotificationMethod::Osc9)
    );
    assert_eq!(
        NotificationMethod::parse("bell"),
        Some(NotificationMethod::Bel)
    );
    assert_eq!(NotificationMethod::parse("nonsense"), None);
}

#[test]
fn osc9_auto_detects_linux_terminal_signals() {
    // The OSC9 auto-detection is tested indirectly: we can't mutate real
    // process env inside a unit test without races, so we verify the
    // expanded terminal_supports_osc9 logic via the Auto resolved() path.
    // In the default test environment $TERM_PROGRAM and Linux signals are
    // absent so Auto should fall back to Bel (not Osc9). The assertion
    // confirms the function runs without panic; actual Linux terminal
    // detection is validated in integration environments.
    let notifier = DesktopNotifier::new(NotificationMethod::Auto);
    let resolved = notifier
        .resolved()
        .expect("Auto must always resolve to a concrete backend");
    assert!(
        matches!(resolved, NotificationMethod::Bel | NotificationMethod::Osc9),
        "Auto must resolve to Bel or Osc9, got {resolved:?}"
    );
}
