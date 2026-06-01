//! xAI Grok provider with split routing between the Responses API and Chat
//! Completions.
//!
//! xAI publishes both an OpenAI-Responses-compatible endpoint
//! (`POST /v1/responses`) and a Chat-Completions endpoint
//! (`POST /v1/chat/completions`) on `https://api.x.ai`. Grok 3 and Grok 4
//! expose their richer feature surface (reasoning summaries, encrypted
//! reasoning replay, hosted tools) only through Responses; earlier Grok
//! models (grok-2, grok-beta) predate the Responses launch and answer only
//! the Chat route. Selecting per request keeps both generations working
//! through one provider entry by picking the route based on the
//! requested model id.
//!
//! The provider holds one client per route and dispatches per-request based
//! on [`is_responses_capable`]; per-startup dispatch would lock a session
//! to a single wire even when the user switches Grok generations mid-run.

use squeezy_core::{OpenAiCompatibleConfig, OpenAiCompatiblePreset, Result};
use tokio_util::sync::CancellationToken;

use crate::{LlmProvider, LlmRequest, LlmStream, OpenAiCompatibleProvider, OpenAiProvider};

#[derive(Debug, Clone)]
pub struct XaiProvider {
    responses: OpenAiProvider,
    chat: OpenAiCompatibleProvider,
}

impl XaiProvider {
    pub fn from_config(config: &OpenAiCompatibleConfig) -> Result<Self> {
        debug_assert_eq!(config.preset, OpenAiCompatiblePreset::XAi);
        Ok(Self {
            responses: OpenAiProvider::from_xai_config(config)?,
            chat: OpenAiCompatibleProvider::from_config(config)?,
        })
    }
}

impl LlmProvider for XaiProvider {
    fn name(&self) -> &'static str {
        "xai"
    }

    fn stream_response(&self, request: LlmRequest, cancel: CancellationToken) -> LlmStream {
        if is_responses_capable(&request.model) {
            self.responses.stream_response(request, cancel)
        } else {
            self.chat.stream_response(request, cancel)
        }
    }
}

/// `true` when the Grok generation supports xAI's Responses endpoint. The
/// Responses route launched alongside Grok 3 and stays available for every
/// later release; Grok 2 / grok-beta / grok-1 still only answer Chat
/// Completions. Match the major version prefix rather than enumerating
/// every dated SKU so new `grok-4-fast-*`, `grok-5-*`, etc. variants pick
/// up the richer wire automatically.
pub(crate) fn is_responses_capable(model: &str) -> bool {
    // Strip an optional `xai/` aggregator namespace prefix so models served
    // through, e.g., OpenRouter routed back into the xAI dedicated provider
    // (rare but possible via base_url override) still resolve correctly.
    let id = model.split_once('/').map(|(_, id)| id).unwrap_or(model);
    // grok-code-* is a Grok 4-era code-tuned family that ships on Responses
    // (see `https://docs.x.ai/docs/models`). It does not carry a numeric
    // generation in the id, so opt it in explicitly.
    if starts_with_ignore_ascii_case(id, "grok-code") {
        return true;
    }
    let Some(rest) = strip_prefix_ignore_ascii_case(id, "grok-") else {
        return false;
    };
    let Some(generation_char) = rest.chars().next() else {
        return false;
    };
    matches!(generation_char, '3'..='9')
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .and_then(|_| value.get(prefix.len()..))
}

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    strip_prefix_ignore_ascii_case(value, prefix).is_some()
}

#[cfg(test)]
#[path = "xai_tests.rs"]
mod tests;
