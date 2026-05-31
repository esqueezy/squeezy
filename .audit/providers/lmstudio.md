# LM Studio Provider Audit

## Summary

- Severity tally: **2 critical / 4 high / 6 medium / 4 low / 2 nit** (18 total)
- Top 3 actionable recommendations
  1. **Wire the dedicated `LMStudioProvider` into `provider_from_config`** — today `provider = "lmstudio"` resolves to the generic `OpenAiCompatibleProvider` (no `LmStudio` arm in `ProviderConfig`). The hand-written, comment-laden adapter at `crates/squeezy-llm/src/lmstudio.rs` is reachable only via `OllamaProvider`'s OpenAI-compat delegate and the cross-crate `pub fn fetch_lmstudio_model_names`. Decide: delete `lmstudio.rs` (and route everything through `compatible.rs`) **or** add the `ProviderConfig::LMStudio(LMStudioConfig)` variant and route `OpenAiCompatiblePreset::LMStudio` to it. Today the two paths silently diverge.
  2. **Implement JIT-load surfacing and reasoning support.** The current adapter never reads `delta.reasoning` / `delta.reasoning_content`, has no notion of LM Studio's `ttl` field, treats a 400 ("model is not loaded") response identically to any other 4xx, and lacks the `model not loaded` UX hint that codex's `LMStudioClient::check_server` ships. Reasoning-mode local models (Qwen3, DeepSeek-R1, gpt-oss-20b) are squeezy's main target audience here and we drop their thinking output.
  3. **Reduce the bug surface between `lmstudio.rs` and `compatible.rs`.** The two parsers have drifted: `compatible.rs` accepts arrayed `content` parts, normalizes `length` / `content_filter` finish reasons with user-visible notices, drains a `reasoning_buf`, surfaces structured error envelopes with `(type, code)`, and skips incomplete tool calls instead of erroring the stream. `lmstudio.rs` does none of these. Factor the shared parser into `crate::chat_completions::parse_event` and have both providers consume it.

## Implementation Overview

The LM Studio surface lives in three places that don't agree:

1. **`lmstudio.rs`** — a hand-written Chat Completions adapter (`LMStudioProvider`, `LMStudioConfig`, `DEFAULT_LMSTUDIO_BASE_URL`, `fetch_lmstudio_model_names`). Module comment markets it as "`compatible.rs` minus aggregator bits" (`lmstudio.rs:6-10`). One production caller: `OllamaProvider::from_config` uses it as a delegate when `OllamaRoute::OpenAiCompatible` (`ollama.rs:35-39`).
2. **`compatible.rs` + `OpenAiCompatiblePreset::LMStudio`** — the generic provider. `provider_from_config` routes `ProviderConfig::OpenAiCompatible{ preset: LMStudio, .. }` here (`registry.rs:370-376`) since `ProviderConfig` has no `LMStudio` variant (`squeezy-core/src/lib.rs:1909-1930`). Real user flows land **here**, not in `lmstudio.rs`.
3. **`squeezy-core/src/lib.rs`** owns preset metadata: default base URL `127.0.0.1:1234/v1` (line 112), env `LMSTUDIO_API_KEY` (2118), aliases (2176), display name (2036).

**Request lifecycle** (`lmstudio.rs:149-253`): vision check → normalize tool-call ids → build body with `stream_options.include_usage = true` → `send_with_retry` POST `/v1/chat/completions` → `SseDecoder` → `parse_chat_event` updates `StreamState`; outer loop emits `Started`, optional `ServerModel`, `TextDelta`s, `ToolCall`s, `Completed`. No `[DONE]` requirement: clean EOF after `finish_reason=stop` ends cleanly (238-251). Idle timeout per chunk via `tokio::select!` against `cancel.cancelled()`.

**Why both exist**: `lmstudio.rs` aimed to drop the API-key requirement, preset headers, and Anthropic `cache_control` markers. In practice `OpenAiCompatibleProvider` already short-circuits those for non-Anthropic model ids — the only meaningful divergence the dedicated file preserves today is the *absence* of richer error/reasoning handling (i.e. drift listed below).

## Findings

### [CRITICAL] `provider = "lmstudio"` never reaches `LMStudioProvider`

