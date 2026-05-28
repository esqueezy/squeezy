use std::{pin::Pin, sync::Arc};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use futures_core::Stream;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
pub use squeezy_core::{
    AnthropicThinkingBlock, AnthropicThinkingKind, ReasoningKind, ReasoningPayload,
    ReasoningSnapshot, resolve_model_alias,
};
use squeezy_core::{CostSnapshot, ReasoningEffort, ResponseVerbosity, Result, SqueezyError};
use tokio_util::sync::CancellationToken;

pub const INVALID_TOOL_ARGUMENTS_KEY: &str = "__squeezy_invalid_tool_arguments";
pub const INVALID_TOOL_ARGUMENTS_ERROR_KEY: &str = "__squeezy_parse_error";
pub const INVALID_TOOL_ARGUMENTS_RAW_KEY: &str = "__squeezy_raw_arguments";

mod anthropic;
mod anthropic_betas;
mod bedrock;
mod cache_policy;
mod compatible;
mod credentials;
mod google;
mod lmstudio;
pub mod model_discovery;
pub mod models_dev;
pub mod oauth;
mod ollama;
mod openai;
mod registry;
mod retry;
mod sse;
pub mod tokens;
mod xai;
pub use tokens::{
    DEFAULT_BYTES_PER_TOKEN, DEFAULT_EMA_ALPHA, ProviderCalibration, TokenCalibration,
    default_bytes_per_token, estimate_tokens,
};

pub use anthropic::AnthropicProvider;
pub use bedrock::BedrockProvider;
pub use compatible::OpenAiCompatibleProvider;
pub use credentials::{
    ApiKeyFuture, ApiKeySource, KeySource, RefreshableToken, ResolvedKey, StaticApiKey, TokenState,
    delete_api_key, resolve_api_key, resolve_api_key_with_inline, static_api_key_source,
};
pub use google::GoogleProvider;
pub use lmstudio::{
    DEFAULT_LMSTUDIO_BASE_URL, LMStudioConfig, LMStudioProvider, fetch_lmstudio_model_names,
};
pub use model_discovery::{
    CONSERVATIVE_FALLBACK_CAPABILITIES, CapabilitySource, ResolvedCapabilities,
    resolve_capabilities, resolve_capabilities_with,
};
pub use oauth::{
    ANTHROPIC_OAUTH_TOKEN_PREFIX, AnthropicLoginConfig, AnthropicOAuthSource,
    OPENAI_CODEX_AUTH_FILE_NAME, OpenAiCodexLoginOutcome, OpenAiCodexOAuthSource,
    OpenAiCodexProvider, PersistedTokens, PkceCodes, TokenResponse,
    anthropic_default_storage_path as oauth_anthropic_default_storage_path,
    anthropic_oauth_beta_header, anthropic_read_tokens as oauth_anthropic_read_tokens,
    anthropic_write_tokens as oauth_anthropic_write_tokens, codex_auth_file_path,
    default_codex_auth_path, exchange_authorization_code, generate_pkce, is_anthropic_oauth_token,
    load_codex_token, login_openai_codex_interactive, parse_authorization_input,
    refresh_anthropic_token, save_codex_token,
};
pub use ollama::{
    OllamaProvider, PullEvent, PullStream, fetch_ollama_context_window, fetch_ollama_model_names,
    pull_model,
};
pub use openai::OpenAiProvider;
pub use registry::{
    MODEL_REGISTRY, ModelCapabilities, ModelInfo, ModelLifecycle, ModelLimits, PROVIDERS,
    RequestTokenEstimate, TokenPricing, TokenizerKind, capabilities_for, estimate_cost,
    estimate_request_context, estimate_request_context_calibrated, model_info_for,
    models_for_provider, provider_from_config, provider_name,
};
pub use xai::XaiProvider;

