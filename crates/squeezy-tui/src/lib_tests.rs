use std::collections::BTreeMap;

use squeezy_core::{PermissionCapability, PermissionRequest, PermissionRisk, PermissionScope};

use super::*;

#[test]
fn app_starts_ready_with_empty_transcript() {
    let app = TuiApp::new("openai", "gpt-test".to_string(), "defaults".to_string());

    assert_eq!(app.provider_name, "openai");
    assert_eq!(app.model, "gpt-test");
    assert_eq!(app.status, "ready");
    assert!(app.transcript.is_empty());
}

#[test]
fn transcript_item_formats_role_label() {
    let item = TranscriptItem::user("hello");
    let line = format_transcript_item(&item);
    let text = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(text, "user hello");
}

#[test]
fn approval_prompt_surfaces_risk_target_and_persistence_keys() {
    let permission = PermissionRequest {
        call_id: "call".to_string(),
        tool_name: "shell".to_string(),
        capability: PermissionCapability::Shell,
        target: "cargo test:*".to_string(),
        risk: PermissionRisk::High,
        summary: "shell description=\"run tests\" command=\"cargo test\"".to_string(),
        metadata: BTreeMap::from([
            ("command".to_string(), "cargo test".to_string()),
            ("cwd".to_string(), ".".to_string()),
            ("env".to_string(), "allowlist/redacted".to_string()),
            ("network".to_string(), "none".to_string()),
            ("destructive".to_string(), "false".to_string()),
        ]),
        suggested_rules: Vec::new(),
    };
    let request = ToolApprovalRequest {
        id: 1,
        call_id: "call".to_string(),
        tool_name: "shell".to_string(),
        scope: PermissionScope::Shell,
        permission,
        matched_rule: None,
        reason: "default shell permission is ask".to_string(),
    };

    let prompt = format_approval_prompt(&request);
    assert!(prompt.contains("risk=high"), "missing risk: {prompt}");
    assert!(
        prompt.contains("target=cargo test:*"),
        "missing target: {prompt}",
    );
    assert!(
        prompt.contains("command=\"cargo test\""),
        "missing command metadata: {prompt}",
    );
    assert!(prompt.contains("cwd=\".\""), "missing cwd: {prompt}");
    assert!(
        prompt.contains("env=\"allowlist/redacted\""),
        "missing env: {prompt}"
    );
    assert!(
        prompt.contains("network=\"none\""),
        "missing network: {prompt}"
    );
    assert!(
        prompt.contains("destructive=\"false\""),
        "missing destructive flag: {prompt}",
    );
    assert!(prompt.contains("y once"));
    assert!(prompt.contains("a user allow"));
    assert!(prompt.contains("p project allow"));
    assert!(prompt.contains("u user deny"));
    assert!(prompt.contains("d project deny"));
    assert!(prompt.contains("n deny once"));
}
