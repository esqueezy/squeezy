# Google Gemini Provider Audit

## Summary

Severity tally: 3 critical / 6 high / 7 medium / 5 low / 4 nit.

Top 3 actionable recommendations:

1. Fix `thinkingConfig` suppression for Gemini 2.5 — `models.json` flags `reasoning_effort: false` while `google.rs:80-88` gates `includeThoughts`/`thinkingBudget` on exactly that flag, so default requests never ask for thought summaries and never carry a budget. Flip the registry capability or stop gating thinking config on it.
2. Fix tool-call id collision — `call_id` uses the part index within a single SSE event, so two streamed chunks each carrying `functionCall` at `parts[0]` collide on `google_call_0` and the canonicalizer pairs both results to one call. Use a per-stream counter.
3. Wrap with `with_stream_retry` (Anthropic does) and surface `MALFORMED_FUNCTION_CALL` + `promptFeedback.blockReason` — squeezy silently truncates when Google's safety layer blocks (no candidates, only `promptFeedback`) or when tool args are unparseable.

## Implementation Overview

`crates/squeezy-llm/src/google.rs` (440 lines) carries the AI Studio Gemini path: `GoogleProvider` (`reqwest::Client` + `Arc<dyn ApiKeySource>` + base URL + transport), `request_body` (`:streamGenerateContent` body), `parse_google_event`, and `GoogleReasoningBuffer`. Built via `GoogleProvider::from_config(&GoogleConfig)` from `crates/squeezy-core/src/lib.rs:2249-2255`. Defaults: base URL `https://generativelanguage.googleapis.com/v1beta` (core/lib.rs:32), model `gemini-2.5-pro` (core/lib.rs:33). Credentials chain: inline TOML, `~/.squeezy/credentials.json`, `api_key_env` (`SQUEEZY_GOOGLE_KEY` default per core/lib.rs:602-604, with `GOOGLE_API_KEY` fallback), then `SQUEEZY_CREDENTIALS_JSON`. `GEMINI_API_KEY` is only honored in `tests/google_costly.rs:12` and CLI auth (`main.rs:1960`); the in-tree `fallback_env_var` never produces it.

Lifecycle: `stream_response` calls `ensure_vision_support("google")`, builds body (always `systemInstruction` + `contents` + empty `generationConfig`), POSTs to `{base_url}/models/{model}:streamGenerateContent?alt=sse` with `x-goog-api-key` header. Body always runs `crate::normalize_tool_ids_for_replay` so cross-provider replay survives; tools flatten to `[{functionDeclarations:[...]}]`. `send_with_auth_retry` does 401/403 invalidate + retry. On 200 the body feeds `SseDecoder` which splits on `\n\n`/`\r\n\r\n` and concatenates `data:` lines into JSON strings parsed by `parse_google_event`. Emits `LlmEvent::Started` before first chunk; `LlmEvent::Completed` at end with `CostSnapshot` + last `finishReason` normalized by `StopReason::from_google`.

Key design choices: (a) API key only in `x-goog-api-key` header — documented at `google.rs:116-121` to prevent `reqwest::Error::Display` leaking the key into `SqueezyError`. (b) Tool-result pairing uses `tool_names_by_call_id` after `lib.rs` canonicalization, then `google_contents` maps to Gemini's name-based scheme. (c) Reasoning uses `GoogleReasoningBuffer` accumulating summary text + one `thoughtSignature`, flushed on text/tool-call boundary or stream end. NOT wrapped in `with_stream_retry` unlike Anthropic (`anthropic.rs:490`).

## Findings

### [CRITICAL] Default request never carries `thinkingConfig`; thought summaries never requested

