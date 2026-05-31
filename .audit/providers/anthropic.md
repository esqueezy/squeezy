# Anthropic Provider Audit

## Summary
- Severity tally: 2 critical / 6 high / 7 medium / 5 low / 4 nit
- Top 3 actionable recommendations:
  1. **Stop swallowing mid-stream `error` events as retryable `ProviderStream` failures** (`anthropic.rs:1032-1039`). Today a post-200 `event: error` (overloaded, model_context_window_exceeded, api_error) becomes a transient error the retry layer happily reconnects on, eating 5 attempts of identical failure. Capture `error.type`, route quota/overflow shapes through the existing classifier, and only reconnect on genuinely transient transport-level loss.
  2. **Round-trip image-carrying `tool_result.content` as an array of `{type:image, source:...}` blocks** instead of stringifying every `FunctionCallOutput.output` (`anthropic.rs:373-381`). MCP tools returning screenshots silently inflate context by ~110k tokens per PNG and make the image invisible to the model — opencode landed exactly this fix (`others/opencode/packages/llm/src/protocols/anthropic-messages.ts:101-114`).
  3. **Cap caller-supplied + auto cache breakpoints at 4 and tighten `model_uses_adaptive_thinking` to require the `claude-` family prefix** (`anthropic.rs:54-68`, `144-242`). Auto-three breakpoints + any caller marker fires a 400; the substring match on `opus-N-M` activates adaptive thinking + EFFORT beta against any proxy whose model id happens to contain those characters.

## Implementation Overview
The Anthropic provider lives in `crates/squeezy-llm/src/anthropic.rs` and centers on three pieces: (1) `AnthropicProvider::request_body` builds the `/v1/messages` JSON body for both API-key and OAuth-driven calls, (2) `anthropic_stream_attempt` performs one HTTP attempt against `{base_url}/messages` and streams SSE, (3) `parse_anthropic_event` decodes individual `data:` payloads into `LlmEvent`s using `AnthropicStreamState` for tool-use input accumulation, thinking blocks, and usage. The request body is parameterized on `AnthropicAuthScheme` (selected by sniffing `sk-ant-oat` token prefix) so OAuth calls prepend the Claude-Code identity system block, set `Authorization: Bearer`, stamp `user-agent: claude-cli/2.1.0` + `x-app: cli`, and merge `claude-code-20250219,oauth-2025-04-20` into `anthropic-beta`.

Request lifecycle: resolve key via `ApiKeySource::current_key()` → build body (normalise tool ids via `normalize_tool_ids_for_replay`, lift legacy `cache_key` into `CacheSpec`, choose adaptive vs explicit thinking, mark breakpoints on system/last-user/last-stable-tool) → POST → on non-200 run the overflow classifier and surface `format_for_provider_error` → on 200 spin an `SseDecoder` loop guarded by `cancel` and `idle_timeout`, parsing events and emitting `ContextOverflow` additively before `Completed`. Cancellation goes through a `CancellationToken` selected against `bytes.next()`. The shared `with_stream_retry` wrapper deduplicates already-yielded text/tool/reasoning prefixes across reconnects. OAuth tokens persist at `~/.squeezy/auth/anthropic.json` (mode 0600) with proactive refresh under an `RwLock` so concurrent callers fire only one refresh.

Notable design choice: `AnthropicStreamState::cost()` (`733-757`) folds Anthropic's "input_tokens = uncached delta only" convention back into `CostSnapshot.input_tokens` = total prompt tokens (uncached + cache_read + cache_write) so reporters see what the model actually saw. The cache breakpoint policy is "auto-3" (system tail + last user block + last stable tool, skipping `mcp__` dynamic tools) without consulting caller markers, breakpoint budgets, or per-model min-cacheable-token floors.

## Findings