- **Location**: `registry.rs:351-382`, `squeezy-core/src/lib.rs:1909-1930`, `lmstudio.rs:80-88`
- **Observed**: `ProviderConfig` has no `LMStudio` variant. `provider_from_config` routes `ProviderConfig::OpenAiCompatible` through `OpenAiCompatibleProvider::from_config` regardless of the inner `preset` (only `XAi → XaiProvider` branches). So `OpenAiCompatiblePreset::LMStudio` ends up in `OpenAiCompatibleProvider`, not `LMStudioProvider`.
- **Issue**: `LMStudioProvider` is reachable only via (a) `OllamaProvider`'s OpenAI-compat delegate (`ollama.rs:35-39`), (b) the `pub fn fetch_lmstudio_model_names` which has zero in-tree callers (see F04), and (c) tests. Users selecting LM Studio in TUI/CLI never hit `lmstudio.rs`.
- **Impact**: 1) every fix in `lmstudio.rs` (orphan tool-id canonicalization, `server_model` echo) is invisible to real users — they get `compatible.rs` instead, which has different behaviour. 2) `LMSTUDIO_API_KEY` resolves through `resolve_api_key_with_inline` (`compatible.rs:84`) which **errors `ProviderNotConfigured`** on empty key — so a default-no-auth LM Studio is unusable unless the user pre-sets `LMSTUDIO_API_KEY=anything`.
- **Fix sketch**: pick one:
  - **A (preserve `lmstudio.rs`)**: add `ProviderConfig::LMStudio(LMStudioConfig)`, route the preset to it in `provider_from_config`.
  - **B (delete duplication)**: drop `lmstudio.rs`, teach `compatible.rs` to tolerate missing keys for the LM Studio + vLLM + llama.cpp presets, give `OllamaProvider`'s delegate a thin From-impl.
- **Reference**: codex keeps a separate client (`others/codex/codex-rs/lmstudio/src/client.rs`, `others/codex/codex-rs/model-provider-info/src/lib.rs:402-435`). Opencode treats LM Studio as plain OpenAI-compatible (`others/opencode/.../providers.mdx:1349-1379`). Squeezy is the worst of both.

### [CRITICAL] Default base URL host literal drifts between core and llm crates

