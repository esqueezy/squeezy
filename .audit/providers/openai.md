# OpenAI (Responses API) Provider Audit

## Summary

- **Severity tally**: 2 critical / 5 high / 9 medium / 6 low / 4 nit
- **Top 3 actionable recommendations**:
  1. Fix the SSE decoder so empty `data:` heartbeat lines do not crash the stream with `invalid SSE JSON` (`crates/squeezy-llm/src/sse.rs:49-63` + `crates/squeezy-llm/src/openai.rs:478-479`). This currently terminates any long stream the moment OpenAI emits a keep-alive.
  2. Handle `response.refusal.delta` / `response.refusal.done` and `response.output_text.done` (the authoritative-text reconcile). Today refusals are silently dropped on the floor (logged as "unhandled") so the user sees an empty completion with `StopReason::EndTurn` when the model refuses outside structured-output mode.
  3. Wrap the OpenAI `stream_response` with `retry::with_stream_retry` (the helper the Anthropic provider already uses at `crates/squeezy-llm/src/anthropic.rs:490`). Without it any mid-stream transport drop after a 200 OK aborts the turn instead of reconnecting — Anthropic, Chat-Completions and Codex all reconnect.

## Implementation Overview

The OpenAI provider lives in `crates/squeezy-llm/src/openai.rs` (792 lines) plus `crates/squeezy-llm/src/openai_prompt_cache.rs` (40 lines for the 64-codepoint clamp). The same struct backs the native OpenAI Responses route, Azure OpenAI Responses (via `from_azure_config`), and the xAI `/responses` route (`from_xai_config`), and an OAuth-authenticated Codex variant through `with_api_key_source`. The Chat-Completions wire is handled by the sibling `OpenAiCompatibleProvider` in `compatible.rs` and is audited separately.

Request lifecycle: `OpenAiProvider::stream_response` (line 307) validates vision support, builds the request body via the static `request_body` helper (line 149), POSTs to `<base_url>/responses` (Azure appends `?api-version=…`), and pipes the SSE response through the shared `SseDecoder` (`sse.rs:6`). Each parsed event is forwarded through `parse_openai_event` (line 470). Events handled are a thin subset: `response.output_text.delta`, `response.reasoning_summary_text.delta`, `response.reasoning_text.delta`, `response.output_item.done` (reasoning + function_call only), `response.completed`, `response.incomplete`, `response.failed`, `error`. The `[DONE]` sentinel is treated as a no-op even though the Responses API never emits it.

Notable design choices: (a) `ReasoningAccumulator` backfills empty `summary: []` arrays from streamed deltas (good — production providers do drop this); (b) the deprecated `cache_key` field lifts into the new `CacheSpec` shape; (c) cache-affinity headers (`session_id`, `x-client-request-id`) accompany the body field; (d) Azure deployment names are resolved by a `BTreeMap` lookup; (e) `previous_response_id` is forwarded verbatim. The provider does NOT support: org/project headers, `service_tier`, encrypted tool result content arrays, `OpenAI-Beta`, per-turn streaming retry, structured output annotations, refusal events, or content-part lifecycle events.

## Findings

### [CRITICAL] Empty SSE `data:` heartbeat crashes the stream

- **Location**: `crates/squeezy-llm/src/sse.rs:49-63`, `crates/squeezy-llm/src/openai.rs:478-479`
- **Observed**: `decode_sse_event` pushes the trimmed payload after every `data:` prefix, including `""`. The wrapped `parse_openai_event` then calls `serde_json::from_str("")`, which fails and returns `SqueezyError::ProviderStream("invalid SSE JSON: EOF while parsing a value at line 1 column 0")`.
- **Issue**: The SSE spec allows `data:` lines with no body as a keep-alive shape, and OpenAI emits keep-alive padding on idle reasoning turns. Codex's parser logs and `continue`s on parse failures (`others/codex/codex-rs/codex-api/src/sse/responses.rs:441-444`); opencode's `Schema.decode` rejects but never propagates. Squeezy escalates the parse error to a terminal stream error.
- **Impact**: Long reasoning turns on o-series / gpt-5 (where OpenAI may stream keep-alives between summary chunks) abort with "invalid SSE JSON" instead of completing. Heuristic: the longer the model thinks, the more likely the kill. The user already verified end-to-end works manually because short turns rarely see heartbeats.
- **Fix sketch**: In `decode_sse_event`, drop any `data_lines` element that is empty after `trim_start`; if `data_lines` becomes empty, return `None`. Alternative: in `parse_openai_event`, early-return `Ok(None)` when `data.trim().is_empty()`.
- **Reference**: WHATWG EventSource §9.2 ("If the field name is `data` and the field value is the empty string, then append a single U+000A LINE FEED…"); `others/codex/codex-rs/codex-api/src/sse/responses.rs:439-445` (continue-on-parse-error).