### [CRITICAL] Mid-stream `error` events are reclassified as retryable `ProviderStream` errors
- **Location**: `crates/squeezy-llm/src/anthropic.rs:1032-1039`; reclassification at `crates/squeezy-llm/src/retry.rs:583-588`
- **Observed**: `parse_anthropic_event` returns `Err(SqueezyError::ProviderStream(message))` for any `event: error` after 200 OK. `with_stream_retry` matches `ProviderStream` as retryable and reconnects up to `stream_max_retries` (default 5).
- **Issue**: Anthropic emits `event: error` with `error.type ∈ {overloaded_error, api_error, invalid_request_error, model_context_window_exceeded, ...}` after the 200 has already been flushed. Treating every variant as transient means a `model_context_window_exceeded` mid-stream replays the identical prompt 5×; an `overloaded_error` floods Anthropic against the user's wishes (the existing 5xx/429 retry policy is *explicitly disabled* for streams, `retry.rs:46-55`). The pre-200 path runs `classify_terminal` and emits a `ContextOverflow` event (`anthropic.rs:550-561`); the post-200 path doesn't. Identical errors get opposite treatment depending on whether 200 flushed.
- **Impact**: Million-token prompts that overflow mid-stream replay 5×, eating quota; overloaded states inflate Anthropic load.
- **Fix sketch**: Capture `error.type`/`message`, run `classify_terminal` and emit `ContextOverflow` when matched, route `overloaded_error`/`rate_limit_error` through `ProviderRequest`, and only return `ProviderStream` for genuinely transient transport loss.
- **Reference**: https://platform.claude.com/docs/en/api/messages-streaming#error-events, `others/opencode/packages/llm/src/protocols/anthropic-messages.ts:710-722`

### [CRITICAL] `FunctionCallOutput.output` always stringified — tool results with image bytes silently bloat context
- **Location**: `crates/squeezy-llm/src/anthropic.rs:373-381`
- **Observed**: `tool_result.content` is always the raw `output: String`. Anthropic accepts a string OR an array of `text`/`image` blocks; squeezy never uses the array form.
- **Issue**: An MCP tool returning a 1280×800 PNG (≈333KB base64) ships as `content: "<huge base64>"` — ~110k tokens of garbage text, plus the model can't see the image because it's not an `image` block.
- **Impact**: One screenshot tool result halves effective context; vision-capable models can't see the image they think they have; cost balloons.
- **Fix sketch**: Extend `LlmInputItem::FunctionCallOutput` (or add a structured-content variant); emit `content: Array<{type:"image", source:{type:"base64", media_type, data}}>` for image bytes.
- **Reference**: https://platform.claude.com/docs/en/build-with-claude/tool-use, `others/opencode/packages/llm/src/protocols/anthropic-messages.ts:101-114`

### [HIGH] Auto cache breakpoints can exceed Anthropic's 4-marker cap when callers contribute markers
- **Location**: `crates/squeezy-llm/src/anthropic.rs:236-239`, `crates/squeezy-llm/src/cache_policy.rs:206-256`
- **Observed**: When `should_apply_caching` is true we unconditionally stamp `cache_control` on system tail + last user block + last stable tool — three breakpoints, no counter.
- **Issue**: Anthropic returns 400 (`invalid_request_error: cache_control breakpoint limit exceeded`) above 4 markers/request. A single future caller-supplied marker (skill-loaded tool def, future system layering, multi-system blocks) pushes the count to 5 and 4xx-s every turn non-retryably.
- **Impact**: Latent foot-gun; lights up when any non-built-in path attaches a marker.
- **Fix sketch**: Mirror opencode's `Cache.Breakpoints` slot allocator (4 slots, invalidation-priority order: tools → system → messages, decrement + drop-and-warn when exhausted).
- **Reference**: https://platform.claude.com/docs/en/docs/build-with-claude/prompt-caching, `others/opencode/packages/llm/src/protocols/anthropic-messages.ts:230-247`

### [HIGH] `redacted_thinking.data` is not accumulated across deltas — round-trip emits empty data
- **Location**: `crates/squeezy-llm/src/anthropic.rs:873-887`; replay at `408-414`; signature_delta path at `944-957`
- **Observed**: We capture `data` only from `content_block_start.content_block.data`. Late-arriving redacted content streams through `signature_delta`, which lands in `block.signature` while the replay JSON reads `block.data`. Result: empty `data: ""` on replay.
- **Issue**: Anthropic requires thinking blocks to be passed back unchanged for multi-turn reasoning continuity. An empty `data` either errors with `invalid_request_error` or silently breaks safety reasoning across tool-call turns.
- **Impact**: Multi-turn safety-sensitive sessions on Claude 4+ lose reasoning continuity.
- **Fix sketch**: For `AnthropicThinkingKind::Redacted`, treat `signature_delta.signature` as `data` accumulation; write whichever is populated on replay.
- **Reference**: https://platform.claude.com/docs/en/build-with-claude/extended-thinking#multi-turn-conversations-with-thinking