- **Location**: `lmstudio.rs:35` (`http://localhost:1234/v1`) vs `squeezy-core/src/lib.rs:112` (`http://127.0.0.1:1234/v1`)
- **Observed**: two `pub const DEFAULT_LMSTUDIO_BASE_URL` symbols with different host literals. `lib.rs:88` re-exports the llm-crate one; `preset.default_base_url()` returns the core-crate one.
- **Issue**: 1) only the core-crate value is observable from real user paths (see F01). 2) `localhost` resolves IPv6-first on Windows; LM Studio binds IPv4, so connections fail silently where `127.0.0.1` succeeds. 3) two `pub const` names invite import mistakes.
- **Impact**: hard-to-diagnose connect refusals on Windows IPv6 hosts; future contributor will pick the wrong constant.
- **Fix sketch**: delete `lmstudio.rs:35`, have `LMStudioConfig::default()` use `squeezy_core::DEFAULT_LMSTUDIO_BASE_URL`. Standardize on `127.0.0.1` (matches LM Studio docs and codex's `DEFAULT_LMSTUDIO_PORT` at `others/codex/codex-rs/model-provider-info/src/lib.rs:478-485`).

### [HIGH] `reasoning_content` / `delta.reasoning` is dropped silently

- **Location**: `lmstudio.rs:474-501` (vs `compatible.rs:1048-1135`)
- **Observed**: `parse_chat_event` reads `delta.content` and `delta.tool_calls` only. No branch for `delta.reasoning` (gpt-oss/o3-mini shape) or `delta.reasoning_content` (Qwen3 / DeepSeek-R1 / vLLM). No `reasoning_buf`, no `reasoning_only_stop` latch.
- **Issue**: LM Studio 0.3.9 added `reasoning_content`; 0.3.23 moved gpt-oss reasoning into `choices.delta.reasoning` to match o3-mini. Reasoning models are the headline LM Studio use case in 2026.
- **Impact**: 1) Qwen3 / DeepSeek-R1 thinking output never reaches the TUI; 2) reasoning-only finishes (model thinks, ends with `stop`, no content/tool calls) become empty assistant messages instead of the `compatible.rs:1106-1108` notice; 3) `cost.reasoning_output_tokens` is hardcoded `None` at line 518 even when `completion_tokens_details.reasoning_tokens` is sent.
- **Fix sketch**: lift `collect_delta_text` + `reasoning_buf` + `drain_reasoning` + `reasoning_only_stop` from `compatible.rs`. Read `delta.reasoning` and `delta.reasoning_content`; read `completion_tokens_details.reasoning_tokens` in `parse_chat_usage`.
- **Reference**: [LM Studio API Changelog](https://lmstudio.ai/docs/developer/api-changelog), `compatible.rs:1052-1066` for the working impl.

### [HIGH] No JIT-load surfacing; "model not loaded" lands as opaque 400

- **Location**: `lmstudio.rs:172-185, 449-457`
- **Observed**: any non-200 becomes `ProviderRequest("LM Studio {status}: {message}")` with raw body. No detection of LM Studio's `"Model 'X' is not loaded"` 400 shape, no `ttl` field for keep-alive.
- **Issue**: LM Studio returns 400/404 when the requested `model` isn't loaded and JIT is disabled. The body is `{"error":{"message":"Model 'qwen/qwen3-32b' is not loaded ...","type":"invalid_request_error"}}`. Users see raw JSON.
- **Impact**: most common LM Studio failure mode; users can't tell whether to fix their model id, install a model, or flip the JIT toggle.
- **Fix sketch**: case-match on `status == 400 && body_lower.contains("not loaded")` and append a hint pointing at Developer → Server Settings JIT toggle. Also plumb `[providers.lmstudio].jit_ttl_seconds` (default `Some(3600)`) into `LMStudioConfig` and emit `"ttl": ttl` in the request body (`lmstudio.rs:113-141`).
- **Reference**: [LM Studio TTL/Auto-Evict](https://lmstudio.ai/docs/app/api/ttl-and-auto-evict) — `ttl` is supported on OpenAI-compat endpoints, default 60 min.

### [HIGH] Empty/whitespace SSE chunks error the whole stream

- **Location**: `lmstudio.rs:446-447`, `sse.rs:49-63`
- **Observed**: `parse_chat_event` calls `serde_json::from_str(data)?` for anything other than literal `[DONE]`. Comment lines (`:`) are stripped to nothing by `sse.rs` (correctly returns `None`), but empty `data:\n` lines become `""` and hit `from_str("")` which fails. Trailing whitespace on `[DONE]` (e.g. `data: [DONE] \n`) lands as `"[DONE] "` and also fails — the literal string check at `lmstudio.rs:431` requires exact match.
- **Impact**: a single empty `data:` line or whitespace-padded `[DONE]` aborts the turn with `invalid SSE JSON: EOF while parsing ...`.
- **Fix sketch**: at the top of `parse_chat_event`, do `let data = data.trim(); if data.is_empty() { return Ok(Vec::new()); }` then match `[DONE]`.
- **Reference**: WHATWG SSE spec — empty data fields are valid; consumers must no-op.

### [HIGH] `fetch_lmstudio_model_names` has a 500ms timeout and zero callers

- **Location**: `lmstudio.rs:274-291`
- **Observed**: builds private `reqwest::Client` with `timeout(500ms)`, `GET {base}/models`. Bypasses `shared_client` + `ProviderTransportConfig`. `pub` and re-exported (`lib.rs:88`), zero in-tree callers.
- **Issue**: 1) dead public API — refactors must preserve its signature for hypothetical external callers. 2) 500 ms is too short for cold loopback enumeration with many models on disk. 3) no `Bearer` header, so a reverse-proxied LM Studio with auth silently degrades. 4) no `Accept: application/json`.
- **Impact**: model picker degrades to "no live discovery" against a perfectly healthy server.
- **Fix sketch**: delete (let the `compatible.rs` path drive discovery), or refactor to take `&LMStudioConfig`, use `shared_client`, attach `Bearer` when set, raise timeout to 5 s.
- **Reference**: codex's client uses `connect_timeout(5s)` + default reqwest read timeout (`others/codex/codex-rs/lmstudio/src/client.rs:32-43`).

### [MEDIUM] `length` and `content_filter` finish reasons emit no notice

