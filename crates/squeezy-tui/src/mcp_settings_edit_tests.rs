//! Unit tests for the `[mcp.servers]` TOML editor invoked by the
//! `/mcp` config page when persisting toggle/add/remove actions
//! (`crates/squeezy-tui/src/mcp_settings_edit.rs`). The tests drive
//! the editor directly against temp-file inputs so they cover both
//! fresh and existing settings files without standing up a
//! `ConfigScreenState`.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::mcp_settings_edit;

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
