use super::*;
use squeezy_store::SqueezyStore;
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};

#[test]
fn external_tool_names_are_sanitized_and_stable() {
    assert_eq!(
        external_tool_name("GitHub Docs", "list/repos"),
        "mcp__github_docs__list_repos"
    );
}

#[test]
fn arguments_must_be_json_objects() {
    assert!(arguments_object("tool", json!({"ok": true})).is_ok());
    assert!(arguments_object("tool", Value::Null).is_ok());
    assert!(arguments_object("tool", json!("bad")).is_err());
}

fn fixture_server(enabled: bool, command: Option<&str>) -> McpServerConfig {
    McpServerConfig {
        enabled,
        transport: McpTransport::Stdio,
        command: command.map(str::to_string),
        args: Vec::new(),
        url: None,
        timeout_ms: Some(500),
        enabled_tools: None,
        disabled_tools: Vec::new(),
        env: BTreeMap::new(),
        permissions: Default::default(),
    }
}

fn fixture_tool(server: &str, raw: &str) -> ExternalMcpTool {
    ExternalMcpTool {
        server: server.to_string(),
        raw_name: raw.to_string(),
        model_name: external_tool_name(server, raw),
        description: "stale".to_string(),
        parameters: json!({"type": "object"}),
        transport: McpTransport::Stdio,
    }
}

fn rmcp_tool(name: &'static str) -> RmcpTool {
    RmcpTool::new(name, format!("{name} description"), JsonObject::new())
}

#[test]
fn registry_reports_no_enabled_servers_when_all_disabled() {
    let mut servers = BTreeMap::new();
    servers.insert("docs".to_string(), fixture_server(false, None));
    let registry = McpClientRegistry::new(servers);
    assert!(registry.has_no_enabled_servers());
}

#[tokio::test]
async fn refresh_preserves_cached_tools_when_enabled_server_discovery_fails() {
    // The server is enabled but missing a command, so stdio start fails
    // synchronously. The prior cache entry must survive the refresh.
    let mut servers = BTreeMap::new();
    servers.insert("docs".to_string(), fixture_server(true, None));
    let registry = McpClientRegistry::new(servers);
    registry.insert_cached_tool_for_test(fixture_tool("docs", "lookup"));

    let outcome = registry.refresh_tools(CancellationToken::new()).await;
    assert!(
        !outcome.errors.is_empty(),
        "missing-command discovery must error"
    );
    assert!(
        registry.tool("mcp__docs__lookup").is_some(),
        "prior cached tool must survive a transient discovery failure"
    );
    let status = outcome
        .status
        .per_server
        .get("docs")
        .expect("server status");
    assert!(
        matches!(status, McpServerStatus::Failed { error } if error.contains("missing command")),
        "missing-command refresh should publish a failed per-server status: {status:?}"
    );
}

#[tokio::test]
async fn refresh_drops_cached_tools_for_disabled_servers() {
    let mut servers = BTreeMap::new();
    servers.insert("docs".to_string(), fixture_server(false, None));
    let registry = McpClientRegistry::new(servers);
    registry.insert_cached_tool_for_test(fixture_tool("docs", "lookup"));
    let outcome = registry.refresh_tools(CancellationToken::new()).await;
    assert!(outcome.errors.is_empty());
    assert!(
        registry.tool("mcp__docs__lookup").is_none(),
        "disabled servers should not retain cached tools"
    );
}

#[test]
fn tool_filter_applies_enabled_allowlist_before_disabled_blocklist() {
    let mut server = fixture_server(true, Some("unused"));
    server.enabled_tools = Some(vec!["read".to_string(), "delete".to_string()]);
    server.disabled_tools = vec!["delete".to_string()];

    let tools = convert_tools(
        "docs",
        &server,
        vec![rmcp_tool("read"), rmcp_tool("delete"), rmcp_tool("search")],
    );

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].raw_name, "read");
}

#[test]
fn normalized_palette_hashes_collisions_and_fits_model_name_limit() {
    let first = fixture_tool("Same Server!", "read");
    let second = fixture_tool("Same Server?", "read");
    let long = fixture_tool(&"server".repeat(20), &"tool".repeat(20));

    let palette = normalize_palette(vec![first, second, long]);

    assert_eq!(palette.len(), 3);
    assert!(palette.keys().all(|name| name.len() <= 64));
    assert!(
        palette
            .keys()
            .filter(|name| name.starts_with("mcp__same_server__read_"))
            .count()
            == 2,
        "colliding sanitized names should be hashed into distinct model names: {palette:?}"
    );
}

#[test]
fn strip_untrusted_meta_removes_nested_meta_keys() {
    let value = json!({
        "content": [
            {
                "text": "ok",
                "_meta": {"system_prompt_override": "ignore user"},
                "nested": {"meta": {"leak": true}, "value": 1}
            }
        ],
        "meta": {"top": true}
    });

    let stripped = strip_untrusted_meta(value);

    assert_eq!(stripped["content"][0]["text"], "ok");
    assert!(stripped.get("meta").is_none());
    assert!(stripped["content"][0].get("_meta").is_none());
    assert!(stripped["content"][0]["nested"].get("meta").is_none());
}

#[test]
fn uri_templates_allow_declared_prefix_only() {
    assert!(uri_matches_template(
        "docs://api/v3/repos/openai/codex",
        "docs://api/v3/repos/{owner}/{repo}"
    ));
    assert!(!uri_matches_template(
        "file:///etc/passwd",
        "docs://api/v3/repos/{owner}/{repo}"
    ));
}