- **Location**: `lmstudio.rs:491-501` vs `compatible.rs:1075-1131`
- **Observed**: drains tool calls and stashes reason — no `TextDelta` notice for `length` or `content_filter`.
- **Impact**: when LM Studio's default-low `max_tokens` (often 4096) bites, the transcript shows silent truncation. Confusing UX.
- **Fix sketch**: lift `compatible.rs:1111-1118`'s notice emission.

### [MEDIUM] No `saw_visible_output` / `reasoning_only_stop` detection

- **Location**: `lmstudio.rs:357-372` (`StreamState`)
- **Observed**: no `saw_visible_output` or `reasoning_only_stop` fields — no equivalent of `compatible.rs:797-811`.
- **Impact**: Qwen3 / DeepSeek-R1 reasoning-only finishes (model thinks, stops, no content/tool calls) become empty turns. Spinner stops with nothing in the transcript.
- **Fix sketch**: combined with F03, latch on first content/tool-name delta; on `stop` with empty visibility + non-empty reasoning buf, set `reasoning_only_stop` and emit notice.

### [MEDIUM] Error envelope parsed only one level deep

- **Location**: `lmstudio.rs:449-457`
- **Observed**: reads `error.message` only; ignores `error.type` and `error.code`. LM Studio's llama.cpp errors carry `type: "invalid_request_error"`, `code: "context_length_exceeded"`, etc.
- **Impact**: `OverflowSignal` classification (`overflow.rs::classify_terminal`) keys off `code=context_length_exceeded`; with the code stripped, LM Studio context overflows don't trigger compaction — the agent retries the same overflowing prompt.
- **Fix sketch**: reuse `compatible::format_chat_error` (`compatible.rs:976-998`); rename to `pub(crate)` and call it here.

### [MEDIUM] No handling of arrayed `content` delta

- **Location**: `lmstudio.rs:478-481`
- **Observed**: only `delta.content` as `as_str()`. `compatible.rs:940-967` documents arrayed-content emissions from Qwen via aggregators and from LM Studio's 0.3.29 `/v1/responses` shim.
- **Impact**: silently drops every arrayed delta. `output_tokens` bill with zero text surfaced.
- **Fix sketch**: share `collect_delta_text`.

### [MEDIUM] Incomplete tool call errors the whole stream

- **Location**: `lmstudio.rs:398-427`, `name.ok_or_else(...)?` at line 403
- **Observed**: `drain_tool_calls` returns `Err` when a partial has no `function.name`.
- **Impact**: local models routinely emit a tool-call wrapper without name (model bails mid-call, stream cuts). One hallucinated tool call kills the turn and discards any assistant text. `compatible.rs:865-884` defends against this with skip+warn.
- **Fix sketch**: mirror `compatible.rs:865-884` — skip the partial, warn-log, continue.

### [MEDIUM] Empty `arguments` silently coerces to `{}` with no marker

- **Location**: `lmstudio.rs:408-419`
- **Observed**: `if partial.arguments.is_empty() { "{}" }` then JSON-parse. Invalid JSON preserves raw text via `INVALID_TOOL_ARGUMENTS_*` keys; empty does not.
- **Impact**: small local models (Llama 3.1 8B, Qwen2.5 7B) often emit a tool call with no arguments when they meant to send required fields. Tool downstream fails with `missing required field` and the error trail blames the tool, not the model.
- **Fix sketch**: only coerce to `{}` when arguments are empty AND the tool spec has no required parameters; otherwise emit the `INVALID_TOOL_ARGUMENTS_KEY=true` marker with empty raw.

### [LOW] No `response_format` / structured output forwarding

