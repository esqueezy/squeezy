//! In-place TOML editor for the `[mcp.servers]` block.
//!
//! Called by the `/mcp` config page when persisting toggle / add /
//! remove actions. The function is pure (input + edit closure → file
//! on disk) so unit tests can drive it without standing up a
//! `ConfigScreenState`, and the call sites in `lib.rs` stay focused on
//! the host/agent plumbing. Empty / missing files are handled by
//! starting from an empty document, and missing parent directories
//! are created so the first edit at the Repo or Local tier never
//! fails on `ENOENT`.

use std::path::Path;

/// Run an in-place edit of `[mcp.servers]` in the TOML file at
/// `path`. Creates parent directories and the file if missing so
/// adding the first server at the Repo or Local tier "just works".
pub(crate) fn mcp_settings_edit(
    path: &Path,
    edit: impl FnOnce(&mut toml_edit::Table) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    let mut doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
    let mcp = doc
        .as_table_mut()
        .entry("mcp")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "[mcp] is not a table".to_string(),
            )
        })?;
    let servers = mcp
        .entry("servers")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "[mcp.servers] is not a table".to_string(),
            )
        })?;
    edit(servers)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, doc.to_string())
}

#[cfg(test)]
#[path = "mcp_settings_edit_tests.rs"]
mod tests;
