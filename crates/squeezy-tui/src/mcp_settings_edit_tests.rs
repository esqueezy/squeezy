//! Unit tests for the `[mcp.servers]` TOML editor invoked by the
//! `/mcp` config page when persisting toggle/add/remove actions
//! (`crates/squeezy-tui/src/mcp_settings_edit.rs`). The tests drive
//! the editor directly against temp-file inputs so they cover both
//! fresh and existing settings files without standing up a
//! `ConfigScreenState`.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{mcp_server_table, mcp_settings_edit};

/// Generate a unique temp path so concurrent test runs do not clash.
/// We rely on the process id plus a monotonic counter rather than
/// pulling `tempfile` into the workspace just for these tests.
fn unique_temp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("squeezy-mcp-edit-{label}-{pid}-{n}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn mcp_settings_edit_creates_parent_dir_and_inserts_table() {
    let dir = unique_temp_dir("insert");
    // Path points at a nested directory that does NOT exist yet —
    // mirrors the first-write scenario when the Repo or Local tier
    // file has never been written.
    let path = dir.join("nested/etc/squeezy/settings.toml");
    mcp_settings_edit(&path, |servers| {
        let mut table = toml_edit::Table::new();
        table.insert(
            "enabled",
            toml_edit::Item::Value(toml_edit::Value::from(true)),
        );
        table.insert(
            "transport",
            toml_edit::Item::Value(toml_edit::Value::from("stdio")),
        );
        table.insert(
            "command",
            toml_edit::Item::Value(toml_edit::Value::from("docs-mcp")),
        );
        servers.insert("docs", toml_edit::Item::Table(table));
        Ok(())
    })
    .expect("edit succeeds");

    let text = fs::read_to_string(&path).expect("file written");
    assert!(
        text.contains("[mcp.servers.docs]"),
        "table header must be present: {text}"
    );
    assert!(text.contains("enabled = true"), "enabled persists: {text}");
    assert!(
        text.contains("command = \"docs-mcp\""),
        "command persists: {text}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_settings_edit_toggles_existing_enabled_flag() {
    let dir = unique_temp_dir("toggle");
    let path = dir.join("settings.toml");
    fs::write(
        &path,
        "[mcp.servers.docs]\nenabled = true\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\n",
    )
    .expect("seed file");

    mcp_settings_edit(&path, |servers| {
        let entry = servers
            .entry("docs")
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
        let table = entry.as_table_mut().expect("table");
        table.insert(
            "enabled",
            toml_edit::Item::Value(toml_edit::Value::from(false)),
        );
        Ok(())
    })
    .expect("toggle persists");

    let text = fs::read_to_string(&path).expect("file readable");
    assert!(text.contains("enabled = false"), "toggle wrote: {text}");
    assert!(
        text.contains("command = \"docs-mcp\""),
        "sibling keys must survive: {text}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_settings_edit_removes_table_entry() {
    let dir = unique_temp_dir("remove");
    let path = dir.join("settings.toml");
    fs::write(
        &path,
        "[mcp.servers.keep]\nenabled = true\ncommand = \"keep\"\n\n\
         [mcp.servers.drop]\nenabled = true\ncommand = \"drop\"\n",
    )
    .expect("seed file");

    mcp_settings_edit(&path, |servers| {
        servers.remove("drop");
        Ok(())
    })
    .expect("remove persists");

    let text = fs::read_to_string(&path).expect("file readable");
    assert!(
        !text.contains("[mcp.servers.drop]"),
        "removed entry must vanish: {text}"
    );
    assert!(
        text.contains("[mcp.servers.keep]"),
        "siblings survive removal: {text}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Regression for the toggle-persist corruption: writing just
/// `enabled = false` to a higher-precedence tier used to default
/// the missing `transport` to `Stdio` during merge, silently
/// downgrading an inherited HTTP/SSE server. The toggle path now
/// serializes the **full** server config; this test pins the
/// resulting table so a future refactor cannot drop `transport`,
/// `url`, `bearer_token_env_var`, etc.
#[test]
fn mcp_server_table_preserves_http_identity() {
    let mut env = BTreeMap::new();
    env.insert("DOCS_API_KEY".to_string(), "secret-ref".to_string());
    let mut headers = BTreeMap::new();
    headers.insert("X-Origin".to_string(), "squeezy".to_string());
    let server = squeezy_core::McpServerConfig {
        enabled: false,
        transport: squeezy_core::McpTransport::Http,
        command: None,
        args: Vec::new(),
        url: Some("https://docs.example/mcp".to_string()),
        timeout_ms: Some(7_500),
        discovery_timeout_ms: None,
        tool_call_timeout_ms: None,
        enabled_tools: None,
        disabled_tools: Vec::new(),
        env,
        permissions: squeezy_core::McpPermissionConfig::default(),
        bearer_token_env_var: Some("DOCS_BEARER".to_string()),
        http_headers: headers,
        env_http_headers: BTreeMap::new(),
    };

    let table = mcp_server_table(&server);

    // Critical invariant: the written table includes `transport =
    // "http"` so the merge layer cannot replace an inherited HTTP
    // server's transport with the default `Stdio`.
    let transport = table
        .get("transport")
        .and_then(|v| v.as_value())
        .and_then(|v| v.as_str());
    assert_eq!(transport, Some("http"));
    let enabled = table
        .get("enabled")
        .and_then(|v| v.as_value())
        .and_then(|v| v.as_bool());
    assert_eq!(enabled, Some(false));
    let url = table
        .get("url")
        .and_then(|v| v.as_value())
        .and_then(|v| v.as_str());
    assert_eq!(url, Some("https://docs.example/mcp"));
    // `command` is None on HTTP servers — the serializer must NOT
    // synthesize a placeholder, otherwise `[mcp.servers.docs]` would
    // claim to be stdio after a toggle.
    assert!(table.get("command").is_none());
    let bearer = table
        .get("bearer_token_env_var")
        .and_then(|v| v.as_value())
        .and_then(|v| v.as_str());
    assert_eq!(bearer, Some("DOCS_BEARER"));
}

/// Drives the editor against three independent tier files to mirror
/// the cross-tier removal behaviour in `persist_mcp_remove`: a
/// remove request must drop the entry from every tier that defines
/// it, otherwise an inherited definition resurrects the server at
/// the next reload. We invoke the public `mcp_settings_edit` once
/// per tier with the same `servers.remove(name)` body the real
/// helper uses.
#[test]
fn cross_tier_remove_drops_entry_from_every_tier_file_that_defines_it() {
    let dir = unique_temp_dir("cross-tier-remove");
    let user = dir.join("user.toml");
    let project = dir.join("project.toml");
    let local = dir.join("local.toml");
    fs::write(
        &user,
        "[mcp.servers.docs]\nenabled = true\ntransport = \"http\"\nurl = \"https://docs.example/mcp\"\n",
    )
    .expect("seed user");
    fs::write(
        &project,
        "[mcp.servers.docs]\nenabled = false\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\n",
    )
    .expect("seed project");
    // local intentionally does NOT define `docs`; the editor must
    // skip non-defining tiers and not waste a write.
    fs::write(
        &local,
        "[mcp.servers.other]\nenabled = true\ncommand = \"x\"\n",
    )
    .expect("seed local");

    let mut touched: Vec<PathBuf> = Vec::new();
    for path in [&user, &project, &local] {
        let mut removed = false;
        mcp_settings_edit(path, |servers| {
            removed = servers.remove("docs").is_some();
            Ok(())
        })
        .expect("edit succeeds");
        if removed {
            touched.push(path.clone());
        }
    }

    assert_eq!(touched, vec![user.clone(), project.clone()]);
    assert!(
        !fs::read_to_string(&user)
            .unwrap()
            .contains("[mcp.servers.docs]")
    );
    assert!(
        !fs::read_to_string(&project)
            .unwrap()
            .contains("[mcp.servers.docs]")
    );
    // `local.toml`'s unrelated entry must survive the no-op pass.
    assert!(
        fs::read_to_string(&local)
            .unwrap()
            .contains("[mcp.servers.other]")
    );
    let _ = fs::remove_dir_all(&dir);
}