pub type LlmStream = Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmRequest {
    pub model: Arc<str>,
    pub instructions: Arc<str>,
    pub input: Arc<[LlmInputItem]>,
    pub max_output_tokens: Option<u32>,
    pub response_verbosity: Option<ResponseVerbosity>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub previous_response_id: Option<String>,
    pub cache_key: Option<String>,
    pub tools: Arc<[Arc<LlmToolSpec>]>,
    pub store: bool,
    /// Optional `tool_choice` hint to forward to the provider when tools are
    /// advertised. `None` omits the field entirely — matches squeezy's
    /// historical behavior and lets the provider apply its default
    /// (typically `auto`). Set to `"required"` for tool-shy models like
    /// Qwen via OpenRouter that otherwise emit a chatty preamble and
    /// finish with `stop` without calling any tool. Mirrors opencode's
    /// `lowerToolChoice` pass-through (`openai-chat.ts:172, 267`) and
    /// clear-code's `options.toolChoice` (`claude.ts:1712`).
    pub tool_choice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<LlmOutputSchema>,
    /// When `Some(false)`, force the OpenAI Responses API to issue tool
    /// calls serially. `None` leaves the OpenAI default (parallel) in
    /// place. Only the OpenAI provider currently reads this; other
    /// providers ignore it.
    pub parallel_tool_calls: Option<bool>,
    /// Anthropic beta opt-ins (e.g. `context-1m-2025-08-07`,
    /// `interleaved-thinking-2025-05-14`). Empty by default. The
    /// Anthropic provider joins these into an `anthropic-beta` HTTP
    /// header; the Bedrock provider partitions them and forwards only
    /// the body-param-eligible subset via
    /// `additional_model_request_fields.anthropic_beta`. Mirrors
    /// clear-code's per-provider routing (`constants/betas.ts` +
    /// `claude.ts:272-331`). Other providers ignore the field.
    #[serde(default = "empty_beta_headers")]
    pub beta_headers: Arc<[Arc<str>]>,
}

fn empty_beta_headers() -> Arc<[Arc<str>]> {
    Arc::from(Vec::new())
}

impl LlmRequest {
    pub fn user_text(
        model: String,
        instructions: String,
        input: String,
        max_output_tokens: Option<u32>,
    ) -> Self {
        Self {
            model: Arc::from(model),
            instructions: Arc::from(instructions),
            input: Arc::from(vec![LlmInputItem::UserText(input)]),
            max_output_tokens,
            response_verbosity: None,
            reasoning_effort: None,
            previous_response_id: None,
            cache_key: None,
            tools: Arc::from(Vec::new()),
            store: false,
            tool_choice: None,
            output_schema: None,
            parallel_tool_calls: None,
            beta_headers: empty_beta_headers(),
        }
    }
}

/// Strict JSON Schema response contract carried on `LlmRequest::output_schema`.
///
/// Providers that support structured outputs (OpenAI Responses
/// `text.format = { type: "json_schema", ... }`) attach this to the request
/// body; others ignore it. `strict` mirrors OpenAI's "the model MUST emit
/// JSON that validates" flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmOutputSchema {
    pub name: String,
    pub schema: Value,
    pub strict: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum LlmInputItem {
    UserText(String),
    AssistantText(String),
    FunctionCall {
        call_id: String,
        name: String,
        arguments: Value,
    },
    FunctionCallOutput {
        call_id: String,
        output: String,
    },
    Reasoning(ReasoningPayload),
    /// Inline image attached to a user turn. `media_type` is an
    /// `image/{png,jpeg,gif,webp}` MIME string; `bytes` carries the raw
    /// image payload (each provider's `request_body` re-encodes as
    /// needed — base64 data URL, `inlineData`, Bedrock `Blob`, etc.).
    /// Stored serialized as a base64 string so checkpoints stay JSON-
    /// safe without bloating to a byte array.
    Image {
        media_type: String,
        #[serde(serialize_with = "serialize_image_bytes_b64")]
        #[serde(deserialize_with = "deserialize_image_bytes_b64")]
        bytes: Arc<[u8]>,
    },
}

impl LlmInputItem {
    /// Construct an `Image` item from a media-type string and raw bytes.
    /// Convenience to keep call sites short.
    pub fn image(media_type: impl Into<String>, bytes: impl Into<Arc<[u8]>>) -> Self {
        Self::Image {
            media_type: media_type.into(),
            bytes: bytes.into(),
        }
    }

    /// `true` for the `Image` variant. Used by the per-provider request
    /// builders and the vision-capability check below.
    pub fn is_image(&self) -> bool {
        matches!(self, Self::Image { .. })
    }
}