- **Location**: `crates/squeezy-llm/src/google.rs:80-88`, `crates/squeezy-llm/src/models.json:193,223,253`
- **Observed**: `let reasoning_capable = crate::capabilities_for("google", &request.model).is_some_and(|caps| caps.reasoning_effort); if reasoning_capable || request.reasoning_effort.is_some() { ... }`. `models.json` declares `"reasoning_effort": false` for `gemini-2.5-pro`, `gemini-2.5-flash`, `gemini-2.5-flash-lite`.
- **Issue**: `caps.reasoning_effort` is false for every Gemini 2.5 model; without `request.reasoning_effort` set explicitly, the whole `thinkingConfig` block is skipped — no `includeThoughts`, no budget. The comment at `google.rs:76-79` states the opposite intent.
- **Impact**: Users on Gemini 2.5 Pro never see reasoning summaries; `ReasoningDelta`/`ReasoningDone` is effectively dead code on default requests. Users are still billed for thinking (`thoughtsTokenCount` populates `reasoning_output_tokens` at `google.rs:369`).
- **Fix sketch**: Either set `"reasoning_effort": true` in `models.json` for Gemini 2.5 family, or introduce a separate `reasoning_tokens` gate, or hard-code "always send `includeThoughts: true` for `gemini-2.5*`". pi/opencode default thoughts-on for 2.5.
- **Reference**: https://ai.google.dev/gemini-api/docs/thinking ; `others/pi/packages/ai/src/providers/google.ts:367-378`.

### [CRITICAL] Tool-call `call_id` collides across SSE chunks for parallel calls

- **Location**: `crates/squeezy-llm/src/google.rs:391,428-432`
- **Observed**: `for (index, part) in parts.iter().enumerate() { ... call_id: format!("google_call_{index}") }`.
- **Issue**: `index` enumerates within a *single* SSE event. Gemini can split parallel `functionCall` parts across chunks; two chunks each with `parts[0]={functionCall:...}` both stamp `call_id="google_call_0"`. `normalize_tool_ids_for_replay` canonicalizes by original id, so two distinct tool calls collapse, and the second `FunctionCallOutput` overrides the first.
- **Impact**: Parallel tool calls lose pairing — the agent loop completes one tool and silently drops the other.
- **Fix sketch**: Lift counter to a per-stream `usize` (init outside SSE loop), pass into `parse_google_event`. opencode does this (`others/opencode/packages/llm/src/protocols/gemini.ts:364`); pi de-dupes ids (`others/pi/packages/ai/src/providers/google.ts:177-182`).
- **Reference**: https://ai.google.dev/gemini-api/docs/function-calling parallel calls.

### [CRITICAL] `MALFORMED_FUNCTION_CALL` and blocked-prompt responses surface as silent successes

- **Location**: `crates/squeezy-llm/src/google.rs:160-225`, `crates/squeezy-llm/src/lib.rs:521-529`
- **Observed**: Stream loop tracks `saw_any` only on SSE event presence. A response that begins with `{"promptFeedback":{"blockReason":"SAFETY"}, "usageMetadata":{...}}` (no candidates) reaches `parse_google_event`, `value.get("candidates").and_then(...).first()` returns None, returns `Ok([])`. `saw_any=true`, loop drains, `Completed { stop_reason: None }` fires. `finishReason: "MALFORMED_FUNCTION_CALL"` falls to `StopReason::Other("MALFORMED_FUNCTION_CALL")`.
- **Issue**: (a) Blocked prompts surface as 200 OK with empty candidates + `promptFeedback.blockReason`; squeezy never inspects that, so agent loop sees `Completed` with zero output and silently retries / gives up. (b) `MALFORMED_FUNCTION_CALL` should be a hard error (model emitted unparseable tool args); as `Other(…)` it's indistinguishable from "paused for unknown reason" — agent retries.
- **Impact**: "Gemini returned no output" with no reason; `MALFORMED_FUNCTION_CALL` causes pointless retry loops on small-model schema mismatches.
- **Fix sketch**: Inspect `promptFeedback.blockReason` in `parse_google_event` → `Err(SqueezyError::ProviderStream("Google blocked prompt: {block_reason}"))`. Add `"MALFORMED_FUNCTION_CALL"` to `StopReason::from_google` (opencode maps it to "error").
- **Reference**: https://ai.google.dev/api/generate-content `promptFeedback.blockReason`; https://github.com/vercel/ai/issues/4235.

### [HIGH] `XHigh` reasoning effort exceeds Gemini 2.5 Pro's max thinking budget (32_768)

