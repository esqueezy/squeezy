use super::*;

#[test]
fn parses_minimal_scenario() {
    let toml = r#"
id = "smoke"
title = "Smoke test"

[workspace]
local = "/tmp/repo"

[[steps]]
kind = "prompt"
text = "hello?"
"#;
    let scenario: Scenario = toml::from_str(toml).unwrap();
    assert_eq!(scenario.id, "smoke");
    assert_eq!(scenario.steps.len(), 1);
    match &scenario.workspace {
        WorkspaceSpec::Local { path, .. } => assert_eq!(path, &PathBuf::from("/tmp/repo")),
        other => panic!("expected local workspace, got {other:?}"),
    }
}

#[test]
fn parses_action_step() {
    let toml = r#"
id = "approve"
title = "Approve test"

[workspace]
local = "/tmp/repo"

[[steps]]
kind = "action"
action = "approve"

[steps.match]
tool = "write_file"
"#;
    let scenario: Scenario = toml::from_str(toml).unwrap();
    match &scenario.steps[0] {
        Step::Action(Action::Approve { r#match, .. }) => {
            assert_eq!(
                r#match.as_ref().unwrap().tool.as_deref(),
                Some("write_file")
            );
        }
        other => panic!("expected approve action, got {other:?}"),
    }
}

#[test]
fn parses_github_workspace() {
    let toml = r#"
id = "gh"
title = "GH"

[workspace.github]
repo = "esqueezy/squeezy-fixture"
sha = "deadbeef"
"#;
    let scenario: Scenario = toml::from_str(toml).unwrap();
    match scenario.workspace {
        WorkspaceSpec::Github { github } => {
            assert_eq!(github.repo, "esqueezy/squeezy-fixture");
            assert_eq!(github.sha, "deadbeef");
        }
        other => panic!("expected github workspace, got {other:?}"),
    }
}

#[test]
fn rejects_edit_file_without_payload() {
    let toml = r#"
id = "bad"
title = "bad"

[workspace]
local = "/tmp/repo"

[[steps]]
kind = "action"
action = "edit_file"
path = "x"
"#;
    let scenario: Scenario = toml::from_str(toml).unwrap();
    assert!(scenario.validate().is_err());
}

#[test]
fn parses_inject_mcp_elicitation_form() {
    let toml = r#"
id = "inject"
title = "inject MCP"

[workspace]
local = "/tmp/repo"

[[steps]]
kind = "action"
action = "inject_mcp_elicitation"

[steps.request]
server = "test-server"
kind = "form"
message = "What is the API key?"
"#;
    let scenario: Scenario = toml::from_str(toml).unwrap();
    scenario.validate().expect("scenario should validate");
    match &scenario.steps[0] {
        Step::Action(Action::InjectMcpElicitation { request, .. }) => {
            assert_eq!(request.server, "test-server");
            assert_eq!(request.kind.as_deref(), Some("form"));
            assert_eq!(request.message, "What is the API key?");
        }
        other => panic!("expected inject_mcp_elicitation action, got {other:?}"),
    }
}

#[test]
fn rejects_inject_mcp_elicitation_url_without_url() {
    let toml = r#"
id = "bad-inject"
title = "bad inject"

[workspace]
local = "/tmp/repo"

[[steps]]
kind = "action"
action = "inject_mcp_elicitation"

[steps.request]
server = "test-server"
kind = "url"
message = "Open this URL"
"#;
    let scenario: Scenario = toml::from_str(toml).unwrap();
    assert!(scenario.validate().is_err());
}

#[test]
fn rejects_inject_mcp_elicitation_empty_server() {
    let toml = r#"
id = "bad-inject"
title = "bad inject"

[workspace]
local = "/tmp/repo"

[[steps]]
kind = "action"
action = "inject_mcp_elicitation"

[steps.request]
server = ""
message = "Q"
"#;
    let scenario: Scenario = toml::from_str(toml).unwrap();
    assert!(scenario.validate().is_err());
}
