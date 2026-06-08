//! Serialize a [`CaptureLog`] to the JSON the out-of-process xterm.js oracle
//! (`tools/termsim/xtermcheck/replay.js`) consumes.
//!
//! The oracle's contract (documented in `replay.js`) is:
//!
//! ```json
//! {
//!   "bytes_hex": "1b5b...",            // OR "bytes_base64"
//!   "frames": [ { "byte_offset": N, "w": COLS, "h": ROWS }, ... ]
//! }
//! ```
//!
//! We emit `bytes_hex` so this leg needs no base64 dependency — the oracle
//! accepts either encoding. Frame *i*'s bytes are
//! `bytes[frames[i-1].byte_offset .. frames[i].byte_offset]`, matching the
//! self-slicing contract on both sides.

use std::io;
use std::path::Path;

use super::types::CaptureLog;

/// Lower-case hex-encode `bytes` with no separators (the encoding
/// `replay.js`'s `bytes_hex` branch expects).
fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    out
}

/// Serialize a [`CaptureLog`] to the xtermcheck JSON string (hex byte
/// encoding).
pub(crate) fn capture_log_to_json(log: &CaptureLog) -> String {
    let frames: Vec<serde_json::Value> = log
        .frames
        .iter()
        .map(|f| {
            serde_json::json!({
                "byte_offset": f.byte_offset,
                "w": f.w,
                "h": f.h,
            })
        })
        .collect();
    let value = serde_json::json!({
        "bytes_hex": to_hex(&log.bytes),
        "frames": frames,
    });
    // Pretty-print so an exported fixture is diffable / human-readable; the
    // oracle parses either form.
    serde_json::to_string_pretty(&value).expect("CaptureLog JSON serializes")
}

/// Write a [`CaptureLog`] as xtermcheck JSON to `path`.
pub(crate) fn export_capture_log(log: &CaptureLog, path: &Path) -> io::Result<()> {
    std::fs::write(path, capture_log_to_json(log))
}

#[cfg(test)]
#[path = "export_tests.rs"]
mod tests;