### [HIGH] `reasoning_only_stop` always emitted as `false` — adaptive-thinking blank turns invisible to the agent loop
- **Location**: `crates/squeezy-llm/src/anthropic.rs:1024-1029`; semantics at `lib.rs:599-612`
- **Observed**: Provider hard-codes `reasoning_only_stop: false` on every `Completed` event.
- **Issue**: When adaptive thinking on Opus 4.7/4.8 with `display: "omitted"` thinks but emits no text, the stream finishes `end_turn` + non-empty `state.finished_thinking` + zero visible output. The contract at `lib.rs:599-612` says this should yield `reasoning_only_stop: true` so the agent retries; today the signal is dead.
- **Impact**: Blank "thinking-only" turns pass through silently instead of triggering retry.
- **Fix sketch**: `let reasoning_only_stop = matches!(stop_reason, Some(StopReason::EndTurn)) && !saw_visible_output && !state.finished_thinking.is_empty();`
- **Reference**: `crates/squeezy-llm/src/lib.rs:599-612`

### [HIGH] `model_uses_adaptive_thinking` matches non-Claude model ids by substring
- **Location**: `crates/squeezy-llm/src/anthropic.rs:54-68`
- **Observed**: `extract_claude_version` searches for `opus-` / `sonnet-` anywhere in the lowercased model id.
- **Issue**: A custom Anthropic-compatible proxy using model ids like `opus-4-7`, `vertex/anthropic/claude-opus-4-7@001`, `anthropic/claude-opus-4-7:nitro`, or a model literal that just happens to contain `sonnet-5-0` activates `thinking.type="adaptive"` + `output_config.effort` + the EFFORT beta. Proxies generally reject these fields.
- **Impact**: Aggregator routes silently 400 on adaptive bodies they don't recognise.
- **Fix sketch**: Anchor on `claude-opus-` / `claude-sonnet-` and require the version digits to be followed by `-`, `@`, `:` or end-of-string.

### [HIGH] Pre-stream HTTP 4xx with `[non-retryable]` marker still gets retried by `with_stream_retry`
- **Location**: `crates/squeezy-llm/src/anthropic.rs:568-570`; filter at `retry.rs:583-588`
- **Observed**: `format_for_provider_error` prefixes `[non-retryable] ` for hard 4xx bodies; the value rides on `SqueezyError::ProviderRequest`. `is_retryable_stream_error` matches both `ProviderStream` and `ProviderRequest`.
- **Issue**: The marker is a TUI rendering hint, never inspected by the retry layer. A 400 (`invalid_request_error: thinking.enabled.budget_tokens must be >= 1024`) gets retried 5× against an immutable failure.
- **Impact**: Hard-config-mistake 400s waste ~10s of backoff before surfacing.
- **Fix sketch**: Either have `is_retryable_stream_error` strip and check the marker, or introduce `SqueezyError::ProviderRequestNonRetryable`.

### [HIGH] Auth-retry layer always retries `StaticApiKey` once on 401 even though it can't rotate
- **Location**: `crates/squeezy-llm/src/retry.rs:79-102`; `StaticApiKey::invalidate` no-op at `credentials.rs:452-454`
- **Observed**: `send_with_auth_retry` calls `invalidate()`, re-reads the key, sends again — always.
- **Issue**: For `StaticApiKey` the second call returns the same dead key, doubling the load on revoked-key paths and slowing the 401 surface. For `AnthropicOAuthSource`, the failure-loop case is also broken: if `force_refresh` errors, `dirty` is never cleared so the *next* `current_key` re-fires the failed network call.
- **Impact**: Doubled 401 round-trip on revoked keys; confused error surface on revoked refresh tokens.
- **Fix sketch**: Add `ApiKeySource::can_rotate() -> bool` (false for `StaticApiKey`); skip the auth retry when `!can_rotate()`. In `force_refresh`, on error keep `dirty=true` *and* short-circuit re-entry via a `last_refresh_err` flag the next caller can observe.

### [MEDIUM] No `pause_turn` mapping — Anthropic's resume-the-turn signal falls into `Other`
- **Location**: `crates/squeezy-llm/src/lib.rs:498-510`
- **Observed**: `from_anthropic` handles canonical variants; `pause_turn` lands in `Other("pause_turn")`.
- **Issue**: Long extended-thinking turns now pause and expect the client to send a `continue` no-op. `Other` is treated as "unknown finish" — turn ends prematurely.
- **Fix sketch**: Add `StopReason::PauseTurn` and a recovery hook in the agent loop.
- **Reference**: https://platform.claude.com/docs/en/api/messages-streaming, `others/opencode/packages/llm/src/protocols/anthropic-messages.ts:473-479`