- **Location**: `crates/squeezy-core/src/lib.rs:2470-2477`, `crates/squeezy-llm/src/google.rs:85`
- **Observed**: `ReasoningEffort::thinking_budget_tokens()` returns `60_000` for `XHigh`. `google.rs:85` plumbs it straight into `thinkingConfig.thinkingBudget`.
- **Issue**: Gemini 2.5 Pro caps at `32_768`; Flash/Flash-Lite at `24_576`. Even `High` (32_768) saturates Pro and exceeds Flash. One Anthropic-shaped scale is used for everyone.
- **Impact**: Any caller with `reasoning_effort: xhigh` on Gemini 2.5 gets a 400.
- **Fix sketch**: Clamp per-model in `google.rs:85` (`min(effort.thinking_budget_tokens(), per_model_max)`) or add a `google_thinking_budget(model, effort)` helper. pi has model-aware budgets at `others/pi/packages/ai/src/providers/google.ts:461-501`.
- **Reference**: https://ai.google.dev/gemini-api/docs/thinking — "2.5 Pro: 128 to 32768; 2.5 Flash: 0 to 24576; 2.5 Flash Lite: 512 to 24576."

### [HIGH] No `with_stream_retry` wrapper — transient stream truncation drops the turn

- **Location**: `crates/squeezy-llm/src/google.rs:127-225` vs `crates/squeezy-llm/src/anthropic.rs:490-496`
- **Observed**: Anthropic wraps in `with_stream_retry("anthropic", RetryPolicy::provider_stream(transport), cancel, make_attempt)`. Google builds inline and never wraps.
- **Issue**: Mid-stream RST, idle timeout, or partial frame returns `ProviderStream` immediately. No reconnect; no `StreamSkipState`.
- **Impact**: Long Gemini 2.5 Pro thinking turns on flaky networks lose the whole turn.
- **Fix sketch**: Extract body into `google_stream_attempt` helper and wrap in `with_stream_retry`. The `StreamSkipState` plumbing (`retry.rs:355-484`) already handles `ReasoningDelta`/`ToolCall` dedup.

### [HIGH] Tool `parameters` schema passed through unsanitized

- **Location**: `crates/squeezy-llm/src/google.rs:90-101`
- **Observed**: `parameters: tool.parameters` straight through.
- **Issue**: Gemini's `functionDeclarations[].parameters` is an OpenAPI 3.03 subset that rejects: `additionalProperties`, `$ref`/`$defs`, several `oneOf` patterns, integer-valued enums on strings, empty `{"type":"object"}` with no `properties` ("should be non-empty for OBJECT type"). The squeezy test (`google_tests.rs:39`) literally uses `json!({"type":"object"})` — the documented-invalid shape.
- **Impact**: Tool schemas working with Anthropic/OpenAI 400 on Gemini.
- **Fix sketch**: Add `sanitize_for_gemini` pass (drop `additionalProperties`, deref `$ref`, ensure non-empty object properties, coerce `[..,"null"]` → `nullable:true`). Or switch to `parametersJsonSchema` which supports full JSON Schema. opencode pipeline at `others/opencode/packages/llm/src/protocols/gemini.ts:144-162`; pi uses `parametersJsonSchema` (`others/pi/packages/ai/src/providers/google-shared.ts:272-288`).
- **Reference**: https://github.com/cline/cline/issues/918.

### [HIGH] `functionResponse.response` always wraps as `{output: str}`; loses structured data and error signal

- **Location**: `crates/squeezy-llm/src/google.rs:256-268`
- **Observed**: `"response": {"output": output}` — always.
- **Issue**: (a) `LlmInputItem::FunctionCallOutput { output: String }` (`lib.rs:256-259`) already collapses structured results to text. (b) opencode/pi use `{"error": msg}` on failure (`others/pi/packages/ai/src/providers/google-shared.ts:206`); squeezy always writes `output`, so Gemini can't tell a successful empty result from an error.
- **Impact**: Model treats every tool result as success; may re-call after errors.
- **Fix sketch**: Extend `LlmInputItem::FunctionCallOutput` with `is_error: bool` and switch key in `google.rs:265`; or heuristic on leading `Error:`.
- **Reference**: https://ai.google.dev/gemini-api/docs/function-calling.

### [HIGH] `tool_choice` not forwarded to Gemini

