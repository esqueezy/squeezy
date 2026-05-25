use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::driver::EvalError;

/// One "frame" per completed (or terminated) agent turn. This is the
/// human-friendly view of what a TUI user would have seen: the assembled
/// assistant text, plus the tool calls fired and any error/finish reason.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FrameRecord {
    pub turn_id: String,
    pub prompt: String,
    /// Concatenation of all assistant text deltas for this turn, in order.
    pub assistant_text: String,
    pub tool_calls: Vec<ToolCallSummary>,
    pub tool_errors: Vec<String>,
    pub elapsed_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Estimated turn cost in USD microdollars (1_000_000 = $1.00). Computed
    /// from the model's pricing entry via `squeezy_llm::estimate_cost`. Zero
    /// when no pricing data is available for the model.
    #[serde(default)]
    pub cost_micro_usd: u64,
    /// Human-readable rendering of `cost_micro_usd`, e.g. `"$0.0123"`.
    #[serde(default)]
    pub cost_display: String,
    pub finish: FrameFinish,
}

/// Per-tool-call breadcrumb stored on the frame so a reviewer can spot
/// duplicate or unexpected calls without reaching into `trace.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallSummary {
    pub name: String,
    /// First ~200 chars of the JSON-encoded arguments. Designed for
    /// human eyeballing, not parsing.
    pub args_preview: String,
    /// Hex SHA-256 of the full canonical-JSON arguments. Stable
    /// identifier used by the auto-findings rules to detect duplicate
    /// calls within a turn.
    pub args_sha256: String,
    /// Tool status when known (`success`, `error`, `cancelled`, ...).
    #[serde(default)]
    pub status: Option<String>,
}

impl ToolCallSummary {
    pub fn from_call(name: &str, arguments: &Value) -> Self {
        let serialized = serde_json::to_string(arguments).unwrap_or_else(|_| "null".into());
        let mut hasher = Sha256::new();
        hasher.update(serialized.as_bytes());
        let digest = hasher.finalize();
        let args_sha256 = digest.iter().fold(String::with_capacity(64), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        });
        let args_preview: String = serialized.chars().take(200).collect();
        Self {
            name: name.to_string(),
            args_preview,
            args_sha256,
            status: None,
        }
    }
}

pub fn format_cost_micro_usd(micro: u64) -> String {
    let dollars = micro as f64 / 1_000_000.0;
    format!("${dollars:.4}")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameFinish {
    #[default]
    Completed,
    Cancelled,
    Failed,
    NoTurn,
}

pub struct FrameWriter {
    inner: Mutex<FrameInner>,
}

struct FrameInner {
    path: PathBuf,
    file: std::fs::File,
}

impl FrameWriter {
    pub fn create(dir: &Path) -> Result<Self, EvalError> {
        std::fs::create_dir_all(dir)
            .map_err(|err| EvalError::Io(format!("create_dir_all {dir:?}: {err}")))?;
        let path = dir.join("frames.jsonl");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|err| EvalError::Io(format!("open {path:?}: {err}")))?;
        Ok(Self {
            inner: Mutex::new(FrameInner { path, file }),
        })
    }

    pub fn write(&self, frame: &FrameRecord) -> Result<(), EvalError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|err| EvalError::Internal(format!("frame mutex poisoned: {err}")))?;
        let line = serde_json::to_string(frame)
            .map_err(|err| EvalError::Internal(format!("serialize frame: {err}")))?;
        writeln!(guard.file, "{line}")
            .map_err(|err| EvalError::Io(format!("append frame: {err}")))?;
        Ok(())
    }

    pub fn path(&self) -> PathBuf {
        self.inner.lock().expect("frame lock").path.clone()
    }
}