### [MEDIUM] `Retry-After: <http-date>` and sub-second values silently rejected, falling back to default backoff
- **Location**: `crates/squeezy-llm/src/retry.rs:335-344`
- **Observed**: Tries `retry-after-ms` as u64, then `retry-after` as u64 seconds. No HTTP-date or float path.
- **Issue**: RFC 7231 allows `Retry-After: Wed, 21 Oct 2026 07:28:00 GMT`; floats like `0.5` are rejected. Today Anthropic uses integer seconds (low risk), but non-Anthropic proxies emit other shapes.
- **Fix sketch**: Try u64 → f64 (clamp to ms) → `httpdate::parse_http_date`.

### [MEDIUM] No `.connect_timeout()` / `.tcp_keepalive()` on the shared HTTP client
- **Location**: `crates/squeezy-llm/src/transport.rs:96-110`
- **Observed**: Only `pool_max_idle_per_host` + `pool_idle_timeout` are set.
- **Issue**: A stuck TLS handshake (captive portal, draconian DoH) leaves `send().await` hanging until the user ctrl-c's; the idle timeout only kicks in after the first byte. Anthropic's docs explicitly recommend TCP keep-alive.
- **Fix sketch**: Add `.connect_timeout(Duration::from_secs(30))` and `.tcp_keepalive(Duration::from_secs(60))`.
- **Reference**: https://platform.claude.com/docs/en/api/errors#long-requests

### [MEDIUM] `tool_use.input` accumulator can corrupt a future zero-arg tool call with non-empty initial input
- **Location**: `crates/squeezy-llm/src/anthropic.rs:824-834`
- **Observed**: We use `input` as seed only when it's a non-empty object; then `input_json_delta` accumulates via `push_str`.
- **Issue**: Today benign (Anthropic always sends `input: {}` then streams). If a future server build emits the complete input upfront for a cached zero-arg tool, we'd start with `{}` then push `{"a":1}`, producing `{}{"a":1}` and an invalid-JSON failure at `content_block_stop`.
- **Fix sketch**: Track a `bool` once a delta arrives, ignoring any non-empty initial seed; or defer all input parsing to stop.

### [MEDIUM] OAuth path doesn't send `anthropic-dangerous-direct-browser-access` header that Claude Code emits
- **Location**: `crates/squeezy-llm/src/anthropic.rs:520-527`
- **Observed**: Sets authorization, user-agent, x-app — missing the dangerous-direct-browser-access header.
- **Issue**: Claude Code's OAuth requests carry this marker for current platform policy. Without it, future policy changes may reject squeezy OAuth requests as "browser policy not acknowledged".
- **Fix sketch**: Add the header for the OAuth arm.

### [MEDIUM] No `max_tokens` clamp against registry-known per-model maxima
- **Location**: `crates/squeezy-llm/src/anthropic.rs:152-160`, registry value at `models.json:115`
- **Observed**: Use `request.max_output_tokens` → registry `max_output_tokens` → `DEFAULT_ANTHROPIC_MAX_OUTPUT_TOKENS = 64_000`. No `min()`.
- **Issue**: A user copying `max_output_tokens = 128000` from an OpenAI config sends 128k to Anthropic and gets a hard 400.
- **Fix sketch**: Clamp `request.max_output_tokens.min(registry_max)` when both are known.

### [MEDIUM] OAuth proactive refresh effectively starts 6 minutes before real expiry, not the documented 60s
- **Location**: `crates/squeezy-llm/src/oauth/anthropic.rs:112-123`, `613-624`
- **Observed**: Persisted `expires_at_unix_ms` is already shifted *back* by `REFRESH_LEAD_TIME = 5min`; the runtime check then subtracts another 60s.
- **Issue**: Refresh fires 6 minutes early rather than the 60s the comment claims. Safe (no harm) but misleading.
- **Fix sketch**: Pick one shift location; document accurately.

### [LOW] SSE decoder uses `String::from_utf8_lossy` per event — silently replaces malformed bytes with U+FFFD
- **Location**: `crates/squeezy-llm/src/sse.rs:49-63`
- **Issue**: Today benign (Anthropic JSON is ASCII-safe), but masks future malformed-byte bugs.
- **Fix sketch**: Use `std::str::from_utf8` and surface decode errors.