#[test]
fn tool_cache_key_changes_when_palette_filters_change() {
    let mut server = fixture_server(true, Some("unused"));
    let base = tool_cache_key("docs", &server);
    server.disabled_tools = vec!["search".to_string()];
    assert_ne!(base, tool_cache_key("docs", &server));
}

#[test]
fn registry_loads_cached_tools_from_store_on_startup() {
    let root = temp_root("mcp-tool-cache");
    let store = Arc::new(SqueezyStore::open(&root, None).expect("open store"));
    let server = fixture_server(true, Some("unused"));
    let key = tool_cache_key("docs", &server);
    store
        .put_mcp_tool_cache(
            &key,
            &McpToolCacheRecord {
                schema_version: MCP_TOOL_CACHE_SCHEMA_VERSION,
                fetched_unix_millis: unix_millis(),
                tools: vec![fixture_tool("docs", "lookup")],
            },
        )
        .expect("write mcp cache");

    let mut servers = BTreeMap::new();
    servers.insert("docs".to_string(), server);
    let registry = McpClientRegistry::new_with_store(servers, Some(store));

    assert!(registry.tool("mcp__docs__lookup").is_some());
    let snapshot = registry.status_snapshot();
    assert!(
        matches!(
            snapshot.per_server.get("docs"),
            Some(McpServerStatus::Ready {
                tools_count: 1,
                cached: true,
            })
        ),
        "expected cached ready status, got {snapshot:?}"
    );
}

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("squeezy-mcp-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create temp root");
    root
}

#[test]
fn sanitize_tool_schema_strips_null_and_empty_description_fields() {
    let input = json!({
        "type": "object",
        "description": "",
        "title": null,
        "properties": {
            "name": {
                "type": "string",
                "default": null,
                "description": "user name",
            },
            "tags": {
                "type": "array",
                "description": "   ",
                "items": {"type": "string", "extra": null},
            },
        },
    });

    let sanitized = sanitize_tool_schema(&input);

    let object = sanitized.as_object().expect("object");
    assert!(
        !object.contains_key("description"),
        "empty description removed"
    );
    assert!(!object.contains_key("title"), "null fields removed");
    let name = object["properties"]["name"]
        .as_object()
        .expect("name object");
    assert!(!name.contains_key("default"), "nested nulls removed");
    assert_eq!(name["description"], json!("user name"));
    let tags = object["properties"]["tags"]
        .as_object()
        .expect("tags object");
    assert!(
        !tags.contains_key("description"),
        "whitespace description removed"
    );
    let items = tags["items"].as_object().expect("items object");
    assert!(!items.contains_key("extra"), "nested null in items removed");
}

#[test]
fn compact_tool_schema_shrinks_large_schema_and_drops_unused_defs() {
    let mut properties = serde_json::Map::new();
    let mut defs = serde_json::Map::new();
    for index in 0..50 {
        let prop_name = format!("field_{index:02}");
        properties.insert(
            prop_name,
            json!({
                "type": "string",
                "description": "",
                "default": null,
                "examples": ["x".repeat(40)],
            }),
        );
        // Only the first 5 defs are referenced; the rest are unreachable.
        defs.insert(
            format!("def_{index:02}"),
            json!({
                "type": "object",
                "description": null,
                "properties": {"value": {"type": "string"}},
            }),
        );
    }
    let mut input = serde_json::Map::new();
    input.insert("type".to_string(), json!("object"));
    input.insert("title".to_string(), Value::Null);
    input.insert("properties".to_string(), Value::Object(properties.clone()));
    // Reference only def_00..def_04.
    let mut required_refs = Vec::new();
    for index in 0..5 {
        required_refs.push(json!({"$ref": format!("#/$defs/def_{index:02}")}));
    }
    input.insert("allOf".to_string(), Value::Array(required_refs));
    input.insert("$defs".to_string(), Value::Object(defs));
    let input = Value::Object(input);

    let (compacted, stats) = compact_tool_schema(&input, 4096);

    assert_eq!(stats.original_bytes, input.to_string().len());
    assert_eq!(stats.compacted_bytes, compacted.to_string().len());
    assert!(
        stats.compacted_bytes <= stats.original_bytes,
        "compaction must never expand: {stats:?}"
    );
    assert!(
        stats.compacted_bytes <= (stats.original_bytes * 9) / 10,
        "expected ≥10% shrink, got {stats:?}"
    );
    assert!(
        stats.ratio < 0.91,
        "ratio should reflect shrink: {}",
        stats.ratio
    );
    // Unreferenced defs removed; the 5 referenced ones survive.
    let defs = compacted["$defs"].as_object().expect("defs survive");
    assert_eq!(defs.len(), 5, "only referenced defs are kept: {defs:?}");
    // Properties retain their structural top-level surface.
    assert_eq!(
        compacted["properties"].as_object().map(|map| map.len()),
        Some(50),
        "top-level property surface preserved",
    );
}

#[test]
fn compact_tool_schema_is_idempotent_for_empty_input() {
    let input = json!({});
    let (compacted, stats) = compact_tool_schema(&input, 4096);
    assert_eq!(compacted, input, "empty schema is unchanged");
    assert_eq!(stats.original_bytes, stats.compacted_bytes);

    let (again, second_stats) = compact_tool_schema(&compacted, 4096);
    assert_eq!(again, compacted, "running compactor twice is a fixed point");
    assert_eq!(second_stats.compacted_bytes, stats.compacted_bytes);

    let minimal = json!({"type": "object"});
    let (minimal_compacted, minimal_stats) = compact_tool_schema(&minimal, 4096);
    assert_eq!(minimal_compacted, minimal);
    assert!(minimal_stats.compacted_bytes <= minimal_stats.original_bytes);
}