- **Location**: `lmstudio.rs:113-141`
- **Observed**: `request.output_schema` is read nowhere.
- **Impact**: LM Studio supports structured outputs (GGUF llama.cpp grammar, MLX Outlines); squeezy callers that set `output_schema` get untyped text. Note `compatible.rs` has the same gap — squeezy-wide.
- **Fix sketch**: emit `"response_format": {"type": "json_schema", "json_schema": {...}}` when `request.output_schema` is set.
- **Reference**: [LM Studio structured output](https://lmstudio.ai/docs/developer/openai-compat/structured-output).

### [LOW] No `tool_choice` forwarding

- **Location**: `lmstudio.rs:122-140`
- **Observed**: `request.tool_choice` ignored. `compatible.rs:292-294` forwards it.
- **Impact**: tool-shy local models (Qwen2.5 7B) emit prose preambles instead of tool calls; `[model].tool_choice = "required"` is the documented workaround. LM Studio 0.3.15 added `tool_choice` support.
- **Fix sketch**: `if let Some(c) = request.tool_choice.as_deref() { body["tool_choice"] = json!(c); }`.

### [LOW] No tracing breadcrumb on request

- **Location**: `lmstudio.rs:149-172`
- **Observed**: no `tracing::debug!` at request build/dispatch.
- **Impact**: `RUST_LOG=squeezy_llm=debug` is silent for LM Studio sessions.
- **Fix sketch**: emit `tracing::debug!(target: "squeezy_llm::lmstudio", model = %request.model, ...)`.

### [LOW] `Reasoning` input items dropped silently on replay

- **Location**: `lmstudio.rs:352`
- **Observed**: `LlmInputItem::Reasoning(_) => return None`.
- **Impact**: cross-provider session that started on Anthropic loses thinking context when resumed on LM Studio. Same in `compatible.rs:676-678`.
- **Fix sketch**: replay as synthetic assistant text `"[previous reasoning]\n..."`.

### [NIT] `transport` `Copy` stored owned

- **Location**: `lmstudio.rs:62-67, 86-87, 155`
- **Stylistic only**; harmless.

### [NIT] `LMStudioConfig` missing `PartialEq` / `serde`

- **Location**: `lmstudio.rs:41-49`
- **Observed**: only `#[derive(Debug, Clone)]`. `OpenAiCompatibleConfig` derives `PartialEq, Eq, Serialize, Deserialize`.
- **Impact**: blocks F01 Option A; harness/round-trip tests can't `assert_eq!` two configs.
- **Fix sketch**: `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]`.

## Verified: OK

- **`[DONE]` exactly once**: `parse_chat_event` early-exits at `lmstudio.rs:431-444`; `completed_emitted` latch enforces single emit. Asserted in `lmstudio_tests.rs:130`, `lmstudio_mock.rs:120-124`.
- **Server-model echo**: captured at `lmstudio.rs:463-468`, drained at 209-211/225-227 — canonical helper.
- **Image data URL**: `data:{mime};base64,{...}` (`lmstudio.rs:339-350`); round-trip test at `lmstudio_tests.rs:220-253`.
- **Vision capability check**: fires before request build (`lmstudio.rs:150-152`).
- **Bearer conditional**: only attached when key set (`lmstudio.rs:165-167`) — no-auth path intact via the Ollama delegate path (not via F01's compat path).
- **Idle timeout** enforced per chunk (`lmstudio.rs:200`).
- **Cancellation** properly races `next()` (`lmstudio.rs:195-201`) and emits `LlmEvent::Cancelled`.
- **`trim_end_matches('/')`** prevents `//chat/completions` (`lmstudio.rs:84`).
- **Stream completes without `[DONE]`** (`lmstudio.rs:238-251`) — matches LM Studio's hang-up-after-usage behaviour.

## Test Coverage Gaps

- **[HIGH/easy]** No test for `reasoning_content` / `delta.reasoning` (F03). Extend `SSE_BODY` in `lmstudio_mock.rs:18-23`.
- **[HIGH/easy]** No test for "model not loaded" 400 hint (F04). Mock returns 400 + LM Studio envelope; assert hint in error string.
- **[HIGH/easy]** No test for empty `data:\n` or whitespace-padded `[DONE]` (F05).
- **[HIGH/easy]** No test for arrayed `content` delta (F10).
- **[MEDIUM/easy]** No test for `finish_reason=length` / `content_filter` notice (F07).
- **[MEDIUM/easy]** No test for incomplete tool-call skip (F11).
- **[MEDIUM/easy]** No test for `response_format` (F13) or `tool_choice` (F14) forwarding.
- **[MEDIUM/easy]** `lmstudio_mock.rs` doesn't exercise cancellation. Add slow-trickle SSE server + mid-stream cancel.
- **[MEDIUM/easy]** No retry-policy assertion (e.g. 500-then-200 with `request_max_retries: 0`).
- **[LOW/easy]** No `fetch_lmstudio_model_names` test against `/v1/models` mock (zero callers anyway — F04).
- **[LOW/easy]** No `const`-equality test asserting both `DEFAULT_LMSTUDIO_BASE_URL` symbols match (F02).
- **[LOW/easy]** `ollama_tests.rs:255-265` asserts `compat.is_some()` but doesn't drive a streamed response through the delegate. Easy: point `OllamaProvider` at the existing `lmstudio_mock.rs` server.
- **[LOW/medium]** No `lmstudio_costly.rs` against a real local server. Codex has env-var-gated tests; squeezy should too.

## Verification Strategy

LM Studio is free; each finding validates in ~20 minutes:

1. **Install**: `brew install --cask lm-studio`, Developer → Server → Start.
2. **F01**: set `provider = "lmstudio"` in `squeezy.toml`, run with `RUST_LOG=squeezy_llm=trace`. Confirm `OpenAiCompatibleProvider` (not `LMStudioProvider`) and that an unset `LMSTUDIO_API_KEY` gives `ProviderNotConfigured`.
3. **F02**: static — `cargo test base_url` with a snapshot assertion.
4. **F03 / F07 / F08**: load `qwen/qwen3-32b` or a DeepSeek-R1 quant; set `max_tokens = 64`; send a deep-reasoning prompt. Observe missing reasoning text, silent length truncation, raw error JSON on bad model id.
5. **F04**: with JIT disabled and no model loaded, send any request — observe raw 400 body.
6. **F05 / F10 / F11 / F13 / F14**: easier to validate by extending `lmstudio_mock.rs` with byte-level SSE payloads / non-200 responses and asserting the parsed event stream.

Port codex's `lmstudio_mock` style (`tokio::net::TcpListener`) and add a body variant per finding.

## References

- [LM Studio OpenAI Compatibility API](https://lmstudio.ai/docs/app/api/endpoints/openai)
- [LM Studio API Changelog](https://lmstudio.ai/docs/developer/api-changelog)
- [LM Studio Tool Use](https://lmstudio.ai/docs/developer/openai-compat/tools)
- [LM Studio Structured Output](https://lmstudio.ai/docs/developer/openai-compat/structured-output)
- [LM Studio Idle TTL and Auto-Evict](https://lmstudio.ai/docs/app/api/ttl-and-auto-evict)
- [LM Studio Responses API (`/v1/responses`)](https://lmstudio.ai/docs/developer/openai-compat/responses)
- [LM Studio v0.3.29 blog (Responses API)](https://lmstudio.ai/blog/lmstudio-v0.3.29)
- [LM Studio Authentication](https://lmstudio.ai/docs/developer/core/authentication)
- [LM Studio Parallel Requests](https://lmstudio.ai/docs/app/advanced/parallel-requests)
- [LM Studio DeepSeek R1 blog](https://lmstudio.ai/blog/deepseek-r1)
- [vLLM Reasoning Outputs](https://docs.vllm.ai/en/latest/features/reasoning_outputs/)
- [LM Studio bug tracker #988 — reasoning_effort ignored](https://github.com/lmstudio-ai/lmstudio-bug-tracker/issues/988)
- [LM Studio bug tracker #1217 — JIT not respecting requested model](https://github.com/lmstudio-ai/lmstudio-bug-tracker/issues/1217)
- [LM Studio bug tracker #1203 — Response API streaming not stopping on cancel](https://github.com/lmstudio-ai/lmstudio-bug-tracker/issues/1203)
- [LM Studio bug tracker #693 — Client disconnected after 2 min wait](https://github.com/lmstudio-ai/lmstudio-bug-tracker/issues/693)
- [LM Studio bug tracker #944 — 300s timeout](https://github.com/lmstudio-ai/lmstudio-bug-tracker/issues/944)
- `others/codex/codex-rs/lmstudio/src/client.rs` — codex's separate LM Studio client
- `others/codex/codex-rs/lmstudio/src/lib.rs:1-46` — `ensure_oss_ready` with model download / load
- `others/codex/codex-rs/model-provider-info/src/lib.rs:402-435` — codex's built-in provider registration
- `others/opencode/packages/web/src/content/docs/providers.mdx:1349-1379` — opencode's "LM Studio is just OpenAI-compatible" approach