- **Location**: `crates/squeezy-llm/src/google.rs:54-103` (no `toolConfig`)
- **Observed**: `request.tool_choice` exists (`lib.rs:159`) — Google reads nothing.
- **Issue**: Even `"required"` (used to coerce tool-shy models) defaults to Gemini's `mode:"AUTO"`.
- **Impact**: Callers can't restrict to ANY/NONE.
- **Fix sketch**: Set `toolConfig.functionCallingConfig.mode` from `tool_choice` (auto/none/required → AUTO/NONE/ANY). opencode at `others/opencode/packages/llm/src/protocols/gemini.ts:173-179`.
- **Reference**: https://ai.google.dev/gemini-api/docs/function-calling#tool-config-mode.

### [HIGH] `output_schema` not forwarded to Gemini

- **Location**: `crates/squeezy-llm/src/google.rs:54-103`
- **Observed**: `LlmRequest.output_schema` (`lib.rs:240-244`) is plumbed for OpenAI Responses only.
- **Issue**: Gemini supports `responseMimeType:"application/json"` + `responseSchema` natively.
- **Impact**: Structured-output features (eval JSON tasks, contribution metadata) lose Gemini's strict guarantee.
- **Fix sketch**: When `output_schema.is_some()`, set `generationConfig.responseMimeType` and `generationConfig.responseSchema`. Strip schema features Gemini rejects (same sanitize pass).
- **Reference**: https://ai.google.dev/gemini-api/docs/structured-output.

### [MEDIUM] `response_id` hard-coded to `None` despite Gemini emitting it

- **Location**: `crates/squeezy-llm/src/google.rs:216-217`
- **Observed**: `response_id: None`. `parse_google_event` never reads top-level `responseId`.
- **Issue**: Gemini's `GenerateContentResponse` carries `responseId` — the natural correlation id for tracing/replay.
- **Impact**: Transcript exporter can't track Gemini turns by id.
- **Fix sketch**: Extract `value.get("responseId").and_then(Value::as_str)` like `modelVersion` and pipe to `Completed`.
- **Reference**: pi at `others/pi/packages/ai/src/providers/google.ts:91`.

### [MEDIUM] `thoughtSignature` shape too narrow for Gemini 3

- **Location**: `crates/squeezy-llm/src/google.rs:282-300,309-336`
- **Observed**: `GoogleReasoningBuffer` keeps a single `Option<String>` signature; on replay every summary part gets the same signature.
- **Issue**: Per Google docs and pi's history (`others/pi/packages/ai/CHANGELOG.md:1001,1412`), `thoughtSignature` is per-part, can appear on any part type, must not merge across parts. Works for 2.5 by coincidence; will break Gemini 3 multi-turn tool use (pi #1829).
- **Impact**: Future Gemini 3 entries will silently drop or wrongly merge signatures.
- **Fix sketch**: Restructure `ReasoningPayload::Google` to carry `Vec<(text, Option<sig>)>` and re-emit each Part with its original signature.

### [MEDIUM] `thoughtSignature` not preserved on text or tool-call parts

- **Location**: `crates/squeezy-llm/src/google.rs:391-433`
- **Observed**: Non-thought `text` parts and `functionCall` parts ignore `part.thoughtSignature`.
- **Issue**: Google docs: signatures may appear on any part type. Discarding them loses context preservation across turns.
- **Impact**: Same as above — fine on 2.5, breaks 3.
- **Fix sketch**: Add `text_signature`/`thought_signature` fields to `LlmInputItem::AssistantText` and the `FunctionCall` variant. pi at `others/pi/packages/ai/src/providers/google.ts:189`.

### [MEDIUM] Implicit prompt-caching not exposed via `CacheSpec`

- **Location**: `crates/squeezy-llm/src/google.rs:54-103`, `crates/squeezy-llm/src/models.json:195`
- **Observed**: `LlmRequest.cache` ignored. `models.json` declares `prompt_caching: false` everywhere.
- **Issue**: Gemini implicit caching is automatic and reports `cachedContentTokenCount` (squeezy reads it at `google.rs:368`). The flag is misleadingly off when caching does happen and costs are at cache-read rate. Explicit `cachedContent` not supported.
- **Impact**: `CacheRetention::Long` no-ops on Gemini; cost estimation assumes no caching.
- **Fix sketch**: Set `prompt_caching: true` for Gemini 2.5 + document implicit-only support; or plumb `request.cache.key` to a `cachedContent` field with explicit-cache registry.
- **Reference**: https://ai.google.dev/gemini-api/docs/caching.