### [CRITICAL] `response.refusal.delta` is silently dropped

- **Location**: `crates/squeezy-llm/src/openai.rs:600-608` (default unhandled case)
- **Observed**: The `match event_type` block doesn't list `response.refusal.delta` or `response.refusal.done`, so refusals fall through to the `_ =>` arm which logs "unhandled OpenAI SSE event" and returns `Ok(None)`.
- **Issue**: When the model refuses (safety filter, content policy), OpenAI streams refusal text via `response.refusal.delta` chunks ending with `response.refusal.done`. The terminal `response.completed` arrives with no `incomplete_details` (the refusal IS the completion, not an "incomplete" state), so squeezy normalises the stop reason to `EndTurn` (line 558-561) and the user sees an empty assistant turn.
- **Impact**: Safety-flagged turns look like the model silently produced nothing — agent loop will likely retry with the same prompt forever, burning quota. The transcript carries no record of the refusal.
- **Fix sketch**: Add `response.refusal.delta => Ok(Some(LlmEvent::TextDelta(delta)))` (treat refusal text as visible output; mark via a `LlmEvent::Refusal` variant if the agent needs to distinguish, or normalise `stop_reason` to `StopReason::Refusal` when any refusal delta latched). Or follow opencode's pattern: track `hasRefusal` in state and surface `StopReason::Refusal` at completion (`others/opencode/packages/llm/src/protocols/openai-responses.ts:416-422`).
- **Reference**: https://developers.openai.com/api/reference/resources/responses/streaming-events (refusal.delta event).

### [HIGH] No `with_stream_retry` wrapper — mid-stream truncation kills the turn

- **Location**: `crates/squeezy-llm/src/openai.rs:307-427`
- **Observed**: `stream_response` returns the raw `try_stream!` body. Anthropic wraps its equivalent in `with_stream_retry(provider, RetryPolicy::provider_stream(transport), cancel.clone(), move || …)` (see `crates/squeezy-llm/src/anthropic.rs:490`).
- **Issue**: Any transient TCP/TLS drop after the 200 response (mid-token bytes_stream error) bubbles up as `SqueezyError::ProviderStream(err)` without reconnect. Anthropic and chat-completions get a free re-attempt via `StreamSkipState` accounting.
- **Impact**: A flaky network or a transient OpenAI 200-then-RST shows up as a failed turn for OpenAI users, while the same incident silently recovers on Anthropic and Chat Completions.
- **Fix sketch**: Refactor the closure body into `make_attempt` and wrap with `with_stream_retry("openai", RetryPolicy::provider_stream(transport), cancel.clone(), make_attempt)`. The retry helper already de-duplicates `TextDelta` / `ReasoningDelta` / `ToolCall` / `Started` events.
- **Reference**: `crates/squeezy-llm/src/retry.rs:486-588`, `crates/squeezy-llm/src/anthropic.rs:490`.

### [HIGH] `response.completed` after `response.failed` discards the response_id and usage

- **Location**: `crates/squeezy-llm/src/openai.rs:591-599`
- **Observed**: `"error" | "response.failed" => Err(SqueezyError::ProviderStream(message.to_string()))`. Only the error message is propagated. The response object on a failed turn carries `id`, `error.code`, `error.param`, and sometimes a partial `usage` block.
- **Issue**: The agent cannot distinguish `context_length_exceeded` (re-compact), `insufficient_quota` (stop loop), `cyber_policy` (stop and surface), `rate_limit_exceeded` (backoff), `server_is_overloaded` (retry-with-delay), or `usage_not_included` (subscription stale). Codex categorizes these explicitly into `ApiError::ContextWindowExceeded` / `QuotaExceeded` / `CyberPolicy` / `ServerOverloaded` / `Retryable { delay }` (`others/codex/codex-rs/codex-api/src/sse/responses.rs:312-345`).
- **Impact**: Hard-quota failures retry through the agent loop until the user cancels manually; rate-limited streams retry without honoring the embedded `try again in 3s` hint; context-overflow failures are not surfaced to `crate::overflow::classify_terminal`.
- **Fix sketch**: Parse `event["response"]["error"]` into `{ code, message, param }`, then branch on `code`. Map to existing `crate::overflow::OverflowSignal` for `context_length_exceeded`, route to retry policy with the parsed `try again in X` for `rate_limit_exceeded`. Keep `response_id` in the propagated error so transcripts can replay.
- **Reference**: `others/codex/codex-rs/codex-api/src/sse/responses.rs:312-356, 513-554`.

