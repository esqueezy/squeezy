//! Per-tool filesystem (and, later, subprocess / network) indirection.
//!
//! Today most tools reach into [`std::fs`] / [`reqwest`] directly. That is
//! convenient but it makes two desirable layers awkward:
//!
//! 1. **Test harnesses** that want to substitute a deterministic in-memory
//!    filesystem so a tool unit test does not have to scribble temp files.
//! 2. **Future sandbox layers** that need to wrap every real syscall with
//!    pre/post hooks (path canonicalisation, permission audit, redaction)
//!    without forking each tool's implementation.
//!
//! [`ToolIo`] is a small indirection that gives both layers a single seam.
//! The first revision keeps the surface intentionally minimal — just the
//! file-read primitives required by `read_file`. Each follow-up tool
//! migration may add another method (e.g. `walk_dir`, `spawn`, `http_get`),
//! and the existing [`WebHttpClient`](crate::web::WebHttpClient) seam will
//! eventually fold into this trait as a single `ToolIo` surface.
//!
//! See `docs/internal/TOOL_IO.md` for the migration plan and the per-tool
//! status table.

use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use sha2::{Digest, Sha256};

/// Pluggable filesystem / IO surface used by tools.
///
/// Implementations must be cheap to clone behind an [`std::sync::Arc`] (they
/// are stored as `Arc<dyn ToolIo>` on [`crate::ToolRegistry`]) and safe to
/// share across threads — tool execution runs on the tokio runtime and many
/// tool calls can be in flight simultaneously.
///
/// The trait is deliberately small. Adding a method here implies wiring
/// every existing tool through it; new additions should land as a separate
/// migration step that updates each tool's call sites in lockstep. The
/// current surface intentionally returns plain values (`u64`, `Vec<u8>`)
/// rather than `std::fs::Metadata` / `std::fs::File`, so mock
/// implementations can be written without invoking `std::fs` at all.
pub trait ToolIo: Send + Sync + std::fmt::Debug {
    /// Return the file size in bytes.
    fn file_len(&self, path: &Path) -> std::io::Result<u64>;

    /// Read the full contents of `path` into memory.
    ///
    /// Prefer [`Self::read_range`] when the caller only needs a bounded
    /// slice; this method exists for callers that have already validated
    /// the file size.
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>>;

    /// Read at most `limit` bytes starting at `offset`. The returned
    /// buffer is shorter than `limit` if the file ends inside the
    /// requested window.
    fn read_range(&self, path: &Path, offset: u64, limit: usize) -> std::io::Result<Vec<u8>>;

    /// Replace the contents of `path` with `bytes`. Parent directories
    /// must already exist; callers requiring `mkdir -p` semantics should
    /// invoke [`std::fs::create_dir_all`] themselves until that primitive
    /// lands on the trait.
    fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()>;

    /// Compute the SHA-256 digest of `path` as a lower-case hex string.
    ///
    /// The default implementation streams through [`Self::read_range`]
    /// in 64 KiB chunks so a custom `ToolIo` (e.g. an in-memory fake)
    /// works without overriding this method. Real-filesystem backends
    /// can override for a single-open streaming hash.
    fn sha256(&self, path: &Path) -> std::io::Result<String> {
        const CHUNK: usize = 64 * 1024;
        let mut hasher = Sha256::new();
        let mut offset: u64 = 0;
        loop {
            let chunk = self.read_range(path, offset, CHUNK)?;
            if chunk.is_empty() {
                break;
            }
            let len = chunk.len();
            hasher.update(&chunk);
            offset = offset.saturating_add(len as u64);
            if len < CHUNK {
                break;
            }
        }
        Ok(hex_digest(hasher))
    }
}

/// Default [`ToolIo`] backed by [`std::fs`].
///
/// This is the implementation the registry ships with when callers do
/// not provide their own. It preserves the exact behaviour of the
/// pre-existing free helpers (`read_prefix`, `read_range`, `sha256_file`,
/// `file_len`) so no observable behaviour changes for production callers.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultToolIo;

impl ToolIo for DefaultToolIo {
    fn file_len(&self, path: &Path) -> std::io::Result<u64> {
        Ok(fs::metadata(path)?.len())
    }

    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn read_range(&self, path: &Path, offset: u64, limit: usize) -> std::io::Result<Vec<u8>> {
        let mut file = File::open(path)?;
        if offset > 0 {
            file.seek(SeekFrom::Start(offset))?;
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(limit as u64)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        fs::write(path, bytes)
    }

    /// Override the trait default with a single-open streaming hash so a
    /// 100 MiB file does not re-open the descriptor per 64 KiB chunk.
    fn sha256(&self, path: &Path) -> std::io::Result<String> {
        let mut file = File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        Ok(hex_digest(hasher))
    }
}

fn hex_digest(hasher: Sha256) -> String {
    let digest = hasher.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}