### [MEDIUM] `Started` emitted before any chunk; first-chunk parse failure leaves dangling `Started`

- **Location**: `crates/squeezy-llm/src/google.rs:151`
- **Observed**: After HTTP 200 but before SSE chunk, `Started` fires. First-chunk error → caller saw `Started`, no `Completed`.
- **Impact**: Stream consumers must defend against missing `Completed`; affects retry telemetry.
- **Fix sketch**: Defer `Started` until first successful `parse_google_event`.

### [MEDIUM] `candidatesTokenCount` excludes thinking tokens; `output_tokens` under-counts

- **Location**: `crates/squeezy-llm/src/google.rs:365-370`
- **Observed**: `cost.output_tokens = usage.get("candidatesTokenCount")`.
- **Issue**: Per opencode (`protocols/gemini.ts:285-307`), `candidatesTokenCount` is *exclusive* of `thoughtsTokenCount`. If display code treats `output_tokens` as total billed output and adds reasoning separately, OK — but the squeezy ledger doesn't model that split explicitly.
- **Impact**: Cost reporting undercounts Gemini output by thinking-token count.
- **Fix sketch**: Document the split; or store `output_tokens = candidates + thoughts` plus a separate `visible_output_tokens`. Add a pinning test.

### [MEDIUM] Inline image base64 has no size guardrail; 20MB request cap unenforced

- **Location**: `crates/squeezy-llm/src/google.rs:269-277`
- **Observed**: Every image becomes a base64 `inlineData` part; total body never measured.
- **Issue**: Gemini's hard cap is 20MB total request size. Base64 inflates ~33%, so ~15MB raw image hits the limit.
- **Impact**: 400 INVALID_ARGUMENT with vendor error instead of a "use File API" hint.
- **Fix sketch**: Reject upfront when encoded body > 20MB with a structured error; or auto-promote large inlineData to `fileData` via the File API.
- **Reference**: https://ai.google.dev/gemini-api/docs/image-understanding.

### [LOW] SSE error envelope only reads `message`; status/code/details ignored

- **Location**: `crates/squeezy-llm/src/google.rs:347-353`
- **Observed**: Only `error.message` surfaces. HTTP non-200 path at `google.rs:142-148` is similar.
- **Issue**: Standard envelope is `{error:{code,message,status,details}}`. Dropping `status` (RESOURCE_EXHAUSTED, INVALID_ARGUMENT) means `retry.rs::is_terminal_quota_error` (matches OpenAI/Anthropic) doesn't catch Gemini billing exhaustion → full retry budget burns on each 429.
- **Fix sketch**: Extend `has_terminal_provider_error_shape` (`retry.rs:236-254`) with a Google branch on `status == "RESOURCE_EXHAUSTED"` + `details[].reason`. At minimum log all four fields.

### [LOW] No reconnect on idle-timeout

- **Location**: `crates/squeezy-llm/src/google.rs:160-170`
- **Observed**: Idle timeout returns `ProviderStream("Google stream idle timeout")` and propagates.
- **Issue**: Without `with_stream_retry`, the class of failure squeezy measures is unrecoverable for Gemini. Long thinking pauses on 2.5 Pro (no heartbeats) can exceed reasonable idle windows.
- **Fix sketch**: Same as the `with_stream_retry` finding.

### [LOW] `Started` emitted on empty 200 body

- **Location**: `crates/squeezy-llm/src/google.rs:151,210-212`
- **Observed**: Empty 200 → loop never enters, `saw_any=false`, error fires after `Started`.
- **Fix sketch**: Defer `Started` to first parsed event.

### [LOW] `base_url` only trims slashes; missing `/v1beta` is invisible