### [HIGH] No `response.function_call_arguments.delta` handling — no incremental tool-call streaming

- **Location**: `crates/squeezy-llm/src/openai.rs:504-546`
- **Observed**: Only `response.output_item.done` produces a `ToolCall` event. The intermediate `response.function_call_arguments.delta` (and `…done`) events are dropped via the `_ =>` arm.
- **Issue**: For long tool arguments (`apply_patch` payloads, multi-file diffs) the UI shows no progress until the entire arg JSON has been assembled server-side. Codex emits `ResponseEvent::ToolCallInputDelta` per chunk (`others/codex/codex-rs/codex-api/src/sse/responses.rs:280-289`); opencode threads each delta through `ToolStream.appendExisting` (`others/opencode/packages/llm/src/protocols/openai-responses.ts:557-575`).
- **Impact**: UX regression vs. Anthropic / Chat Completions; the TUI's "tool args streaming" indicator never advances for OpenAI turns even though the wire is sending deltas. Also makes the agent's compaction/cost estimation lag.
- **Fix sketch**: Add a streaming `ToolCallDelta { call_id, name, arguments_chunk }` event and a parser branch on `response.function_call_arguments.delta` / `response.output_item.added` (which carries the initial `item.call_id` + `item.name`). Fall back to the existing `output_item.done` aggregation when the event stream comes through buffered.
- **Reference**: https://developers.openai.com/api/reference/resources/responses/streaming-events; opencode `onFunctionCallArgumentsDelta`.

### [HIGH] Quota / context-overflow error envelopes aren't mapped to `OverflowSignal`

- **Location**: `crates/squeezy-llm/src/openai.rs:591-599`
- **Observed**: `response.failed` returns a plain `SqueezyError::ProviderStream(message)`. No `LlmEvent::ContextOverflow { provider, signal }` is ever emitted from the Responses path.
- **Issue**: The overflow classifier (`crate::overflow::classify_terminal`) is designed to consume the canonical signal. By bypassing it, the agent's compact-and-retry recovery doesn't fire when OpenAI returns `code: "context_length_exceeded"` mid-stream.
- **Impact**: A user hitting the 200k-token window on `gpt-5` sees a bare provider error and is told to manually `/compact` instead of squeezy doing it automatically.
- **Fix sketch**: When parsing the `response.failed` error envelope (see previous finding), yield `LlmEvent::ContextOverflow { provider, signal: OverflowSignal::Detected }` before the terminal `Err(...)`.
- **Reference**: `crates/squeezy-llm/src/overflow.rs`, comment at `lib.rs:574-578`.

### [HIGH] `response.output_text.done` reconcile is skipped, deltas can drift

