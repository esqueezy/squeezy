use super::*;

#[test]
fn turn_id_displays_stably() {
    assert_eq!(TurnId::new(42).to_string(), "turn-42");
}

#[test]
fn transcript_constructors_set_roles() {
    assert_eq!(TranscriptItem::user("hello").role, Role::User);
    assert_eq!(TranscriptItem::assistant("hi").role, Role::Assistant);
    assert_eq!(TranscriptItem::system("rules").role, Role::System);
}

#[test]
fn config_without_env_uses_openai_provider_defaults() {
    let config = AppConfig::from_env_vars(|_| None);
    assert_eq!(config.model, DEFAULT_OPENAI_MODEL);
    assert_eq!(config.max_output_tokens, Some(DEFAULT_MAX_OUTPUT_TOKENS));
    assert_eq!(config.permissions, PermissionPolicy::default());
    assert!(!config.store_responses);
    assert_eq!(config.max_parallel_tools, 8);
    match config.provider {
        ProviderConfig::OpenAi(openai) => {
            assert_eq!(openai.api_key_env, "OPENAI_API_KEY");
            assert_eq!(openai.base_url, DEFAULT_OPENAI_BASE_URL);
        }
    }
}

#[test]
fn config_reads_supported_env_overrides() {
    let config = AppConfig::from_env_vars(|name| match name {
        "SQUEEZY_MODEL" => Some("custom-model".to_string()),
        "OPENAI_BASE_URL" => Some("https://example.test/v1".to_string()),
        "SQUEEZY_EDIT_PERMISSION" => Some("allow".to_string()),
        "SQUEEZY_SHELL_PERMISSION" => Some("deny".to_string()),
        "SQUEEZY_STORE_RESPONSES" => Some("true".to_string()),
        "SQUEEZY_MAX_PARALLEL_TOOLS" => Some("3".to_string()),
        _ => None,
    });

    assert_eq!(config.model, "custom-model");
    assert_eq!(config.permissions.edit, PermissionMode::Allow);
    assert_eq!(config.permissions.shell, PermissionMode::Deny);
    assert!(config.store_responses);
    assert_eq!(config.max_parallel_tools, 3);
    match config.provider {
        ProviderConfig::OpenAi(openai) => {
            assert_eq!(openai.base_url, "https://example.test/v1");
        }
    }
}

#[test]
fn permission_mode_parses_expected_values() {
    assert_eq!(PermissionMode::parse("allow"), Some(PermissionMode::Allow));
    assert_eq!(PermissionMode::parse("ASK"), Some(PermissionMode::Ask));
    assert_eq!(PermissionMode::parse("deny"), Some(PermissionMode::Deny));
    assert_eq!(PermissionMode::parse("maybe"), None);
}