- **Location**: `crates/squeezy-llm/src/google.rs:49,228-230`
- **Observed**: `base_url.trim_end_matches('/')` then `format!("{base_url}/models/{model}:...")`.
- **Issue**: `https://example.com` (no `/v1beta`) produces `https://example.com/models/...` — wrong URL silently.
- **Fix sketch**: Validate the URL ends in `/v1*` in `from_config`.

### [LOW] `gemini` provider alias accepted but `GEMINI_API_KEY` not in `fallback_env_var`

- **Location**: `crates/squeezy-llm/src/credentials.rs:149-159`, `crates/squeezy-cli/src/main.rs:1960`
- **Observed**: `fallback_env_var("SQUEEZY_GOOGLE_KEY")` returns `Some("GOOGLE_API_KEY")` only.
- **Issue**: Costly test special-cases `GEMINI_API_KEY` but normal resolution doesn't. A user exporting `GEMINI_API_KEY` only is unresolved unless `GOOGLE_API_KEY` is also set.
- **Fix sketch**: Add explicit alias mapping so `SQUEEZY_GOOGLE_KEY` also tries `GEMINI_API_KEY`. Or document the requirement.

### [NIT] `StopReason::from_google` missing several enum values

- **Location**: `crates/squeezy-llm/src/lib.rs:521-529`
- **Observed**: Lacks `MALFORMED_FUNCTION_CALL`, `UNEXPECTED_TOOL_CALL`, `NO_IMAGE`, `IMAGE_PROHIBITED_CONTENT`, `IMAGE_RECITATION`, `IMAGE_OTHER`, `FINISH_REASON_UNSPECIFIED`, `OTHER`.
- **Fix sketch**: Extend match (pi enumerates all at `others/pi/packages/ai/src/providers/google-shared.ts:309-336`).

### [NIT] `tools[]` always single `functionDeclarations` wrapper

- **Location**: `crates/squeezy-llm/src/google.rs:90-100`
- **Issue**: Gemini `tools[]` can hold `functionDeclarations` + `codeExecution`/`googleSearch`. Not exposed today but the shape assumes single-purpose.

### [NIT] Display `google_call_N` ids coupled to wire ids

- **Location**: `crates/squeezy-llm/src/google.rs:429`
- **Issue**: Fine today (Google has no id); fragile once registry plumbs Google IDs.

### [NIT] No MIME validation on image upload

- **Location**: `crates/squeezy-llm/src/google.rs:269-277`
- **Issue**: `infer_image_mime` (lib.rs) covers PNG/JPEG/GIF/WEBP but user-provided `media_type` is shipped as-is; vendor-unsupported types slip through to a 400.

## Verified: ✓

- API key only in `x-goog-api-key` header, never URL (`google.rs:120-122,134-135`). Test at `google_tests.rs:8-21` pins this.
- `systemInstruction` at top-level, not in `contents` (`google.rs:67-69`).
- `role: "model"` for assistant turns (`google.rs:241-242,252`).
- `functionCall.args` sent as structured object, not JSON string (`google.rs:253`).
- `cachedContentTokenCount` and `thoughtsTokenCount` read from `usageMetadata` (`google.rs:368-369`).
- SSE decoder handles `\r\n\r\n` and `\n\n` boundaries; multi-line `data:` continuations (`sse.rs:36-63`).
- `modelVersion` captured and re-emitted as `LlmEvent::ServerModel` on mismatch (`google.rs:354-364`).
- `ensure_vision_support("google")` called at top of `stream_response` (`google.rs:112-114`).
- Cancellation: `tokio::select!` on `cancel.cancelled()` interleaved with byte poll + idle timeout (`google.rs:161-167`).

## Test Coverage Gaps