- **Location**: `crates/squeezy-llm/src/openai.rs:504-512`
- **Observed**: Only `response.output_text.delta` produces output. The matching `response.output_text.done` (which carries the authoritative final text for that part) is ignored.
- **Issue**: Per OpenAI's streaming guide ("the final `.done` event carries the completed string for that piece"), clients should reconcile deltas with the `.done` text in case any delta was dropped or a chunk was re-ordered. Squeezy never observes the authoritative value.
- **Impact**: If two `output_text.delta` events arrive in the wrong order (rare but documented), or one is dropped during reconnect-without-skip (because finding 3 isn't in place), the persisted transcript diverges from what the model actually said.
- **Fix sketch**: Add `response.output_text.done` to the parser. Compare the cumulative delta buffer against the `text` field of the done event; if they diverge, emit a corrective `LlmEvent::TextDelta` for the suffix.
- **Reference**: OpenAI streaming events doc (cited above).

### [MEDIUM] `instructions: ""` is always serialized

- **Location**: `crates/squeezy-llm/src/openai.rs:160-166`
- **Observed**: `json!({ ..., "instructions": request.instructions, ... })` always emits the field.
- **Issue**: Codex marks the field `#[serde(skip_serializing_if = "String::is_empty")]` (`others/codex/codex-rs/codex-api/src/common.rs:172-173`). When the caller has no system prompt the empty-string instructions overrides any stored conversation default on `previous_response_id` chains.
- **Impact**: A turn that should inherit the stateful conversation's instructions from `previous_response_id` accidentally drops them.
- **Fix sketch**: Only insert `instructions` when `!request.instructions.is_empty()`.
- **Reference**: codex `ResponsesApiRequest`.

### [MEDIUM] `tool_choice` is dropped when tools are empty

- **Location**: `crates/squeezy-llm/src/openai.rs:218-240`
- **Observed**: `body["tool_choice"]` is only set inside `if !request.tools.is_empty()`.
- **Issue**: Callers can't say `tool_choice: "none"` on a turn that re-uses tools via `previous_response_id` (the stored tools come from the prior turn, not the current `request.tools`). Codex sends `tool_choice: "auto"` unconditionally; opencode sends it whenever set.
- **Impact**: With Responses-state continuations, `parallel_tool_calls=false` and `tool_choice="none"` are useless on follow-up turns where the caller doesn't re-attach the tools.
- **Fix sketch**: Move the `if let Some(choice) = request.tool_choice.as_deref()` block out of the `tools` guard.

### [MEDIUM] No org / project / `service_tier` / `OpenAI-Beta` knobs

- **Location**: `crates/squeezy-llm/src/openai.rs:335-355` (request builder), `crates/squeezy-core` config types
- **Observed**: Only `bearer_auth(key)` (or `api-key` header on Azure) is attached, plus the affinity headers. There's no `OpenAI-Organization`, `OpenAI-Project`, `service_tier` body field, or `OpenAI-Beta` header.
- **Issue**: Multi-org users on a single key can't route requests to the right billing entity. Enterprise users using "flex" (50% off, slower) or "priority" (paid SLAs) tiers can't opt in. `OpenAI-Beta: assistants=v2` etc. is also missing.
- **Impact**: Squeezy users on a Pay-As-You-Go org with multiple projects have all OpenAI usage charged to the default project, and can't access the flex tier (which would halve costs on long o3 reasoning turns).
- **Fix sketch**: Add `organization`, `project`, `service_tier` to `OpenAiConfig`; thread `service_tier` into `request_body` per `request.service_tier` Option; emit `OpenAI-Organization` / `OpenAI-Project` headers when set. Codex supports both (`others/codex/codex-rs/core/src/client.rs:752, 905`).

### [MEDIUM] No default `reasoning.effort` per model

- **Location**: `crates/squeezy-llm/src/openai.rs:205-217`, `crates/squeezy-llm/src/registry.rs:21`
- **Observed**: `reasoning.effort` is only set when `request.reasoning_effort.is_some()`. The registry only carries a boolean `reasoning_effort: bool` flag.
- **Issue**: Codex maintains a `default_reasoning_level` per `ModelInfo` and applies it when the caller didn't pick one (`others/codex/codex-rs/core/src/client.rs:705`). OpenAI's server-side default for o3 is "medium" but for gpt-5 it varies — pinning it client-side gives reproducible behavior.
- **Impact**: Bug surface when OpenAI tweaks server-side defaults; users observe behavior drift turn-over-turn.
- **Fix sketch**: Extend `ModelCapabilities` with `default_reasoning_effort: Option<ReasoningEffort>` and fall back to it before omitting `effort`.

### [MEDIUM] `previous_response_id` 404 / expiration not gracefully recovered

- **Location**: `crates/squeezy-llm/src/openai.rs:167-169`
- **Observed**: `previous_response_id` is forwarded verbatim. There's no detection of the `previous_response_not_found` error code that OpenAI returns when the 30-day TTL elapses or when `store: false` chains break.
- **Issue**: After 30 days the agent's stored response chain becomes invalid; OpenAI returns `404 {"error": {"code": "previous_response_not_found"}}`. The current code bubbles up a `ProviderRequest` error with the raw 404 body.
- **Impact**: Resumed long-lived sessions hard-fail with a confusing 404 instead of clearing the stale id and re-sending full input.
- **Fix sketch**: On `404 + code: previous_response_not_found`, emit a structured `SqueezyError::ProviderStateExpired` so the agent's retry layer can drop the id and resend the materialized input.
- **Reference**: https://community.openai.com/t/responses-previous-response-id-reliable/1362365 (community confirmation of 30-day TTL).

### [MEDIUM] Tool result `output` is string-only — can't carry images

- **Location**: `crates/squeezy-llm/src/openai.rs:639-643`
- **Observed**: `LlmInputItem::FunctionCallOutput { call_id, output }` serializes `output` as a plain string.
- **Issue**: The Responses API accepts `function_call_output.output` as either a plain string or an ordered array of `{type:"input_text"|"input_image"…}` items (opencode demonstrates: `others/opencode/packages/llm/src/protocols/openai-responses.ts:60-68`). Tool results that need to return a screenshot (browser tool, OCR pipeline) have to inline a base64 data URL into the JSON string output, which the model parses poorly.
- **Impact**: Vision-using sub-agents can't surface visual tool outputs to the next assistant turn.
- **Fix sketch**: Extend `LlmInputItem::FunctionCallOutput` to optionally carry a structured payload analogous to opencode's `ToolResultContentPart`. Existing string outputs still serialize as plain strings.

### [MEDIUM] `LlmInputItem::UserText` content shape diverges from production agents

- **Location**: `crates/squeezy-llm/src/openai.rs:619-628`, `crates/squeezy-llm/src/openai.rs:611-617`
- **Observed**: User text is serialized as `{ role: "user", content: "<text>" }` (string content). Opencode emits `{ role: "user", content: [{type:"input_text",text:"…"}] }` (array of typed parts).
- **Issue**: Both forms are accepted by OpenAI today. However the string form is being phased out for the Responses API (the API reference defines `content` as an array of input parts). Mixing this turn shape with `LlmInputItem::Image` produces inconsistent shapes within the same `input` array (string here, array there).
- **Impact**: Future API tightening could reject the string form; today it occasionally triggers different prompt-cache prefixes than the array form (the body bytes differ).
- **Fix sketch**: Always emit `content` as an array of `{type:"input_text", text}` parts. Drop the `if let [UserText(text)] = input { return json!(text); }` fast-path at line 612-614.

### [MEDIUM] No cancellation cleanup beyond the cancel token

- **Location**: `crates/squeezy-llm/src/openai.rs:378-383`
- **Observed**: `tokio::select! { _ = cancel.cancelled() => yield Cancelled; return; }` — works when the caller fires the token explicitly. Dropping the `LlmStream` cancels via reqwest connection close, but no `Cancelled` event is emitted.
- **Issue**: The agent's transcript / telemetry can't differentiate "user pressed Ctrl-C" from "stream dropped because the TUI overlay was rebuilt". The Anthropic provider has the same shape so it's not OpenAI-specific, but worth flagging.
- **Impact**: Telemetry under-counts user-initiated cancels.
- **Fix sketch**: Document the contract (`Cancelled` is opt-in, drop is silent), or add a `Drop` impl on the stream to emit a final telemetry beacon.

### [LOW] Stream-truncation error suppresses `response.failed` payload

- **Location**: `crates/squeezy-llm/src/openai.rs:421-425`
- **Observed**: Stream-ended-without-`response.completed` returns a fixed string with no diagnostic.
- **Issue**: When `response.failed` arrived but the parser already returned a per-event `Err`, the caller sees the error from that event. But on a clean TCP close after a half-emitted stream (rare), the error message is "OpenAI stream ended without response.completed" — same shape Anthropic uses, but Anthropic includes the upstream's last status / partial body. Hard to debug from a transcript alone.
- **Fix sketch**: Capture the last raw SSE chunk and include its first 200 bytes in the error string.

### [LOW] Affinity header values are unbounded

- **Location**: `crates/squeezy-llm/src/openai.rs:264-269`
- **Observed**: `session_id` and `x-client-request-id` are set to the raw `cache_spec.key` value, no length cap.
- **Issue**: The body field is clamped to 64 codepoints; the comment claims headers have a "general (much larger) header length cap" — but reqwest / hyper enforce 8KB header line limits per default config. Adversarial inputs (multi-MB cache keys propagated from user-controlled session ids) panic the request builder.
- **Impact**: Theoretical; in practice cache keys are squeezy-generated and small. Worth a defensive clamp.
- **Fix sketch**: Clamp header values to e.g. 256 bytes.

### [LOW] `parse_openai_event` doesn't validate `event.type` is a string

- **Location**: `crates/squeezy-llm/src/openai.rs:480-483`
- **Observed**: `value.get("type").and_then(Value::as_str).unwrap_or_default()` — a non-string `type` field silently turns into the "unhandled" arm.
- **Issue**: A malformed proxy could send `{"type": null, "data": ...}` and squeezy would treat it as benign rather than surfacing the protocol violation.
- **Fix sketch**: Track an unhandled-event counter; emit a tracing warn once per turn.

### [LOW] `response.in_progress` and `response.queued` aren't observable

- **Location**: `crates/squeezy-llm/src/openai.rs:504-608`
- **Observed**: Both events fall through to the `_ =>` arm (logged as unhandled).
- **Issue**: For deep reasoning turns (long `o3`/`gpt-5` queries) the UI has no signal between `Started` and the first `ReasoningDelta`. `response.in_progress` is the canonical "model is now thinking" beat. `response.queued` indicates the request is waiting for capacity (priority tier).
- **Fix sketch**: Emit `LlmEvent::ServerHeartbeat` (new variant) so the TUI can update its spinner.

### [LOW] No `response.content_part.added` / `…done` lifecycle tracking

- **Location**: `crates/squeezy-llm/src/openai.rs:504-608`
- **Observed**: Content part events are dropped.
- **Issue**: Some response shapes (audio, structured outputs with annotations) drive their own lifecycle through `content_part.*` rather than `output_text.delta`. Without these, audio outputs would be invisible.
- **Impact**: Speculative — squeezy isn't shipping audio today.
- **Fix sketch**: Future hook; add a `LlmEvent::ContentPart{added,done}` variant.

### [LOW] No annotation handling for citations / web_search

- **Location**: `crates/squeezy-llm/src/openai.rs:504-608`
- **Observed**: `response.output_text.annotation.added` is dropped.
- **Issue**: When the model emits a citation tied to a text region (file_search results, web_search URLs), the metadata never reaches the user. Anthropic surfaces citations via `LlmEvent` extensions; OpenAI annotations should too.
- **Fix sketch**: Emit `LlmEvent::Annotation { text_index, source }` once the feature is wired through.

### [LOW] Built-in tools (web_search / file_search / computer_use) not passed through

- **Location**: `crates/squeezy-llm/src/openai.rs:218-249`
- **Observed**: Tools are projected only as `{type:"function", name, description, parameters, strict}`. The Responses API supports hosted tools like `{type:"web_search"}`, `{type:"file_search"}`, `{type:"computer_use_preview"}`.
- **Issue**: Squeezy users can't opt into OpenAI's hosted tools. Opencode whitelists them (`others/opencode/packages/llm/src/protocols/openai-responses.ts:436-454`).
- **Fix sketch**: Extend `LlmToolSpec` with a `kind` enum (function / hosted) and emit `{type:"web_search"}` etc. when set.

### [NIT] `[DONE]` sentinel handling is dead code

- **Location**: `crates/squeezy-llm/src/openai.rs:474-476`
- **Observed**: `if data == "[DONE]" { return Ok(None); }` — Responses API never emits `[DONE]`; only Chat Completions does.
- **Fix sketch**: Remove the branch and the comment.

### [NIT] Trace event-type is empty string when `type` is missing

- **Location**: `crates/squeezy-llm/src/openai.rs:480-484`
- **Observed**: `tracing::trace!(target: "squeezy_llm::openai", event_type, "sse event")` with `event_type = ""` when the field is absent.
- **Fix sketch**: Skip the trace line when empty.

### [NIT] `affinity_headers` clones the key twice

- **Location**: `crates/squeezy-llm/src/openai.rs:264-269`
- **Observed**: `vec![("session_id", key.clone()), ("x-client-request-id", key)]`. The function consumes the key but still calls `.clone()`.
- **Fix sketch**: Pre-build a tuple with `key.clone()` and move the second one — already done — but the first `clone()` is unavoidable; minor and not a real fix.

### [NIT] `request_body` rebuilds `text` map even when neither verbosity nor schema set

- **Location**: `crates/squeezy-llm/src/openai.rs:185-197`
- **Observed**: Allocates `serde_json::Map::new()` unconditionally, then checks `if !text.is_empty()`.
- **Fix sketch**: Lazy-build the map only when either `response_verbosity` or `output_schema` is Some.

### Verified: ✓

- **`prompt_cache_key` 64-codepoint clamp**: Correctly counts codepoints via `char_indices` (`openai_prompt_cache.rs:28-36`); test coverage at `openai_tests.rs:286-377` includes multibyte cases. **Note**: the comment ("OpenAI silently drops…") understates the issue — OpenAI actually returns a 400 ("string too long") so the clamp is also a 400-rate fix, not just a cache-miss fix.
- **`prompt_cache_retention: "24h"` emission**: Long retention flag flows correctly (`openai_tests.rs:253-283`).
- **Cache-affinity headers gated on cache key presence**: `affinity_headers_absent_when_no_cache_key` (`openai_tests.rs:446-470`) covers this.
- **Reasoning summary backfill from streamed deltas**: `parser_backfills_empty_summary_from_streamed_deltas` (`openai_tests.rs:682-743`) covers the bug class.
- **Reasoning accumulator reset between items**: Same test confirms next_done clears the buffer.
- **Encrypted reasoning replay (store=false → `include: ["reasoning.encrypted_content"]`)**: `openai_tests.rs:165-170`.
- **Azure deployment name map**: `azure_deployment_name_map_*` tests cover both mapped and unmapped paths (`openai_tests.rs:986-1043`).
- **JSON output schema → `text.format` shape**: `request_body_emits_text_format_when_output_schema_set` (`openai_tests.rs:773-814`) covers the structured-outputs body.
- **Vision capability gating**: `LlmRequest::ensure_vision_support` is called before any HTTP work (`openai.rs:307-310`).
- **Tool-call canonical id replay**: `normalize_tool_ids_for_replay` (`lib.rs:396-456`) handles mid-session model switches.
- **Auth retry**: `send_with_auth_retry` correctly invalidates the key on 401/403 (`retry.rs:79-102`); supports both static and `RefreshableToken` sources.
- **Per-process shared `reqwest::Client`**: `shared_client` keyed on transport config — connection pool reused across providers (`transport.rs:68-89`).
- **Idle timeout per chunk**: `timeout(idle_timeout(transport), bytes.next())` (`openai.rs:383`); a stuck stream surfaces a typed error, not a hang.
- **Cancellation token wired into both initial request and stream loop**: `send_with_auth_retry` honors cancel (`retry.rs:108-118`), stream loop honors cancel (`openai.rs:378-383`).
- **Anthropic-shape reasoning replay safely dropped to OpenAI**: `lib.rs:677` returns `None` for non-OpenAi `ReasoningPayload`.
- **Multibyte `prompt_cache_key` preserved when under cap**: `request_body_preserves_multibyte_prompt_cache_key_under_codepoint_limit` (`openai_tests.rs:319-346`).

## Test Coverage Gaps

| Scenario | Severity | Mock-coverable? |
| --- | --- | --- |
| Empty SSE `data:` heartbeat doesn't kill the stream | Critical | Yes (mock SSE server) |
| `response.refusal.delta` → `response.refusal.done` chain produces visible output | Critical | Yes |
| Mid-stream `response.failed` with `code: context_length_exceeded` → `ContextOverflow` | High | Yes |
| Mid-stream `response.failed` with `code: rate_limit_exceeded` honors `try again in 3s` | High | Yes |
| Mid-stream TCP close after partial deltas triggers `with_stream_retry` reconnect | High | Yes (axum/wiremock server that closes mid-response) |
| `response.function_call_arguments.delta` → incremental tool-call event | High | Yes |
| `response.output_text.done` reconcile (text matches concatenated deltas) | High | Yes |
| Stale `previous_response_id` (404 `previous_response_not_found`) → typed error | Medium | Yes |
| `OpenAI-Organization` / `OpenAI-Project` headers sent when configured | Medium | Yes (header inspection in mock) |
| `service_tier: "flex"` / `"priority"` round-trips | Medium | Yes |
| `instructions: ""` is NOT emitted when empty | Medium | Yes |
| `tool_choice` honored when tools list is empty (Responses replay case) | Medium | Yes |
| `LlmInputItem::FunctionCallOutput` with structured (image+text) output | Medium | Yes once schema extended |
| Default reasoning effort per model (gpt-5 vs o3 vs o4-mini) | Medium | Yes (parametrize over model id) |
| `response.in_progress` / `response.queued` lifecycle events | Low | Yes |
| `response.output_text.annotation.added` annotations propagated | Low | Yes |
| Hosted-tool calls (web_search / file_search) round-trip | Low | Yes |
| Affinity header values > 256 bytes don't panic | Low | Yes |

## Verification Strategy

The codebase already has a precedent — `tests/lmstudio_mock.rs` and `tests/ollama_pull_mock.rs` use `wiremock` to stand up a fake provider. Apply the same pattern to OpenAI:

1. **SSE-fixture-driven parser tests.** All event-handling findings (critical / high / most medium) reduce to "call `parse_openai_event` with a `&str` fixture and assert the returned `LlmEvent`". Add fixtures alongside `openai_tests.rs` covering:
   - Empty `data:` line (assert parser does NOT error, or test SSE decoder directly).
   - `response.refusal.delta` chunks ending with `response.refusal.done` (assert visible output + `StopReason::Refusal`).
   - `response.failed` with each documented `error.code`.
   - `response.function_call_arguments.delta` events.

2. **End-to-end SSE-server fixtures.** Spin up `wiremock` (or hand-rolled axum) with an `/responses` route that streams a recorded byte sequence. Assert the high-level `LlmEvent` sequence matches expectations. Pattern: opencode's `openai-responses.test.ts` records real SSE byte streams into JSON fixtures (`others/opencode/packages/llm/test/fixtures/recordings/openai-responses/`) and replays them. Port the same approach to Rust.

3. **Header assertion.** wiremock can assert that the incoming request carried `OpenAI-Organization`, the `session_id` affinity header, the right `Bearer` token, and the `api-version` query parameter on Azure URLs.

4. **Mid-stream truncation.** Use `axum`/`tokio::io::duplex` to start a 200 response, ship 2 events, then drop the connection. The `with_stream_retry` wrapper should reconnect; without it, the test would observe `ProviderStream` directly.

5. **No external network needed.** Every finding above can be reproduced against a `127.0.0.1` server. The existing `costly-tests` feature stays as a sanity check; the new mock suite covers all of the above without an OpenAI key.

## References

- OpenAI Responses streaming events: https://developers.openai.com/api/reference/resources/responses/streaming-events
- OpenAI streaming guide: https://developers.openai.com/api/docs/guides/streaming-responses
- Responses streaming community guide (event taxonomy): https://community.openai.com/t/responses-api-streaming-the-simple-guide-to-events/1363122
- Prompt caching guide: https://developers.openai.com/api/docs/guides/prompt-caching
- `prompt_cache_key` 64-character limit GitHub issue: https://github.com/earendil-works/pi/issues/4720
- `prompt_cache_retention` "in_memory" / "24h" community confirmation: https://community.openai.com/t/prompt-cache-retention-not-being-recognized-as-a-valid-argument/1366509
- Conversation state / `previous_response_id` 30-day TTL: https://developers.openai.com/api/docs/guides/conversation-state
- Stale `previous_response_id` 404 community thread: https://community.openai.com/t/responses-previous-response-id-reliable/1362365
- Reasoning models temperature unsupported community thread: https://community.openai.com/t/gpt-5-models-temperature/1337957
- Service tier (`flex` / `priority`) docs reference: https://learn.microsoft.com/en-us/azure/foundry/openai/how-to/reasoning
- Migrate to Responses API guide: https://platform.openai.com/docs/guides/migrate-to-responses
- Structured outputs (`text.format` shape): https://developers.openai.com/api/docs/guides/structured-outputs
- Rate-limit headers: https://developers.openai.com/api/docs/guides/rate-limits
- Refusal event reference: https://platform.openai.com/docs/api-reference/responses-streaming/response/refusal/delta
- Codex Responses SSE parser (reference impl): `others/codex/codex-rs/codex-api/src/sse/responses.rs`
- Codex request builder (reference impl): `others/codex/codex-rs/core/src/client.rs:698-773`
- Opencode Responses protocol (TS reference impl): `others/opencode/packages/llm/src/protocols/openai-responses.ts`