### [LOW] `tool_choice` is never forwarded to the Anthropic body
- **Location**: `crates/squeezy-llm/src/anthropic.rs:144-242`; field documented at `lib.rs:155-159` as OpenAI-only
- **Issue**: Anthropic supports `tool_choice: {type:"auto"|"any"|"tool", name}` plus `disable_parallel_tool_use`. No way to force a tool.
- **Fix sketch**: Map `Some("auto"|"required"|"tool:X")` to the Anthropic shape.
- **Reference**: `others/opencode/packages/llm/src/protocols/anthropic-messages.ts:264-270`

### [LOW] API-key path sends no `User-Agent`, identifying as bare reqwest
- **Location**: `crates/squeezy-llm/src/anthropic.rs:517-525`
- **Issue**: Anthropic uses UA for rate-limit attribution and analytics; bare reqwest is unattributed.
- **Fix sketch**: Stamp `squeezy-cli/<version>` for the API-key arm.

### [LOW] `merge_oauth_beta_header` dedup is case-sensitive
- **Location**: `crates/squeezy-llm/src/anthropic.rs:272-294`
- **Issue**: `Claude-code-20250219` and `claude-code-20250219` both kept; Anthropic is case-insensitive.
- **Fix sketch**: Compare lowercased.

### [LOW] Connection-pool cache never evicts entries for distinct `ProviderTransportConfig` values
- **Location**: `crates/squeezy-llm/src/transport.rs:68-89`
- **Issue**: Long-running TUI sessions can leak clients if config mutates per-skill (not today, latent).
- **Fix sketch**: LRU cap.

### [NIT] Stop-reason logger drops unknown values into `Other` silently
- **Location**: `crates/squeezy-llm/src/lib.rs:498-510`
- **Fix sketch**: `tracing::warn!(provider="anthropic", stop_reason=%value, "unknown stop_reason");` on the `Other` branch.

### [NIT] `anthropic_costly` smoke test never exercises the OAuth path
- **Location**: `crates/squeezy-llm/tests/anthropic_costly.rs:18-86`
- **Fix sketch**: Add a sibling test gated on `SQUEEZY_RUN_OAUTH_COSTLY_TESTS=1` + presence of `~/.squeezy/auth/anthropic.json`.

### [NIT] Server-tool result blocks (`web_search_tool_result`, `code_execution_tool_result`, `web_fetch_tool_result`) silently discarded
- **Location**: `crates/squeezy-llm/src/anthropic.rs:889`
- **Issue**: Catch-all returns `none()`. If a user enables Anthropic server tools, responses are dropped.
- **Fix sketch**: Log+warn at minimum; long-term map to a marker variant.
- **Reference**: `others/opencode/packages/llm/src/protocols/anthropic-messages.ts:534-561`

