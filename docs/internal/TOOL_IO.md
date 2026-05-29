# Pluggable Tool IO

`squeezy-tools` ships a `ToolIo` trait so tools can route filesystem (and
later subprocess / network) calls through a single seam instead of
reaching into `std::fs` / `reqwest` directly. The seam lets test harnesses
substitute an in-memory backend without touching real disk, and it gives
future sandbox layers one place to wrap every real syscall with
canonicalisation, audit, and redaction.

## Surface

```rust
pub trait ToolIo: Send + Sync + std::fmt::Debug {
    fn file_len(&self, path: &Path) -> std::io::Result<u64>;
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>>;
    fn read_range(&self, path: &Path, offset: u64, limit: usize) -> std::io::Result<Vec<u8>>;
    fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()>;
    fn sha256(&self, path: &Path) -> std::io::Result<String> { /* default streams via read_range */ }
}
```

The default registry implementation, `DefaultToolIo`, wraps `std::fs`
with the same behaviour the pre-existing free helpers
(`file_len` / `read_prefix` / `read_range` / `sha256_file`) provided, so
production callers see no observable difference. Callers that want to
swap the backend use `ToolRegistry::with_tool_io(Arc<dyn ToolIo>)`.

Trait values were chosen over std types (`u64` instead of
`std::fs::Metadata`, `Vec<u8>` instead of `std::fs::File`) so a mock
implementation can be written end-to-end without invoking `std::fs` at
all. `std::fs::Metadata` cannot be constructed outside of `std`, which
would have forced every mock to back the trait with a real file on disk.

## Migration tracker

| Tool                              | Status     | Notes                                                                |
| --------------------------------- | ---------- | -------------------------------------------------------------------- |
| `read_file` (`file_ops.rs`)       | Migrated   | Proof of concept. All IO (`file_len`, prefix read, range read, sha256) routes through `self.io`. |
| `glob` / `grep` (`file_ops.rs`)   | Pending    | Still call `file_len` / `read_prefix` free helpers and `ignore::WalkBuilder` directly. Needs a `walk_dir` primitive on the trait. |
| `read_slice` / repo map / graph IO (`graph_tools.rs`) | Pending | Several call sites use `sha256_file` / `file_len` / `read_range`. Migrate alongside any rework of `read_slice`. |
| `write_file` / `apply_patch` / `notebook_edit` (`patch.rs`) | Pending | Need `create_dir_all`, atomic-write, and rename on the trait. |
| `shell` (`shell.rs`)              | Pending    | Subprocess spawn is structurally distinct from filesystem IO — likely a sibling trait (`ToolShell`) rather than a method on `ToolIo`. |
| `webfetch` / `websearch` (`web.rs`) | Pending  | Already runs through the `WebHttpClient` trait. Plan is to fold `WebHttpClient` into `ToolIo` (or expose both behind a single `ToolBackend` aggregate) once enough tools have migrated to make the trait shape stable. |
| MCP resource fetch (`ipc.rs`, MCP plumbing) | Pending | Uses the MCP client registry, not `std::fs`; revisit once the FS migration is done. |
| Checkpoint store (`checkpoints.rs`, `checkpoint_provider.rs`) | Pending | Has its own pluggable seam (`CheckpointProvider`) so it does not block the IO trait. |

Adding a method to `ToolIo` should land as a single PR that also
migrates every existing tool that needs it, so the trait never grows
ahead of real usage.

## Why a separate trait instead of generics

`ToolRegistry` is stored behind `Arc` in many places (agent, TUI,
checkpoints, MCP). Threading a generic `Io` type parameter through
every call site would force every owner to become generic too, and
would break the existing `Clone` impl on `ToolRegistry` because each
caller would have to keep a concrete `Io`. A `dyn ToolIo` trait object
behind an `Arc` matches the existing `Arc<dyn WebHttpClient>` pattern in
the same module, keeps the public API stable, and costs one extra
vtable dispatch per call — negligible compared to the actual IO work
the tools perform.
