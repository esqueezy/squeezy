use super::*;

#[test]
fn match_slash_command_prefix_returns_command_length() {
    assert_eq!(match_slash_command_prefix("/help"), Some(5));
    assert_eq!(
        match_slash_command_prefix("/help changing the model"),
        Some(5)
    );
}

#[test]
fn match_slash_command_prefix_prefers_longest_match() {
    // `/job-cancel foo` must resolve to `/job-cancel`, not `/job`.
    assert_eq!(
        match_slash_command_prefix("/job-cancel abc"),
        Some("/job-cancel".len())
    );
}

#[test]
fn match_slash_command_prefix_requires_word_boundary() {
    // `/helpme` is not `/help`.
    assert_eq!(match_slash_command_prefix("/helpme"), None);
    // `/config-foo` is not `/config`.
    assert_eq!(match_slash_command_prefix("/config-foo"), None);
}

#[test]
fn match_slash_command_prefix_rejects_unknown_or_non_slash() {
    assert_eq!(match_slash_command_prefix("/notacommand"), None);
    assert_eq!(match_slash_command_prefix("help"), None);
    assert_eq!(match_slash_command_prefix(""), None);
}

fn find_command(name: &str) -> &'static SlashCommand {
    SLASH_COMMANDS
        .iter()
        .find(|cmd| cmd.name == name)
        .unwrap_or_else(|| panic!("slash command {name} not registered"))
}

#[test]
fn slash_commands_declare_expected_capabilities() {
    // Anchors the audited capability mapping so future edits to the catalog
    // stay deliberate rather than accidentally silent.
    assert_eq!(
        find_command("/help").capabilities,
        &[PermissionCapability::Network]
    );
    assert_eq!(
        find_command("/compact").capabilities,
        &[PermissionCapability::Network]
    );
    assert_eq!(
        find_command("/feedback").capabilities,
        &[PermissionCapability::Network]
    );
    assert_eq!(
        find_command("/report").capabilities,
        &[PermissionCapability::Network]
    );
    assert_eq!(
        find_command("/attach").capabilities,
        &[PermissionCapability::Read]
    );
    assert_eq!(
        find_command("/diff").capabilities,
        &[PermissionCapability::Git, PermissionCapability::Read]
    );
    assert_eq!(
        find_command("/model").capabilities,
        &[PermissionCapability::Edit]
    );
    assert_eq!(
        find_command("/theme").capabilities,
        &[PermissionCapability::Edit]
    );
    assert_eq!(
        find_command("/session-cleanup").capabilities,
        &[PermissionCapability::Destructive]
    );
    assert_eq!(
        find_command("/undo").capabilities,
        &[PermissionCapability::Edit, PermissionCapability::Destructive]
    );
    assert_eq!(
        find_command("/revert-turn").capabilities,
        &[PermissionCapability::Edit, PermissionCapability::Destructive]
    );
}

#[test]
fn purely_informational_slash_commands_declare_no_capabilities() {
    // `/cost`, `/context`, `/jobs`, `/pin`, etc. only read in-memory state.
    // Showing capability badges on them would dilute the signal for commands
    // that actually touch the world.
    for name in [
        "/cost",
        "/context",
        "/jobs",
        "/job",
        "/job-cancel",
        "/pins",
        "/pin",
        "/unpin",
        "/expand",
        "/collapse",
        "/copy",
        "/attachments",
        "/detach",
        "/plan",
        "/build",
    ] {
        assert!(
            find_command(name).capabilities.is_empty(),
            "expected {name} to have no capability badges, got {:?}",
            find_command(name).capabilities,
        );
    }
}

#[test]
fn capability_badges_match_capability_as_str() {
    let cmd = find_command("/diff");
    assert_eq!(cmd.capability_badges(), vec!["git", "read"]);
    let cmd = find_command("/help");
    assert_eq!(cmd.capability_badges(), vec!["net"]);
    let cmd = find_command("/session-cleanup");
    assert_eq!(cmd.capability_badges(), vec!["destructive"]);
}

#[test]
fn capability_badge_labels_are_stable() {
    // Order-independent guarantee that every variant has a short label so a
    // future capability added to squeezy_core surfaces visibly rather than
    // panicking the renderer at run time.
    let variants = [
        PermissionCapability::Read,
        PermissionCapability::Search,
        PermissionCapability::Edit,
        PermissionCapability::Shell,
        PermissionCapability::Network,
        PermissionCapability::Mcp,
        PermissionCapability::Git,
        PermissionCapability::Compiler,
        PermissionCapability::Destructive,
    ];
    for cap in variants {
        let label = capability_badge_label(cap);
        assert!(!label.is_empty(), "{cap:?} produced an empty badge");
    }
}