fn serialize_image_bytes_b64<S: Serializer>(
    bytes: &Arc<[u8]>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_str(&BASE64_STANDARD.encode(bytes.as_ref()))
}

fn deserialize_image_bytes_b64<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Arc<[u8]>, D::Error> {
    use serde::de::Error;
    let encoded: String = String::deserialize(deserializer)?;
    let bytes = BASE64_STANDARD
        .decode(encoded.as_bytes())
        .map_err(|err| Error::custom(format!("invalid base64 image payload: {err}")))?;
    Ok(Arc::from(bytes.into_boxed_slice()))
}

/// Detect the canonical image MIME type from a byte prefix using magic
/// numbers. Supports PNG, JPEG, GIF (87a/89a), and WEBP (RIFF / WEBP
/// container). Returns `None` when the prefix does not match a known
/// image format. The exhaustive variant list matches what the upstream
/// providers (Anthropic / OpenAI / Google / Bedrock) accept for inline
/// image content blocks; everything else has to round-trip as text.
pub fn infer_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

impl LlmRequest {
    /// Refuse to ship a request that carries `LlmInputItem::Image`
    /// payloads when the destination model's
    /// [`crate::ModelCapabilities::vision`] flag is false. Each provider's
    /// `stream_response` calls this before building the wire body so the
    /// caller sees a structured error (`SqueezyError::ProviderRequest`)
    /// instead of an upstream-rejected 4xx with a vendor-specific
    /// message. Models that are unknown to the registry (custom presets,
    /// fresh aggregator SKUs) fall back to the conservative
    /// `vision: false` default and surface the same error — callers can
    /// extend `models.json` or attach `model_discovery::ResolvedCapabilities`
    /// to opt in.
    pub fn ensure_vision_support(&self, provider: &str) -> Result<()> {
        if !self.input.iter().any(LlmInputItem::is_image) {
            return Ok(());
        }
        let supports_vision =
            crate::capabilities_for(provider, &self.model).is_some_and(|caps| caps.vision);
        if supports_vision {
            return Ok(());
        }
        Err(SqueezyError::ProviderRequest(format!(
            "model `{model}` on provider `{provider}` does not support image inputs (capabilities.vision = false); pick a vision-capable model before attaching an image",
            model = self.model,
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub strict: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

/// Normalized completion cause. Each provider maps its native `stop_reason`
/// (Anthropic), `finish_reason`/`incomplete_details.reason` (OpenAI),
/// `finishReason` (Google), Bedrock `stopReason`, or Ollama `done_reason`
/// into one of these variants so the agent can branch on a single shape.
///
/// `EndTurn` is the model voluntarily releasing the turn; `ToolUse` means
/// the model wants to invoke tools; `MaxTokens` and `ContextWindowExceeded`
/// are truncation signals the agent surfaces explicitly so the user (and
/// future compaction-retry logic) can act on them instead of seeing a bare
/// provider error; `StopSequence` and `Refusal` carry the remaining
/// semantically distinct cases; `Other` keeps provider-specific strings
/// reachable without forcing the registry to enumerate every value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    ContextWindowExceeded,
    StopSequence,
    Refusal,
    Other(String),
}

impl StopReason {
    /// Parse Anthropic Messages API `stop_reason` strings.
    pub fn from_anthropic(value: &str) -> Self {
        match value {
            "end_turn" => Self::EndTurn,
            "tool_use" => Self::ToolUse,
            "max_tokens" => Self::MaxTokens,
            "model_context_window_exceeded" => Self::ContextWindowExceeded,
            "stop_sequence" => Self::StopSequence,
            "refusal" => Self::Refusal,
            other => Self::Other(other.to_string()),
        }
    }

    /// Parse OpenAI Responses API `incomplete_details.reason` strings.
    pub fn from_openai_incomplete(value: &str) -> Self {
        match value {
            "max_output_tokens" => Self::MaxTokens,
            "content_filter" => Self::Refusal,
            other => Self::Other(other.to_string()),
        }
    }

    /// Parse Google `candidates[0].finishReason` strings.
    pub fn from_google(value: &str) -> Self {
        match value {
            "STOP" => Self::EndTurn,
            "MAX_TOKENS" => Self::MaxTokens,
            "SAFETY" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY"
            | "LANGUAGE" | "RECITATION" => Self::Refusal,
            other => Self::Other(other.to_string()),
        }
    }

    /// Parse Bedrock Converse `stopReason` strings.
    pub fn from_bedrock(value: &str) -> Self {
        match value {
            "end_turn" => Self::EndTurn,
            "tool_use" => Self::ToolUse,
            "max_tokens" => Self::MaxTokens,
            "model_context_window_exceeded" => Self::ContextWindowExceeded,
            "stop_sequence" => Self::StopSequence,
            "guardrail_intervened" | "content_filtered" => Self::Refusal,
            other => Self::Other(other.to_string()),
        }
    }

    /// Parse Ollama `done_reason` strings.
    pub fn from_ollama(value: &str) -> Self {
        match value {
            "stop" => Self::EndTurn,
            "length" => Self::MaxTokens,
            other => Self::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum LlmEvent {
    Started,
    TextDelta(String),
    ReasoningDelta {
        text: String,
        kind: ReasoningKind,
    },
    ReasoningDone(ReasoningPayload),
    ToolCall(LlmToolCall),
    Completed {
        response_id: Option<String>,
        cost: CostSnapshot,
        /// Normalized completion cause. `None` when the provider stream
        /// closed without emitting one (e.g. transport truncation handled
        /// elsewhere). Producers that have a native value MUST populate
        /// this; the agent uses it to drive explicit recovery branches.
        stop_reason: Option<StopReason>,
        /// `true` iff the stream finished with `stop_reason=EndTurn`,
        /// no content or tool-call delta latched
        /// `state.saw_visible_output`, AND the reasoning buffer was
        /// non-empty.
        ///
        /// This is the canonical Qwen3 / DeepSeek-R1 "reasoning-only
        /// finish" pattern — model thinks, model stops, no actionable
        /// output. Agent loop consumers may retry the turn once when
        /// this flag is set. Separate from `stop_reason` because the
        /// normalized `EndTurn` variant alone can't distinguish a clean
        /// "model emitted a real answer and stopped" from a degenerate
        /// "model spent the round on reasoning and stopped with
        /// nothing visible".
        #[serde(default)]
        reasoning_only_stop: bool,
    },
    Cancelled,
}

impl LlmEvent {
    /// Construct a `Completed` event with no provider-reported stop
    /// reason and no reasoning-only-stop marker. Convenience for test
    /// code and synthetic completions (replay reconstruction, helper
    /// turn paths) that don't carry a real upstream signal.
    pub fn completed(response_id: Option<String>, cost: CostSnapshot) -> Self {
        LlmEvent::Completed {
            response_id,
            cost,
            stop_reason: None,
            reasoning_only_stop: false,
        }
    }

    /// Construct a `Completed` event with explicit normalized
    /// `stop_reason` and `reasoning_only_stop` markers. Used by the
    /// Chat-Completions provider when the upstream surfaces a real
    /// terminal reason AND we want the reasoning-only-stop signal
    /// latched.
    pub fn completed_with_reason(
        response_id: Option<String>,
        cost: CostSnapshot,
        stop_reason: Option<StopReason>,
        reasoning_only_stop: bool,
    ) -> Self {
        LlmEvent::Completed {
            response_id,
            cost,
            stop_reason,
            reasoning_only_stop,
        }
    }
}

pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn stream_response(&self, request: LlmRequest, cancel: CancellationToken) -> LlmStream;
}

#[derive(Debug, Clone)]
pub struct UnavailableProvider {
    name: &'static str,
    reason: Arc<str>,
}

impl UnavailableProvider {
    pub fn new(name: &'static str, reason: impl Into<String>) -> Self {
        Self {
            name,
            reason: Arc::from(reason.into()),
        }
    }
}

impl LlmProvider for UnavailableProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    fn stream_response(&self, _request: LlmRequest, _cancel: CancellationToken) -> LlmStream {
        let reason = self.reason.clone();
        Box::pin(futures_util::stream::once(async move {
            Err(SqueezyError::ProviderNotConfigured(reason.to_string()))
        }))
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