### [NIT] Verified — areas checked and OK
- Verified: SSE decoder splits on `\n\n`/`\r\n\r\n` and does not look for `[DONE]` (Anthropic doesn't emit one). `sse.rs:36-47`.
- Verified: Cancellation aborts the streaming select via `tokio::select! { _ = cancel.cancelled() => yield Cancelled; return; }`. `anthropic.rs:587-594`.
- Verified: `merge_usage` correctly accumulates Anthropic's "final delta wins" via `usage.field.or(state.field)`. `anthropic.rs:1044-1065`.
- Verified: `AnthropicStreamState::cost()` correctly converts uncached-delta convention into total-prompt-tokens. `anthropic.rs:733-757`.
- Verified: OAuth token storage uses atomic write-temp + rename + mode 0600. `oauth/anthropic.rs:394-435`.
- Verified: Mid-stream reconnect dedupes already-yielded text by char count. `anthropic_stream_retry.rs:194-231`, `retry.rs:407-484`.
- Verified: `redacted_thinking` round-trip *kind* preserved (modulo data-loss finding above).
- Verified: Server-echoed `message.model` correctly drained pre-event-emit so `ServerModel` lands on the first frame. `anthropic.rs:606-610`.

## Test Coverage Gaps
- **Mid-stream `event: error` after 200** [CRITICAL] — mockable; verify provider emits `ProviderRequest` + `ContextOverflow`, not retried.
- **`redacted_thinking` round-trip via `signature_delta`** [HIGH] — mockable; assert replay JSON includes full encrypted data.
- **Adaptive thinking + zero text → `reasoning_only_stop=true`** [HIGH] — mockable.
- **4-breakpoint cap exhaustion** [HIGH] — request-body assertion only.
- **Tool result with image bytes → `tool_result.content: Array<image>`** [CRITICAL] — mockable (post-shape-change).
- **OAuth `force_refresh` failure doesn't loop on next `current_key`** [HIGH] — mockable via stub token URL.
- **Non-Claude model id with `opus-4-7` substring does NOT activate adaptive thinking** [HIGH] — assertion only.
- **`[non-retryable]` pre-stream 400 triggers zero stream retries** [HIGH] — mockable.
- **`pause_turn` stop reason → `PauseTurn` variant** [MEDIUM] — drop-in mockable.
- **Concurrent `current_key()` racing through `force_refresh()` only issues one POST** [MEDIUM] — mockable with counter.
- **`Retry-After: <http-date>` correctly parsed** [MEDIUM] — assertion only.
- **Connection-timeout fires on stuck TLS handshake** [MEDIUM] — TCP-accept-no-handshake mock.

## Verification Strategy
All findings are reproducible without a paid Anthropic key. Three patterns cover the gaps:

1. **Mock TCP/SSE server**: `anthropic_stream_retry.rs:54-121` already shows the pattern — `TcpListener::bind("127.0.0.1:0")`, write chunked HTTP + hand-crafted SSE. Extend with a behaviour enum (`DropMidStream | Return400 | ReturnMidStreamError(error_type) | ReturnOverloaded | ...`) so every error path has a deterministic reproducer.

2. **Mock OAuth token endpoint**: `AnthropicOAuthSource::with_parts` (`oauth/anthropic.rs:475-491`) already accepts a custom `AnthropicLoginConfig`. Point `token_url` at a `tokio::net::TcpListener` returning canned 200/4xx so refresh semantics, concurrency, and persistence-on-success are covered.

3. **Body-shape unit tests**: `anthropic_tests.rs` already uses `AnthropicProvider::request_body(&request, AnthropicAuthScheme::ApiKey)` then inspects `body[...]`. Cover the breakpoint counter, model-id heuristic, max_tokens clamp, and `tool_use` accumulator invariants with no network. For end-to-end coverage of mid-stream behaviours, the TCP mock plus a small state machine over canned SSE frames is sufficient — squeezy already proves this pattern works for the reconnect tests.

A fixture-recording helper (capture once with a paid key into `tests/fixtures/anthropic_stream_*.jsonl`, replay through `parse_anthropic_event` for unit tests) would expand coverage cheaply. Pattern matches `others/opencode/packages/llm/test/provider/anthropic-messages.recorded.test.ts`.

## References
- https://platform.claude.com/docs/en/api/messages-streaming — SSE event types, `pause_turn`, signature_delta, mid-stream `error` events
- https://platform.claude.com/docs/en/api/errors — error envelope, HTTP code semantics, `request_id` header
- https://platform.claude.com/docs/en/docs/build-with-claude/prompt-caching — 4-breakpoint cap, 5m vs 1h TTL, usage fields, min cacheable token floor per model
- https://platform.claude.com/docs/en/api/rate-limits — `anthropic-ratelimit-*` headers, cache-aware ITPM, `retry-after` semantics
- https://platform.claude.com/docs/en/build-with-claude/extended-thinking — `thinking.type=adaptive` vs `enabled`, `display: omitted`, signature_delta, multi-turn unchanged-passback requirement
- `crates/squeezy-llm/src/anthropic.rs` — squeezy implementation under audit
- `crates/squeezy-llm/src/retry.rs` — shared retry / auth-retry layer
- `crates/squeezy-llm/src/oauth/anthropic.rs` — OAuth source implementation
- `crates/squeezy-llm/src/sse.rs` — shared SSE decoder
- `crates/squeezy-llm/src/cache_policy.rs` — cache breakpoint marker placement
- `crates/squeezy-llm/src/overflow.rs` — triple-path overflow classifier
- `crates/squeezy-llm/src/anthropic_error.rs` — error envelope humaniser + non-retryable marker
- `crates/squeezy-llm/tests/anthropic_stream_retry.rs` — TCP mock SSE pattern
- `others/opencode/packages/llm/src/protocols/anthropic-messages.ts:101-114` — `tool_result.content` array form
- `others/opencode/packages/llm/src/protocols/anthropic-messages.ts:230-247` — `Cache.Breakpoints` 4-slot allocator
- `others/opencode/packages/llm/src/protocols/anthropic-messages.ts:264-270` — `lowerToolChoice`
- `others/opencode/packages/llm/src/protocols/anthropic-messages.ts:473-479` — finish-reason map including `pause_turn`
- `others/opencode/packages/llm/src/protocols/anthropic-messages.ts:534-561` — server-tool result block handling
- `others/opencode/packages/llm/src/protocols/anthropic-messages.ts:710-722` — mid-stream `error` event handling
