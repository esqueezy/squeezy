//! Reusable cache-hint placement helpers.
//!
//! Anthropic-family wire formats (native Anthropic Messages, OpenAI-compatible
//! aggregators that proxy Anthropic, and Bedrock Converse) all accept inline
//! breakpoint markers that tell the server where to cache the prefix. The
//! exact serialization differs per protocol — Anthropic and aggregator routes
//! attach a JSON `cache_control: { type: "ephemeral" }` object; Bedrock's
//! Converse API uses typed `CachePoint` content blocks built through the AWS
//! SDK. The *decision* of where to place those breakpoints (tools tail,
//! system tail, last user message) is identical across all three.
//!
//! This module centralizes that decision so each protocol adapter only has to
//! emit the protocol-specific marker — not re-derive the strategy. The
//! Anthropic Messages and OpenAI-compatible adapters operate on
//! `serde_json::Value` and can use [`anthropic_messages::mark_last_user_block`]
//! and the matching `system` / `tool` helpers verbatim. The Bedrock adapter
//! builds typed AWS SDK structures and relies on
//! [`should_apply_caching`] for the cross-protocol gating decision while
//! keeping the typed insertion local.

use serde_json::{Value, json};

use crate::{LlmRequest, capabilities_for};

#[cfg(test)]
#[path = "cache_policy_tests.rs"]
mod tests;

/// Where in the message list to anchor the trailing breakpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageStrategy {
    /// Mark the most recent user-role message (Anthropic recommended).
    LatestUserMessage,
}

/// Auto-placement policy: mark tools, system, and the latest user message.
///
/// Mirrors OpenCode's `AUTO = { tools: true, system: true, messages: "latest-user-message" }`
/// (`packages/llm/src/cache-policy.ts:18-22`). Squeezy currently exposes only
/// this default; per-skill or per-session overrides plug into the same struct
/// when needed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CachePolicy {
    pub tools: bool,
    pub system: bool,
    pub messages: MessageStrategy,
}

impl CachePolicy {
    /// The single default policy used by every Anthropic-family adapter.
    pub(crate) const AUTO: Self = Self {
        tools: true,
        system: true,
        messages: MessageStrategy::LatestUserMessage,
    };
}

/// Decide whether the destination model accepts inline cache breakpoints.
///
/// Returns `true` only when the caller supplied a cache_key *and* the model
/// registry reports `prompt_caching` for `(provider, model)`. The cache_key
/// gate lets agent code disable caching on short conversations (where cache
/// writes cost more than reads); the registry gate keeps us from sending
/// markers to models that would 400.
pub(crate) fn should_apply_caching(provider: &str, request: &LlmRequest) -> bool {
    request.cache_key.is_some()
        && capabilities_for(provider, &request.model)
            .is_some_and(|capabilities| capabilities.prompt_caching)
}

/// Ephemeral `cache_control` literal shared across Anthropic Messages and
/// OpenAI-compatible aggregator wire formats.
pub(crate) fn ephemeral_marker() -> Value {
    json!({ "type": "ephemeral" })
}

/// Tool-name prefix the agent reserves for dynamically advertised MCP
/// tools. The tool registry pushes any tool whose name starts with this
/// to the *end* of the advertised list, so the cache breakpoint must
/// land before them — otherwise an MCP `tools/list` refresh that
/// reorders or replaces dynamic tools would invalidate the cached
/// prompt prefix on every turn.
pub(crate) const DYNAMIC_TOOL_NAME_PREFIX: &str = "mcp__";

/// Pick the index of the last *stable* (non-mcp__-prefixed) tool to
/// anchor the cache breakpoint on. Falls back to the literal last
/// index when every advertised tool is dynamic so callers still place a
/// breakpoint somewhere when caching is enabled. Returns `None` only on
/// an empty iterator.
///
/// Centralizing this decision means the Anthropic JSON path, the
/// OpenAI-compatible aggregator path (Anthropic-flavoured), and any
/// future protocol adapter all agree on which tool entry receives the
/// marker. Each adapter still owns the marker insertion in its own
/// wire shape (`cache_control` on the chosen JSON object, or a typed
/// `CachePoint` block for Bedrock).
pub(crate) fn last_stable_tool_index<'a, I>(names: I) -> Option<usize>
where
    I: IntoIterator<Item = &'a str>,
    I::IntoIter: DoubleEndedIterator + ExactSizeIterator,
{
    let iter = names.into_iter();
    let len = iter.len();
    if len == 0 {
        return None;
    }
    let stable = iter
        .enumerate()
        .rev()
        .find_map(|(idx, name)| (!name.starts_with(DYNAMIC_TOOL_NAME_PREFIX)).then_some(idx));
    Some(stable.unwrap_or(len - 1))
}

/// Helpers that operate on the Anthropic Messages JSON wire format. The
/// OpenAI-compatible aggregator path (when the destination is an Anthropic
/// model) re-uses the same content shape, so it shares these helpers.
pub(crate) mod json_markers {
    use serde_json::{Value, json};

    /// Wrap a plain `system` string in the array form Anthropic Messages
    /// requires for inline cache markers.
    pub(crate) fn system_array_with_marker(instructions: &str) -> Value {
        json!([{
            "type": "text",
            "text": instructions,
            "cache_control": super::ephemeral_marker(),
        }])
    }

    /// Mark the last block of the most recent user-role message. No-op when
    /// no user message exists.
    pub(crate) fn mark_last_user_block(messages: &mut [Value]) {
        for message in messages.iter_mut().rev() {
            if message.get("role").and_then(Value::as_str) != Some("user") {
                continue;
            }
            if let Some(block) = message
                .get_mut("content")
                .and_then(Value::as_array_mut)
                .and_then(|content| content.last_mut())
                .and_then(Value::as_object_mut)
            {
                block.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
            }
            return;
        }
    }

    /// Attach the cache breakpoint to the last *stable* tool definition in
    /// the Anthropic Messages tool shape (`{"name": ..., ...}`). Reads
    /// the `name` field directly off each JSON value and delegates the
    /// breakpoint-index decision to [`super::last_stable_tool_index`].
    /// No-op on an empty slice.
    pub(crate) fn mark_last_stable_tool(tool_values: &mut [Value]) {
        let Some(idx) = super::last_stable_tool_index(
            tool_values
                .iter()
                .map(|tool| tool.get("name").and_then(Value::as_str).unwrap_or("")),
        ) else {
            return;
        };
        if let Some(obj) = tool_values.get_mut(idx).and_then(Value::as_object_mut) {
            obj.insert("cache_control".to_string(), super::ephemeral_marker());
        }
    }
}