- **MALFORMED_FUNCTION_CALL** finishReason — no fixture (mockable).
- **promptFeedback.blockReason** empty-candidates path (mockable).
- **safetyRatings on blocked candidate** (mockable).
- **Parallel tool calls across separate SSE chunks** — `parser_extracts_text_tool_calls_and_usage` (`google_tests.rs:146-186`) tests both parts in one event only. Add fixture with two events each containing `functionCall` at `parts[0]` to assert distinct `call_id`s (currently fails).
- **`thoughtSignature` round-trip** preservation (mockable).
- **`finishReason` arriving in chunk after usage-only chunk** still surfaces.
- **Idle timeout** error path.
- **Error envelope** missing-message or full envelope cases.
- **`reasoning_effort: xhigh` budget validation** within model's range.
- **Tool-schema sanitization** — current test uses the documented-invalid `{"type":"object"}` shape.
- **`tool_choice` forwarding** into `toolConfig` (doesn't happen today).
- **`output_schema`** into `generationConfig.responseMimeType` (doesn't happen today).
- **20MB inline-image guardrail**.
- **`response_id` extraction** from Gemini's `responseId`.

All gaps mockable without a Google key by extending `google_tests.rs` with synthetic SSE strings.

## Verification Strategy

Local-only paths (no Google key required):

1. **Unit-level parser fuzz**: extend `google_tests.rs` with synthetic SSE strings reproducing each finding — `promptFeedback` empty candidates, `MALFORMED_FUNCTION_CALL`, two `functionCall` events with parts[0] collision, missing `usageMetadata`, partial JSON across chunks, `\r\n\r\n` vs `\n\n`, multi-line `data:` carrying inner JSON newlines. `parse_google_event` is pure.
2. **`SseDecoder` boundary tests** in `sse_tests.rs` — verify `data: {"candidates":` split from `[{"content":...}]}\n\n` reassembles.
3. **Mocked HTTP via wiremock**: stand up a local server miming `:streamGenerateContent?alt=sse`. Pin URL shape, header (`x-goog-api-key`), `Started`/`Completed` ordering on 200, then on 400/401/403/429/500. Confirm 401 triggers `ApiKeySource::invalidate` once.
4. **`base_url` validation**: assert `https://example.com` (no `/v1beta`) is rejected at `from_config` or produces an auditable URL.
5. **Capability gating**: unit test pinning that with current `reasoning_effort:false`, `request_body` produces no `thinkingConfig` key — locks in the bug, then update after the fix.
6. **`thinking_budget_tokens()` clamp** per-effort assertions clamped to per-model max (32_768/24_576).
7. **Replay harness**: persist a recorded SSE stream (tool call, thought summary, finishReason STOP, usage) under `tests/fixtures/google/` and replay through `parse_google_event` to lock the contract.
8. **Faux provider scenarios**: add a faux `google` scenario in `FauxProvider` so the agent loop exercises `Completed` shapes today only covered by hand-rolled tests.

The eval harness (`crates/squeezy-skills/external-docs/PROVIDERS.md`) can target `google` without real money via faux for dev, costly path only for final validation.

## References

- https://ai.google.dev/gemini-api/docs/thinking — thinkingBudget ranges, thoughtSignature semantics
- https://ai.google.dev/api/generate-content — GenerateContentResponse fields, usageMetadata, finishReason
- https://ai.google.dev/gemini-api/docs/function-calling — functionCall.args/functionResponse.response, toolConfig modes
- https://ai.google.dev/gemini-api/docs/api-key — `x-goog-api-key` header
- https://ai.google.dev/gemini-api/docs/models — gemini-2.5-pro/flash/flash-lite; Gemini 3 uses `thinkingLevel`
- https://ai.google.dev/gemini-api/docs/troubleshooting — HTTP codes (400/403/429/503)
- https://ai.google.dev/api/caching — explicit/implicit cache
- https://ai.google.dev/gemini-api/docs/image-understanding — 20MB inline cap
- https://cloud.google.com/apis/design/errors — `{error:{code,message,status,details}}`
- https://discuss.ai.google.dev/t/please-update-rest-documentation-to-include-alt-sse-param/2963 — SSE `data:` framing
- https://github.com/vercel/ai/issues/4235 — promptFeedback.blockReason streaming gap
- `others/pi/packages/ai/src/providers/google.ts:367-501` — model-aware thinking budgets
- `others/pi/packages/ai/src/providers/google-shared.ts:100-235,272-288,309-336` — message/tool-result conversion, parametersJsonSchema, finishReason map
- `others/pi/packages/ai/CHANGELOG.md:1001,571,178,1412` — Gemini 3 thoughtSignature lessons
- `others/opencode/packages/llm/src/protocols/gemini.ts:144-417` — schema sanitization, toolConfig modes, mapFinishReason, mapUsage
