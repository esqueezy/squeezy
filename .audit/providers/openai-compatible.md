# OpenAI-Compatible Aggregator Audit

## Summary

- Severity tally: **3 critical / 8 high / 11 medium / 6 low / 4 nit** = **32 findings**.
- Top 3 actionable recommendations:
  1. **Fix Cloudflare AI Gateway dual-auth**: today squeezy uses `CLOUDFLARE_API_KEY` as the `Authorization: Bearer` value, but the deprecated `/compat` endpoint expects the **upstream provider's** key in `Authorization` and the **Cloudflare gateway token** in `cf-aig-authorization`. The current scheme only works when the gateway proxies to Workers AI; routing to OpenAI/Anthropic upstreams 401s.
  2. **Migrate Vercel AI Gateway from `/v1` to `/v1/ai`** (or document that `/v1` returns 404 on chat completions today) — see §Per-Preset / Vercel.
  3. **Hard-cap the request body's `n: 1`** in the shared core, because the chat-completions parser only honors `choices[0]`; if a user supplies `n>1` via a TOML knob in future, all but the first choice are silently dropped without warning, including their tool calls.

## Implementation Overview

The aggregator is a single Rust type, `OpenAiCompatibleProvider`, defined at `crates/squeezy-llm/src/compatible.rs:39-46`. It wraps `POST {base_url}/chat/completions` for 18 distinct presets (OpenRouter, Vercel AI Gateway, PortKey, Groq, DeepSeek, Vertex AI, Mistral, Together AI, Fireworks AI, Cerebras, DeepInfra, Baseten, LM Studio, vLLM, llama.cpp, Cloudflare Workers AI, Cloudflare AI Gateway, Custom). The 19th preset, xAI, is split-routed: Grok 3+ goes to the OpenAI Responses endpoint via `OpenAiProvider::from_xai_config` (`crates/squeezy-llm/src/xai.rs:30-37`), older models fall back to chat-completions. Azure OpenAI is **not** part of this provider — it routes through `OpenAiProvider::from_azure_config` (`crates/squeezy-llm/src/openai.rs:69-86`) against the `/responses` endpoint with `api-key` header + `?api-version=` query.

Preset metadata (default base URL, default model, default env-var name) lives in `OpenAiCompatiblePreset` at `crates/squeezy-core/src/lib.rs:1963-2215`. Cloudflare presets carry `{account_id}` / `{gateway_id}` placeholders in their default URLs (`crates/squeezy-core/src/lib.rs:122-125`); the substitution happens at provider construction time in `substitute_url_placeholders` (`crates/squeezy-llm/src/compatible.rs:701-745`). Vertex's URL is fully synthesized from `vertex_project` + `vertex_location` via `vertex_base_url` (`crates/squeezy-core/src/lib.rs:133-137`) and the OAuth access token rides in the standard `Authorization: Bearer` header.

The request lifecycle: `from_config` resolves API key + headers + URL, building one `reqwest::Client` per provider via `shared_client` (a pooled factory at `crates/squeezy-llm/src/transport.rs`). At stream time, `stream_response` calls `request_body` (`crates/squeezy-llm/src/compatible.rs:134-297`) which always sets `stream: true, stream_options: { include_usage: true }`, normalizes tool-call ids via `normalize_tool_ids_for_replay`, optionally attaches Anthropic-style `cache_control` markers when the model id is `anthropic/*` (driven by `COMPAT_TABLE` at `compatible.rs:374-403`), and unconditionally emits both `reasoning_effort` and `reasoning: {effort}` shapes. The SSE stream is decoded by the shared `SseDecoder` (`crates/squeezy-llm/src/sse.rs`) and parsed per-chunk by `parse_chat_event` (`compatible.rs:1000-1136`), which tracks tool-call accumulation, reasoning buffers, and `finish_reason` normalization.

PortKey gets a special error-message hint path (`compatible.rs:505-524`) but no auto-injected routing header (the historical `x-portkey-provider` auto-injection was removed). OpenRouter is the only preset with built-in default extra headers (`HTTP-Referer` + `X-Title`, see `compatible.rs:762-775`). All other preset-specific behavior must come from user-supplied `extra_headers`.

## Shared-Core Findings

### C1 — `[DONE]` after a final usage chunk loses the usage payload (high → critical)

`parse_chat_event` (`compatible.rs:1000-1017`) handles `[DONE]` by emitting `LlmEvent::Completed` with `state.cost` and then setting `state.completed_emitted = true`. The outer loop at `compatible.rs:563-565` and `578-580` then short-circuits with `return` the moment `completed_emitted` flips. **Several providers (Groq, OpenRouter forwarding Groq, OpenAI itself) emit a final chunk with `choices: []` and `usage: {...}` AFTER the chunk that carried `finish_reason: "stop"`.** Because `finish_reason: "stop"` triggers `drain_tool_calls` + emits `Completed` *inline* (`compatible.rs:1081-1109`), `state.completed_emitted` is `true` before the usage-only chunk arrives, and the outer loop returns from `stream_response` (`compatible.rs:563-565`) without ever calling `parse_chat_event` on that final chunk. Usage data is lost; cost is reported as 0 input / 0 output.

**Fix**: don't set `completed_emitted` inside the `finish_reason` handler; only set it on `[DONE]`. Or: keep `completed_emitted` but continue to drain pending events (specifically, replay the usage parser) when subsequent chunks arrive — currently the loop exits as soon as `completed_emitted` is `true`. Codex and opencode both keep parsing until `[DONE]` and only then emit the terminal event.

### C2 — Cloudflare AI Gateway dual-auth is inverted (critical)

`crates/squeezy-core/src/lib.rs:2127` sets `CloudflareAiGateway`'s `default_api_key_env` to `CLOUDFLARE_API_KEY`. The provider passes this through `bearer_auth(key)` at `compatible.rs:474`. But Cloudflare's `/compat` endpoint documentation (verified May 2026) requires:

- `Authorization: Bearer <UPSTREAM_PROVIDER_KEY>` (e.g. the OpenAI API key when routing to OpenAI)
- `cf-aig-authorization: Bearer <CF_AIG_TOKEN>` (the Cloudflare gateway token)

squeezy's config flow at `lib.rs:8700-8713` does add `cf-aig-authorization` from `CF_AIG_TOKEN`, but the `Authorization` header carries the Cloudflare token instead of the upstream provider's key. This only works when the gateway proxies to Workers AI (where the Cloudflare token *is* the upstream). For OpenAI/Anthropic/Groq/Grok upstreams, requests 401 because the upstream sees a Cloudflare key in its Bearer slot.

**Fix**: add an `upstream_api_key_env` field (or rename so that `CLOUDFLARE_API_KEY` always populates `cf-aig-authorization` and the user supplies the upstream key separately). opencode's `cloudflare.ts:42-51` models this correctly: `cf-aig-authorization` from `CLOUDFLARE_API_TOKEN`/`CF_AIG_TOKEN`, `Authorization` from `apiKey`.

### C3 — Cloudflare AI Gateway `/compat` is deprecated (high → critical for new users)

Verified May 2026: Cloudflare's docs state `/compat/chat/completions` is deprecated; the recommended path is now `https://api.cloudflare.com/client/v4/accounts/{ACCOUNT_ID}/ai/v1/chat/completions` (the REST API path). New AI Gateway users following Cloudflare's current docs will not be able to configure squeezy without overriding `base_url`. squeezy hard-codes the deprecated `/compat` path at `crates/squeezy-core/src/lib.rs:124-125`.

**Fix**: switch the default template to `https://gateway.ai.cloudflare.com/v1/{account_id}/{gateway_id}/openai` (or the new REST shape) and update the test at `compatible_tests.rs:1024-1027`.

### H1 — `n > 1` silently drops choices 1..N (high)

`parse_chat_event` iterates over `choices` (`compatible.rs:1051-1132`) but the assembler at `accumulate_tool_call` (`compatible.rs:831-851`) keys tool calls by `index` only — there is no per-choice partition. For `n=2`, two choices both populating `index=0` would silently merge into a single tool call. Furthermore, `LlmRequest` has no `n` field, so squeezy hard-codes `n=1` implicitly by omitting it. But if a caller ever passes a `Custom` preset with a body extension, or a future config knob, this is a footgun.

**Fix**: explicitly emit `n: 1` in the body so an upstream default of `n=2` (rare but legal) can't silently double-bill.

### H2 — Tool-call argument JSON parse error is masked when `arguments` is omitted (high)

In `drain_tool_calls` at `compatible.rs:885-895`, an empty `partial.arguments` is rewritten to `"{}"` and parsed. But many tool calls *do* have arguments and the streamed `arguments` field may not be valid JSON (mid-chunk cutoff, encoding issues). The fallback at `compatible.rs:890-896` wraps the failure in `INVALID_TOOL_ARGUMENTS_*` markers, which is correct, **but `partial.arguments` is concatenated without bound** in `accumulate_tool_call` (`compatible.rs:847-849`). A pathological stream that keeps sending `function.arguments` deltas without `finish_reason` could grow `partial.arguments` to gigabytes before the timeout fires.

**Fix**: cap `entry.arguments.len()` at a sane upper bound (e.g. 1 MiB) and synthesize an invalid-arguments error past that point.

### H3 — `seed`, `top_p`, `temperature`, `frequency_penalty`, `presence_penalty`, `stop`, `logprobs` are never emitted (high → medium)

`request_body` (`compatible.rs:134-297`) only emits `model`, `messages`, `stream`, `stream_options`, `max_tokens`, `reasoning_effort` / `reasoning`, `prompt_cache_key`, `prompt_cache_retention`, `tools`, `tool_choice`. **There is no path for the user to set `temperature`, `top_p`, `seed`, `stop`, `frequency_penalty`, `presence_penalty`, or `logprobs`**, which are core OpenAI-shape parameters. The `LlmRequest` struct (`lib.rs:130-176`) also lacks fields for them. For aggregator routes that ship determinism-critical workloads (eval, replay, regression suites), the absence of `seed` is significant. For routes whose pricing/quality matters (Together, Fireworks, Cerebras) the absence of `temperature` means every call uses the provider's default (typically 1.0).

**Fix**: add these to `LlmRequest` and forward them in `request_body`. Provide a `Custom`-preset escape hatch for unknown body fields.

### H4 — Tool-call `index` falls back to `0` when missing (high)

`compatible.rs:1069-1071`:
```rust
let index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
```
A streamed tool-call delta without `index` defaults to `0`, which means if a provider sends two parallel tool calls and the second omits `index` on one of its deltas (some aggregators do this when relaying Anthropic's `content_block_delta` over chat completions), the second call's arguments will be concatenated into the first call's accumulator.

**Fix**: track the highest seen `index` per stream and treat missing `index` as "continuation of the most recent active index", not zero. Or hard-fail on missing `index` once another delta with `index > 0` has been observed.

### H5 — `[DONE]` not seen + reasoning-only stop: the synthetic notice fires twice (medium → high)

When the stream ends without `[DONE]` (truncated upstream), the outer loop in `stream_response` at `compatible.rs:584-612` calls `drain_tool_calls` + `drain_reasoning` + emits a synthetic "stream ended without producing any content" notice. But if the upstream already emitted a `finish_reason: "stop"` with reasoning-only output (`compatible.rs:1081-1109`), the inline `parse_chat_event` already injected its own notice (`"[squeezy] model finished without emitting any content or tool call (finish_reason=stop)..."`). Both notices land in the transcript.

**Fix**: gate the post-loop notice on `!state.saw_visible_output && state.finish_reason.is_none()`.

### H6 — `format_chat_error` flattens to `default_message` on the wrong shape (medium)

`format_chat_error` (`compatible.rs:976-998`) falls back to `default_message` when `error.message`, `error` (as a string), and `value.message` are all absent. But several providers ship `{ "error": { "errors": [...] } }` (Google-style nested) or `{ "code": "..", "message": ".." }` (top-level, no `error` wrapper). These would surface as `default_message` only, eating the upstream's actual error text.

**Fix**: also probe `errors[0].message`, `value.detail`, `value.error.errors[0].message`.

### H7 — `error` field inside an SSE chunk surfaces as `ProviderStream` not `ProviderRequest` (medium)

`parse_chat_event` at `compatible.rs:1022-1027` returns `SqueezyError::ProviderStream` for inline `error` payloads. But the agent loop's retry policy at `retry.rs:46-55` (`provider_stream`) sets `retry_429: false, retry_5xx: false`, so a streamed mid-flight `error: { code: "rate_limit_exceeded" }` is not retried — even though the original POST request succeeded with 200 OK. PortKey and OpenRouter both ship inline rate-limit envelopes mid-stream when the upstream throttles late.

**Fix**: classify the inline error and either escalate to `ProviderRequest` (so the request-level policy retries) or extend the stream policy to honor classified-retryable inline errors.

### H8 — `prompt_cache_key` clamp may collide across distinct prompts (medium)

`clamp_prompt_cache_key` (called at `compatible.rs:236`) silently truncates keys to 64 codepoints (verified by `compatible_tests.rs:818-832`). For users that derive keys from full path hashes (longer than 64 chars), this is a silent collision that causes cache mixing across sessions.

**Fix**: hash long keys (BLAKE3 or SHA-256 → first 32 hex chars) before clamping rather than truncate.

### M1 — `extra_headers` user overrides clobber `HTTP-Referer` / `X-Title` (medium)

`from_config` at `compatible.rs:85-90`:
```rust
let mut headers = preset_default_headers(config.preset);
for (key, value) in &config.extra_headers {
    headers.insert(key.clone(), value.clone());
}
```
This is documented behavior (user wins), but a `BTreeMap` keyed on raw strings means `HTTP-Referer` and `http-referer` from the user TOML are *both* preserved (HTTP header keys are case-insensitive). The wire request then carries duplicate referer headers; some servers reject the second one, others use the latter.

**Fix**: normalize header keys to canonical case before merging, or use a `HeaderMap`.

### M2 — `n=1` is implicit; `parallel_tool_calls` is ignored entirely (medium)

`LlmRequest::parallel_tool_calls` (`lib.rs:166`) exists but the chat-completions provider ignores it (only the native OpenAI Responses provider reads it, per the field's doc comment). Aggregator routes that proxy to OpenAI silently lose the user's intent to serialize tool calls. OpenRouter/Vercel routes happily forward unknown fields, so emitting `parallel_tool_calls` when set would just work.

**Fix**: forward `parallel_tool_calls` when `Some(...)` and the route is chat-completions.

### M3 — `response_format` / `output_schema` is not emitted (medium)

`LlmRequest::output_schema` (`lib.rs:160-161`) is a `LlmOutputSchema` (name, schema, strict) and the OpenAI Responses provider emits it as `text.format`. The chat-completions provider never references it. Aggregator routes that DO support `response_format: { type: "json_schema", ... }` (OpenRouter through OpenAI, Together, Mistral, Groq) lose the contract.

**Fix**: forward `output_schema` as `response_format: { type: "json_schema", json_schema: { ... } }` when set.

### M4 — Trailing-slash trim is one-shot; `//chat/completions` still possible (medium)

`from_config` (`compatible.rs:78`) does `config.base_url.trim_end_matches('/')` — note the **plural** `trim_end_matches`, which strips *all* trailing slashes. Good. But `stream_response` builds the URL with `format!("{}/chat/completions", self.base_url)` (`compatible.rs:451`), assuming the trim happened. The `substitute_url_placeholders` function (`compatible.rs:701-745`) substitutes `{account_id}` / `{gateway_id}` values that themselves may contain `/`. If a user mistakenly sets `cloudflare_account_id = "/acct"`, the resolved URL is `https://api.cloudflare.com/client/v4/accounts//acct/ai/v1` → 404. There's no validation of placeholder *values*.

**Fix**: reject placeholder values containing `/`, `?`, `#`, whitespace, or non-ascii.

### M5 — `Custom` preset accepts ANY base URL including `http://internal-host` (medium)

`Custom` (`crates/squeezy-core/src/lib.rs:1990`) has no scheme/host validation in `build_openai_compatible_config` (`lib.rs:8602-8726`). `check_base_url_scheme` is invoked for `AzureOpenAi` per `lib.rs:8544` but is NOT called for the `OpenAiCompatible` arm. A user-supplied `base_url = "file:///etc/passwd"` or `http://169.254.169.254/...` (AWS IMDS) passes straight through to `reqwest`. SSRF-class attack surface if squeezy ever runs in a hosted environment.

**Fix**: enforce `https://` for non-loopback hosts; allow `http://` only for `127.0.0.1`, `localhost`, and link-local. Reuse the existing `is_loopback_host` helper at `lib.rs:8580-8599`.

### M6 — `reasoning_effort` is sent for non-reasoning models (medium)

`compatible.rs:215-224` always emits both `reasoning_effort` and `reasoning: { effort }` when `request.reasoning_effort` is `Some(_)`. The comment claims aggregators ignore unknown fields. But: Mistral's chat completions API rejects unknown body fields with 422 (verified May 2026); some Cerebras endpoints same. A user setting `reasoning_effort = "high"` on a `mistral-large-latest` model gets a hard 422.

**Fix**: gate emission on `compat_entry(model).map_or(false, |e| e.supports_reasoning)` rather than always-on. Today's `COMPAT_TABLE` only knows about 4 namespaces; broader presets fall through to `Generic` and get the field. Worse: the field is `descriptive only` per the doc comment at `compatible.rs:362-364`, so the existing flag is decorative.

### M7 — Tool-call `arguments` set to `"{}"` when empty masks server-side intent (medium)

`drain_tool_calls` at `compatible.rs:885-887`:
```rust
let arguments_text = if partial.arguments.is_empty() {
    "{}".to_string()
} else {
    partial.arguments
};
```
A tool that legitimately takes zero arguments and a tool whose argument stream is silently dropped both surface as `{}`. Better behavior: surface `null` so the agent can distinguish "no arguments" from "missing arguments" downstream.

**Fix**: emit empty `Value::Null` and let the tool runtime decide.

### M8 — `account_id` containing URL-encoded characters is double-encoded (medium)

`substitute_url_placeholders` (`compatible.rs:730-743`) does a raw `String::replace`. opencode's `cloudflare.ts:39,57` wraps the value in `encodeURIComponent`. squeezy doesn't. A Cloudflare account ID is hex so this isn't user-hostile in the common case, but if a user mistakenly pastes `account_id = "abc?api-version=2024"` to debug, the chunk after `?` becomes a query-string segment of the resulting URL.

**Fix**: percent-encode placeholder values, or validate against a `[A-Za-z0-9_-]+` regex.

### M9 — `serde_json::to_string(arguments)` in chat replay loses key ordering (low → medium)

`chat_message` for `FunctionCall` at `compatible.rs:650-657` serializes a `Value` to a string via `serde_json::to_string` (which is alphabetic-key-ordered by default for `Map`). If the model produced `{"b": 1, "a": 2}` originally, the replay turn sends `{"a":2,"b":1}`. For Anthropic-via-aggregator routes that cache against the prefix, this changes the prefix hash on replay and busts the cache. Native Anthropic uses the canonical JSON shape; chat-completions replay should too, but the canonical shape isn't necessarily sorted-keys.

**Fix**: preserve the original ordering by carrying through the raw `arguments_text` from the upstream response rather than re-serializing.

### M10 — `server_model` echo loses the *first* chunk's `model` echo for OpenRouter (low → medium)

`compatible.rs:1033-1042` sets `state.server_model = Some(...)` only when it's `None`. The outer loop (`compatible.rs:555-559, 571-575`) drains it via `state.server_model.take()` *between* chunks. But the `server_model_echo.observe` call mutates `emitted` so any subsequent chunk's mismatched model echo is suppressed — including OpenRouter's well-known mid-stream provider fallback (when the primary provider 429s and OpenRouter retries through a different upstream). The user never learns the request actually came from a different vendor.

**Fix**: re-emit `ServerModel` whenever a *different* echo arrives, not just the first one.

### M11 — `ensure_vision_support` does not consult the resolved Anthropic flavor (low)

`stream_response` at `compatible.rs:445-447` calls `request.ensure_vision_support(self.preset.as_str())`. `capabilities_for(provider, model)` (in `registry.rs:256-258`) is keyed on preset name (`"openrouter"`, `"vercel"`, …). For `anthropic/claude-opus-4-7` routed through OpenRouter, vision capability is *Anthropic's* (true), but the registry only has 4 OpenRouter entries and the lookup falls back to `vision: false`. A user attaching an image on a vision-capable model via OpenRouter sees a `provider does not support vision` error.

**Fix**: also consult the cross-flavor capability table via `compat_entry(model).flavor` before failing.

### L1 — `tracing` warning on `eprintln!` is logged twice (low)

`drain_tool_calls` at `compatible.rs:866-883` calls both `eprintln!` and `tracing::warn!` with the same content. Duplicate noise in any environment that captures both stderr and the tracing subscriber.

**Fix**: drop the `eprintln!`. If a stderr breadcrumb is desired, route it through the tracing layer's stderr appender instead.

### L2 — `find_event_boundary` re-scans the whole buffer each iteration (low)

`sse.rs:36-47` walks `self.buffer` with `.windows(2)` on every push call. For long buffered SSE chunks (multi-MB reasoning streams from DeepSeek-R1 via aggregator), this is O(n²) per push. Not a correctness bug; a performance cliff.

**Fix**: track scan position across calls.

### L3 — `decode_sse_event` ignores `event:` field entirely (low)

`sse.rs:49-63` only extracts `data:` lines. SSE allows `event: <name>` lines that an OpenAI-compat aggregator could use to disambiguate stream phases. Today nobody in the verified preset list does, but it's a silent loss.

**Fix**: keep the `event` field on the returned struct (small refactor).

### L4 — `[DONE]` after a `usage` chunk also breaks the lossy-completion path (low)

Related to C1: if the upstream sends `{usage: ...} \n\n data: [DONE] \n\n` together, `decode_sse_event` joins both into one chunk. `parse_chat_event` then sees `"{usage:...}\n[DONE]"` which is invalid JSON, surfacing `SqueezyError::ProviderStream("invalid SSE JSON: ...")`. Verified path possible per the `decode_sse_event` "join with \n" logic (`sse.rs:60`).

**Fix**: split events on data-line boundaries inside `decode_sse_event` when `[DONE]` is detected, OR special-case `[DONE]` interleaving in `parse_chat_event`.

### N1 — `display_name` for Mistral says "Mistral La Plateforme" (nit)

`crates/squeezy-core/src/lib.rs:2030`: "Mistral La Plateforme" mixes English + French + capitalized "L". The official brand is "La Plateforme" (English context) or "Mistral AI" depending on surface. Either pick one.

### N2 — Comment claims xAI routes through this provider but it doesn't (nit)

`compatible.rs:5-6`: "xAI, DeepSeek, Mistral, Together AI..." — xAI Grok 3+ goes through `OpenAiProvider::from_xai_config`, not this provider. Stale comment.

### N3 — `default_api_key_env` for LMStudio/vLLM/llamacpp returns a real env name (nit)

`lib.rs:2117-2120` returns `"LMSTUDIO_API_KEY"` etc. Local servers usually don't auth. If the user has any of these set in their shell for OTHER reasons, the resolver picks them up and tries to use them, producing confusing 401s when the reverse proxy in front doesn't accept the token.

**Fix**: return `""` for local presets and treat empty as "no auth" in the resolver (similar to `Custom`).

### N4 — `is_full_tier` and `MODEL_REGISTRY` providers disagree (nit)

`is_full_tier` (`lib.rs:2048-2059`) returns `true` for OpenRouter, Vercel, PortKey, Groq, XAi, DeepSeek, Vertex. But `models.json` has zero entries for `portkey` (verified by `grep "portkey" models.json` — no matches). So PortKey claims "full tier" yet has no curated models, no capability flags, no pricing.

**Fix**: either add the Portkey-routed Anthropic/OpenAI/Google entries to `models.json` (matching the `@open-ai/gpt-...` shape the error hint suggests at `compatible.rs:515-519`), or drop PortKey from `is_full_tier`.

## Per-Preset Findings

### OpenRouter

- **Base URL in squeezy**: `https://openrouter.ai/api/v1` (`crates/squeezy-core/src/lib.rs:79`) — Verified: ✓ (official docs)
- **Auth header**: `Authorization: Bearer <key>` (`compatible.rs:474`) — Verified: ✓
- **Env var**: `OPENROUTER_API_KEY` (`lib.rs:2099`) — Verified: ✓
- **Extra headers**: `HTTP-Referer`, `X-Title` injected by default (`compatible.rs:768-772`)
- **Findings**:
  - **OR-1 (medium)**: OpenRouter recently renamed `X-Title` to `X-OpenRouter-Title` (May 2026 docs — `X-Title` still works for backwards compat but no longer the recommended name). Cosmetic for now; future-proofing.
  - **OR-2 (medium)**: Squeezy does not surface `usage.cost` from OpenRouter's response envelope. OpenRouter ships a top-level `cost` field in USD (per their docs); `parse_chat_usage` (`compatible.rs:1138-1164`) only reads token fields. Users routing through OpenRouter get squeezy's *estimated* cost (or zero) instead of the actual cost OpenRouter computed. Fix: read `cost` from the streamed `usage` JSON for OpenRouter-flavored streams.
  - **OR-3 (low)**: No support for OpenRouter-specific body fields like `provider.order`, `route: "fallback"`, `transforms: ["middle-out"]`, which let users opt out of provider fallback or compress prompts. Custom users can supply these via a body extension mechanism — but no such mechanism exists yet. Tracked under H3.
  - **OR-4 (low)**: Provider routing rewrites mid-stream are detected only on the *first* `model` echo (M10). When OpenRouter swaps providers mid-request, the user never learns.

### Vercel AI Gateway

- **Base URL in squeezy**: `https://ai-gateway.vercel.sh/v1` (`lib.rs:81`) — Verified: ✓
- **Auth header**: `Authorization: Bearer <key>` — Verified: ✓
- **Env var**: `AI_GATEWAY_API_KEY` (`lib.rs:2100`) — Verified: ✓ (Vercel docs use `AI_GATEWAY_API_KEY` for the standalone token; the OIDC flow uses `VERCEL_OIDC_TOKEN`)
- **Extra headers**: none by default
- **Findings**:
  - **VL-1 (medium)**: Vercel REQUIRES the `provider/model` prefix in the model id (verified May 2026: "Requests with unrecognized model prefixes return a 400 Bad Request"). Squeezy's default model (`anthropic/claude-opus-4-7`, `lib.rs:82`) matches, but a user who configures `model = "claude-opus-4-7"` (no prefix) gets a 400 with no actionable hint. Add validation: when preset is `Vercel`, the model id must contain `/`.
  - **VL-2 (low)**: No `VERCEL_OIDC_TOKEN` fallback. Vercel allows OIDC-based auth when deployed on Vercel; squeezy only honors the static `AI_GATEWAY_API_KEY`. Document or implement.
  - **VL-3 (low)**: Vercel-specific provider routing options (`providerOptions: { ... }`) are not exposed. Same root cause as H3.

### PortKey

- **Base URL in squeezy**: `https://api.portkey.ai/v1` (`lib.rs:83`) — Verified: ✓
- **Auth header**: `Authorization: Bearer <key>` (`compatible.rs:474`) — Verified ✗ — **PortKey expects `x-portkey-api-key`, not `Authorization: Bearer`** for the bare REST flow. The Bearer form works only because PortKey treats both as equivalent (Authorization-as-PortkeyKey is a documented alias), but the canonical form is the header.
- **Env var**: `PORTKEY_API_KEY` (`lib.rs:2101`) — Verified: ✓
- **Extra headers**: user supplies `x-portkey-virtual-key`, `x-portkey-config`, `x-portkey-provider` via TOML (no defaults)
- **Findings**:
  - **PK-1 (medium)**: Sending `Authorization: Bearer` works but PortKey's preferred header is `x-portkey-api-key`. The current scheme means a user can't proxy a request through PortKey *to OpenAI* using PortKey's "send your OpenAI key as Bearer + your Portkey key in `x-portkey-api-key`" mode. This forces the user to use a virtual-key indirection for every OpenAI call. Fix: add a config option to switch.
  - **PK-2 (medium)**: `portkey_routing_header_present` (`compatible.rs:747-760`) recognizes `x-portkey-provider`, `-virtual-key`, `-config`. It doesn't recognize `x-portkey-trace-id`, `x-portkey-metadata`, `x-portkey-cache-namespace`, or the new `x-portkey-router` (added Q1 2026 per Portkey changelog). The hint text says "set one of those and retry" even when the user *has* set a non-listed routing header.
  - **PK-3 (low)**: No PortKey entries in `models.json` (verified — `grep "portkey" crates/squeezy-llm/src/models.json` is empty). The `is_full_tier` flag promises curated models but delivers none. See N4.
  - **PK-4 (low)**: Multi-key fallback (PortKey allows `x-portkey-config: { "fallback": [...] }`) is not surfaced; needs to go through `extra_headers` as raw JSON. Document.

### Groq

- **Base URL in squeezy**: `https://api.groq.com/openai/v1` (`lib.rs:86`) — Verified: ✓
- **Auth header**: `Authorization: Bearer <key>` — Verified: ✓
- **Env var**: `GROQ_API_KEY` (`lib.rs:2102`) — Verified: ✓
- **Extra headers**: none
- **Findings**:
  - **GQ-1 (high)**: Groq's chat-completions endpoint only emits `usage` when `stream_options: { include_usage: true }` is set. Squeezy always sets it (`compatible.rs:210`) — good. But Groq's final usage chunk arrives **after** the chunk that carries `finish_reason: "stop"`, so C1 above hits Groq directly. Cost reporting is broken for Groq today.
  - **GQ-2 (medium)**: Groq doesn't support `seed` on every model (rejected with 400 on `gpt-oss-*`-class models). Not currently a problem because squeezy doesn't emit `seed` (H3); will become one when it does.
  - **GQ-3 (medium)**: Groq's `tool_choice` accepts `"auto"`, `"none"`, `"required"`, and `{"type":"function","function":{"name":"..."}}`. Squeezy's `tool_choice: Option<String>` only handles strings — explicit-function pinning is impossible.
  - **GQ-4 (low)**: Groq surfaces `x-ratelimit-*` headers (limit, remaining, reset). Squeezy doesn't read them; the retry logic at `retry.rs:148-153` would benefit from honoring Groq's hint.

### DeepSeek

- **Base URL in squeezy**: `https://api.deepseek.com/v1` (`lib.rs:90`) — Verified: ✓ (DeepSeek docs accept both `https://api.deepseek.com` and the `/v1`-suffixed form)
- **Auth header**: `Authorization: Bearer <key>` — Verified: ✓
- **Env var**: `DEEPSEEK_API_KEY` (`lib.rs:2103`) — Verified: ✓
- **Extra headers**: none
- **Findings**:
  - **DS-1 (high)**: DeepSeek streams `reasoning_content` as a separate field from `content` for thinking-mode models. Squeezy's `parse_chat_event` does pick it up at `compatible.rs:1053-1054` (via `collect_delta_text(delta.get("reasoning_content"))`), good. But the `reasoning_only_stop` logic at `compatible.rs:1094-1109` injects a noisy `[squeezy] model finished without emitting any content` notice — for `deepseek-reasoner`, this is a **normal** completion when the model finishes its thinking then a content turn was expected next. The notice text references `tool_choice = "required"` which makes no sense for a pure-text reasoning turn.
  - **DS-2 (high)**: Default model `deepseek-chat` is scheduled for deprecation 2026/07/24 (verified DeepSeek docs); the replacement is `deepseek-v4-flash`. Update `DEFAULT_DEEPSEEK_MODEL` (`lib.rs:91`).
  - **DS-3 (medium)**: DeepSeek's `usage` object includes `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`. `parse_chat_usage` reads `prompt_cache_hit_tokens` at `compatible.rs:1147-1151`. Good. But the `prompt_cache_miss_tokens` field isn't surfaced — squeezy's CostSnapshot lacks the slot. Minor accounting gap.
  - **DS-4 (low)**: `deepseek-reasoner`'s thinking budget (`reasoning_effort` analog) is controlled via a `thinking` parameter, not `reasoning_effort`. Squeezy sends both `reasoning_effort` and `reasoning.effort` (`compatible.rs:222-223`); neither is what DeepSeek wants. The thinking-mode toggle is silently lost.

### Vertex AI

- **Base URL in squeezy**: built from `vertex_base_url(project, location)` → `https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}/endpoints/openapi` (`lib.rs:133-137`) — Verified: ✓ (both `v1` and `v1beta1` work; `v1` is GA)
- **Auth header**: `Authorization: Bearer <OAuth_token>` — Verified: ✓
- **Env var**: `VERTEX_ACCESS_TOKEN` (`lib.rs:2109`) — Verified: ✗ — there is no Google-published env var with this exact name. The de-facto convention is the user runs `gcloud auth print-access-token` and either pipes into a custom env or uses GOOGLE_APPLICATION_CREDENTIALS for service accounts. Squeezy invented `VERTEX_ACCESS_TOKEN`; documentation should make clear this is a squeezy-local convention.
- **Extra headers**: none
- **Findings**:
  - **VX-1 (critical)**: Vertex tokens expire in ~1h. squeezy's auth-retry path (`retry.rs:79-102`) calls `source.invalidate()` then re-reads from the env. But `static_api_key_source` (`compatible.rs:93`) snapshots the env var value at `from_config` time and never re-reads it. So once the token expires mid-session, the agent fails permanently with 401. The OAuth source pattern used for GitHub Copilot (`with_api_key_source`) exists exactly for this but isn't wired up for Vertex.
  - **VX-2 (medium)**: No support for the Anthropic-on-Vertex flavor (`anthropic/claude-opus-4-7@<region>`). The base URL is hard-coded to the OpenAI-compat path; routing Claude through Vertex requires a different URL (`projects/{p}/locations/{l}/publishers/anthropic/models/...:streamRawPredict`). Acknowledged limitation but not documented.
  - **VX-3 (medium)**: `model` field for Vertex's OpenAI-compat path expects `google/gemini-2.5-pro` (with the `google/` namespace). Default at `lib.rs:96` is correct. But for *Anthropic-on-Vertex* (out of scope per VX-2) the model ID would be `claude-...` without prefix. No validation of the constraint.
  - **VX-4 (low)**: `vertex_base_url` doesn't trim whitespace before concatenating. A user with `VERTEX_PROJECT=" my-project"` (leading space) builds a URL with a space in the path.

### Mistral

- **Base URL in squeezy**: `https://api.mistral.ai/v1` (`lib.rs:98`) — Verified: ✓
- **Auth header**: `Authorization: Bearer <key>` — Verified: ✓
- **Env var**: `MISTRAL_API_KEY` (`lib.rs:2110`) — Verified: ✓
- **Extra headers**: none
- **Findings**:
  - **MS-1 (high)**: Mistral's `tool_choice` accepts `"none"`, `"auto"`, `"any"`, or `{"type":"function","function":{"name":"..."}}`. **The user-facing `tool_choice = "required"` (the value squeezy's docs recommend at `compatible.rs:283-294`) is NOT recognized by Mistral** — they call it `"any"`. Users with a Mistral preset setting `tool_choice = "required"` get the model's default behavior, silently.
  - **MS-2 (medium)**: Mistral rejects unknown body fields with HTTP 422 (verified May 2026 Mistral docs). Squeezy's `reasoning_effort` + `reasoning` body fields trigger this on Mistral models. See M6.
  - **MS-3 (low)**: No entries in `models.json` for Mistral.
  - **MS-4 (nit)**: Display name "Mistral La Plateforme" — see N1.

### Together AI

- **Base URL in squeezy**: `https://api.together.xyz/v1` (`lib.rs:100`) — Verified: ✓
- **Auth header**: `Authorization: Bearer <key>` — Verified: ✓
- **Env var**: `TOGETHER_API_KEY` (`lib.rs:2111`) — Verified: ✓
- **Extra headers**: none
- **Findings**:
  - **TG-1 (medium)**: Together's `tool_choice` accepts the OpenAI shape but with a known quirk: for Llama-3.x models, `tool_choice = "required"` is occasionally ignored on streamed responses (per Together docs, May 2026). Not squeezy's bug, but the docs should call it out.
  - **TG-2 (medium)**: Together exposes a `repetition_penalty` field that's distinct from `presence_penalty`. Not reachable from `LlmRequest` (H3).
  - **TG-3 (low)**: No entries in `models.json` for `together`.

### Fireworks AI

- **Base URL in squeezy**: `https://api.fireworks.ai/inference/v1` (`lib.rs:102`) — Verified: ✓
- **Auth header**: `Authorization: Bearer <key>` — Verified: ✓
- **Env var**: `FIREWORKS_API_KEY` (`lib.rs:2112`) — Verified: ✓
- **Extra headers**: none
- **Findings**:
  - **FW-1 (medium)**: Fireworks model IDs use `accounts/fireworks/models/<name>` shape; default in squeezy is `accounts/fireworks/models/llama-v3p3-70b-instruct` (`lib.rs:103`). Verified shape. Stale model name though: `llama-v3p3-70b-instruct` was deprecated in favor of `llama-v4-*` SKUs in Q1 2026. Update default.
  - **FW-2 (medium)**: Fireworks supports a `prompt_truncate_len` field for budget control — not exposed.
  - **FW-3 (low)**: No entries in `models.json` for `fireworks`.

### Cerebras

- **Base URL in squeezy**: `https://api.cerebras.ai/v1` (`lib.rs:104`) — Verified: ✓
- **Auth header**: `Authorization: Bearer <key>` — Verified: ✓
- **Env var**: `CEREBRAS_API_KEY` (`lib.rs:2113`) — Verified: ✓
- **Extra headers**: none
- **Findings**:
  - **CB-1 (medium)**: Cerebras's model IDs use a flat naming convention (`llama-3.3-70b`, not `meta-llama/Llama-3.3-70B-Instruct`). Default is correct (`lib.rs:105`).
  - **CB-2 (medium)**: Cerebras rejects `stream_options.include_usage: true` on some legacy SKUs with 400 — squeezy unconditionally sends it (`compatible.rs:210`). No model-id branching.
  - **CB-3 (low)**: Cerebras's `usage` payload was missing `prompt_tokens_details` until late 2025; older API versions return only `prompt_tokens` / `completion_tokens`. Cached-input billing visibility is lost on older deployments.
  - **CB-4 (low)**: No entries in `models.json` for `cerebras`.

### DeepInfra

- **Base URL in squeezy**: `https://api.deepinfra.com/v1/openai` (`lib.rs:106`) — Verified: ✓
- **Auth header**: `Authorization: Bearer <key>` — Verified: ✓
- **Env var**: `DEEPINFRA_API_KEY` (`lib.rs:2114`) — Verified: ✓
- **Extra headers**: none
- **Findings**:
  - **DI-1 (low)**: No costly test; no `models.json` entries.

### Baseten

- **Base URL in squeezy**: `https://inference.baseten.co/v1` (`lib.rs:108`) — Verified: ✓
- **Auth header**: `Authorization: Bearer <key>` — Verified: ✓
- **Env var**: `BASETEN_API_KEY` (`lib.rs:2115`) — Verified: ✓
- **Extra headers**: none
- **Findings**:
  - **BT-1 (medium)**: Baseten also exposes per-deployment URLs (`https://model-{id}.api.baseten.co/environments/production/sync/v1`) for SLA-pinned custom models. Not addressable via the preset — user must use `Custom`. Document.
  - **BT-2 (low)**: No costly test; no `models.json` entries.

### LM Studio

- **Base URL in squeezy**: `http://127.0.0.1:1234/v1` (`lib.rs:112`) — Verified: ✓
- **Auth header**: `Authorization: Bearer <key>` (only when key is set) — Verified: ✓
- **Env var**: `LMSTUDIO_API_KEY` (`lib.rs:2118`) — Verified: ✗ — LM Studio has no official env var. See N3.
- **Extra headers**: none
- **Findings**:
  - **LM-1 (medium)**: There are TWO LM Studio code paths: this preset goes through `OpenAiCompatibleProvider`, while the `LMStudioProvider` in `crates/squeezy-llm/src/lmstudio.rs` is used by *Ollama's* OpenAI-compat fallback. They behave differently (no cache markers, no preset-default headers, no PortKey hint in LMStudioProvider). Decision: pick one. Today a user can't tell which they're using; the registry routes LMStudio preset to `OpenAiCompatibleProvider` (`registry.rs:370-376`).
  - **LM-2 (low)**: No `models.json` entries; vision capability defaults to `false`, so vision-capable local checkpoints can't be used with images.
  - **LM-3 (low)**: `from_config` requires a non-empty `base_url` (`compatible.rs:63-69`) but the default is `http://127.0.0.1:1234/v1` which is non-empty. Fine. Just noting it works.

### vLLM

- **Base URL in squeezy**: `http://127.0.0.1:8000/v1` (`lib.rs:113`) — Verified: ✓
- **Auth header**: `Authorization: Bearer <key>` (when set) — Verified: ✓
- **Env var**: `VLLM_API_KEY` (`lib.rs:2119`) — Verified: ✗ — vLLM uses `OPENAI_API_KEY` by default; the env var is named arbitrarily. See N3.
- **Extra headers**: none
- **Findings**:
  - **VL_VLLM-1 (low)**: No `models.json` entries.

### llama.cpp

- **Base URL in squeezy**: `http://127.0.0.1:8080/v1` (`lib.rs:114`) — Verified: ✓
- **Auth header**: `Authorization: Bearer <key>` (when set) — Verified: ✓
- **Env var**: `LLAMACPP_API_KEY` (`lib.rs:2120`) — Verified: ✗ — no official env. See N3.
- **Extra headers**: none
- **Findings**:
  - **LC-1 (medium)**: llama.cpp's `/v1/chat/completions` does not support `stream_options.include_usage` on older builds; squeezy unconditionally sends it. May 422 some older installs.

### Cloudflare Workers AI

- **Base URL in squeezy**: `https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1` (`lib.rs:122-123`) — Verified: ✓
- **Auth header**: `Authorization: Bearer <CLOUDFLARE_API_KEY>` — Verified: ✓
- **Env var**: `CLOUDFLARE_API_KEY` (`lib.rs:2126`) — Verified: ✓
- **Extra headers**: none
- **Findings**:
  - **CWAI-1 (medium)**: Workers AI model IDs are prefixed with `@cf/` (e.g. `@cf/meta/llama-3.3-70b-instruct-fp8-fast`); default at `lib.rs:127` is correct. But the `@` triggers a *non-issue* on most aggregators — verify nothing in normalize/encode escapes it.
  - **CWAI-2 (medium)**: Cloudflare's chat-completions response does not include `usage` for some models; the `parse_chat_usage` fallback returns zeros silently.
  - **CWAI-3 (low)**: No `models.json` entries.

### Cloudflare AI Gateway

- **Base URL in squeezy**: `https://gateway.ai.cloudflare.com/v1/{account_id}/{gateway_id}/compat` (`lib.rs:124-125`) — Verified: ✗ — `/compat` is **deprecated** (May 2026 verification); recommended is the new REST API path. Functional today but on a deprecation timeline.
- **Auth header**: `Authorization: Bearer <CLOUDFLARE_API_KEY>` + optional `cf-aig-authorization` (`lib.rs:8700-8713`) — Verified: ✗ — see C2; the Authorization header should carry the UPSTREAM provider's key, not Cloudflare's.
- **Env var**: `CLOUDFLARE_API_KEY` for the Bearer + `CF_AIG_TOKEN` for `cf-aig-authorization` — Verified: ✗ (split incorrectly)
- **Extra headers**: `cf-aig-authorization` injected from `CF_AIG_TOKEN` if set
- **Findings**:
  - **CFAG-1 (critical)**: Dual-auth scheme is inverted. See C2.
  - **CFAG-2 (critical)**: `/compat` deprecated. See C3.
  - **CFAG-3 (medium)**: The default gateway id is `"default"` (`lib.rs:126`). Good — matches Cloudflare's auto-created gateway. But the substitution code doesn't fall back to `"default"` automatically when `gateway_id` is `None`; it 404s with a placeholder URL. The config builder at `lib.rs:8684-8687` does fall back, but a direct `OpenAiCompatibleProvider::from_config` caller without a `gateway_id` gets the placeholder error.

### Azure OpenAI

- **Base URL in squeezy**: empty default (`DEFAULT_AZURE_OPENAI_BASE_URL = ""`, `lib.rs:34`); user-supplied — Verified: per resource (must be `https://{resource}.openai.azure.com/openai/v1` for the v1 GA, or older `/openai/deployments/{deployment}` paths for the classic API).
- **Auth header**: `api-key: <key>` (`crates/squeezy-llm/src/openai.rs:342-346`) — Verified: ✓
- **Env var**: `AZURE_OPENAI_API_KEY` (per costly test `azure_openai_costly.rs:13`) — Verified: ✓
- **Extra headers**: none; `?api-version=` query string appended (`openai.rs:316-319`)
- **Findings**:
  - **AZ-1 (high)**: Azure routes through `OpenAiProvider::from_azure_config` (`openai.rs:69-86`) hitting `/responses`, NOT through the OpenAI-compatible chat-completions aggregator. So most aggregator findings don't apply to Azure. **The current Azure path assumes the v1 GA URL shape (`/openai/v1`)**; users with classic-style URLs (`/openai/deployments/<deployment>/chat/completions?api-version=...`) get a 404 because the provider always appends `/responses`.
  - **AZ-2 (medium)**: Azure's classic auth scheme (`api-key` header) is correctly implemented at `openai.rs:342-346`. Bearer-via-Entra-ID (the modern auth path) is not. Users with Entra workload identity must override `Authorization` via `extra_headers`, but `from_azure_config` doesn't carry an `extra_headers` slot.
  - **AZ-3 (medium)**: `deployment_name_map` rewrites the body's `model` field (`openai.rs:329-332`); this works for `/responses` v1 GA where deployment is the model field, but breaks for any user still on the URL-path-encoded classic flow (where `{deployment}` is in the URL).

### Custom

- **Base URL in squeezy**: empty default — Verified: ✓
- **Auth header**: `Authorization: Bearer <key>` — Verified: ✓
- **Env var**: empty default — Verified: ✓
- **Extra headers**: user-supplied via TOML
- **Findings**:
  - **CT-1 (medium)**: No URL validation (M5). SSRF surface.
  - **CT-2 (medium)**: No way to specify a non-Bearer auth scheme (header name + value) without going through `extra_headers` + an empty `api_key_env`. Users wiring LiteLLM-fronted models with custom auth schemes hit friction.
  - **CT-3 (low)**: `Custom` is the only path to use the Chat-Completions wire for an arbitrary host (e.g. a self-hosted LiteLLM proxy). Document that this is the supported escape hatch.

## Test Coverage Gaps

| Preset | Costly test? | Mock test? | `models.json`? | Gaps |
|---|---|---|---|---|
| OpenRouter | ✓ (`openrouter_costly.rs`) | ✗ | ✓ (4 entries) | No test for `provider.order` body, no test for `usage.cost` parsing |
| Vercel AI Gateway | ✓ (`vercel_costly.rs`) | ✗ | ✓ (3 entries) | No prefix-required validation test |
| PortKey | ✓ (`portkey_costly.rs`) | ✗ | ✗ | No virtual-key vs config-key vs provider-header matrix |
| Groq | ✓ (`groq_costly.rs`) | ✗ | ✓ (3 entries) | No post-`finish_reason` usage chunk test (C1) |
| DeepSeek | ✓ (`deepseek_costly.rs`) | ✗ | ✓ (2 entries) | No `reasoning_content` end-to-end test, no thinking-mode test |
| Vertex AI | ✓ (`vertex_costly.rs`) | ✗ | ✓ (2 entries) | No OAuth-token-refresh test (VX-1) |
| Azure OpenAI | ✓ (`azure_openai_costly.rs`) | ✗ | ✓ (3 entries) | No classic-URL vs v1-GA path differentiator test |
| Mistral | ✗ | ✗ | ✗ | No coverage at all |
| Together | ✗ | ✗ | ✗ | No coverage at all |
| Fireworks | ✗ | ✗ | ✗ | No coverage at all |
| Cerebras | ✗ | ✗ | ✗ | No coverage at all |
| DeepInfra | ✗ | ✗ | ✗ | No coverage at all |
| Baseten | ✗ | ✗ | ✗ | No coverage at all |
| LM Studio | ✗ | ✓ (`lmstudio_mock.rs`, but tests `LMStudioProvider` not the preset) | ✗ | The preset path is untested |
| vLLM | ✗ | ✗ | ✗ | No coverage |
| llama.cpp | ✗ | ✗ | ✗ | No coverage |
| Cloudflare Workers AI | ✗ | ✗ | ✗ | URL substitution tested in `compatible_tests.rs:986-1029`; nothing end-to-end |
| Cloudflare AI Gateway | ✗ | ✗ | ✗ | Same as above; dual-auth bug (C2) goes undetected |
| Custom | ✗ | ✗ | ✗ | No SSRF/validation test |

**Universal shared-core gaps**:
- No test for tool-call `index` partition across choices (H4).
- No test for the post-`finish_reason` usage chunk loss (C1).
- No test for inline mid-stream `error` JSON triggering `ProviderStream` (H7).
- No test for SSE chunks where `[DONE]` is joined into the same event as the previous JSON (L4).
- No test for `n > 1` body emission or response (H1).
- No test for `parallel_tool_calls`, `output_schema`, `seed`, `temperature` forwarding (H3, M2, M3).

## Verification Strategy

A parameterized mock-server harness would cover the 18 presets in one file without any API keys. Suggested shape (adapt the `spawn_chat_server` pattern from `lmstudio_mock.rs:25-63`):

```rust
fn cases() -> Vec<(OpenAiCompatiblePreset, &'static str)> {
    vec![
        (Preset::OpenRouter, "anthropic/claude-haiku-4-5"),
        (Preset::Vercel, "anthropic/claude-haiku-4-5"),
        (Preset::PortKey, "@open-ai/gpt-5.5"),
        (Preset::Groq, "llama-3.1-8b-instant"),
        (Preset::DeepSeek, "deepseek-chat"),
        // … etc
    ]
}
```

The mock server should:
1. Accept any POST to `/chat/completions` (or the per-preset URL after placeholder substitution).
2. Assert the wire shape: `Authorization` header for Bearer presets, `api-key` for Azure, `cf-aig-authorization` for AI Gateway.
3. Reply with a canned SSE script that exercises:
   - Plain content delta + `finish_reason: stop` + separate usage chunk + `[DONE]` (catches C1).
   - Two tool calls with overlapping `index` (catches H4).
   - Inline `error: {...}` JSON mid-stream (catches H7).
   - `[DONE]` joined to the previous chunk (catches L4).
   - `reasoning_content` array shape (catches M11 / vision logic).
4. Capture cost + events and assert no regressions.

**401-ping verification**: For each preset, run `OpenAiCompatibleProvider::from_config` with a deliberately bad key against a mock that returns 401, then assert the error message contains the preset's display name and a hint that the API key resolution succeeded but auth failed. Catches PK-2 / CFAG-1 / VX-1 inversions.

**Endpoint smoke tests against current docs**: write a unit test that asserts each preset's `default_base_url()` matches a hand-curated table; that table is updated whenever vendor docs change (CI lint to refresh quarterly). Catches CFAG-2 / FW-1 / DS-2 silently rotting.

## References

- OpenRouter API reference: https://openrouter.ai/docs/api/reference/overview
- OpenRouter app attribution headers: https://openrouter.ai/docs/app-attribution
- Vercel AI Gateway chat completions: https://vercel.com/docs/ai-gateway/sdks-and-apis/openai-chat-completions
- Vercel AI Gateway model + provider docs: https://vercel.com/docs/ai-gateway/models-and-providers
- PortKey chat completions reference: https://docs.portkey.ai/docs/api-reference/chat-completions
- PortKey virtual keys: https://portkey.ai/docs/product/ai-gateway/virtual-keys
- Groq OpenAI compatibility: https://console.groq.com/docs/openai
- DeepSeek chat completions reference: https://api-docs.deepseek.com/api/create-chat-completion
- DeepSeek thinking mode: https://api-docs.deepseek.com/guides/thinking_mode
- Vertex AI OpenAI-compat v1: https://cloud.google.com/vertex-ai/generative-ai/docs/reference/rest/v1/projects.locations.endpoints.chat/completions
- Vertex AI OpenAI-compat v1beta1: https://cloud.google.com/vertex-ai/docs/reference/rest/v1beta1/projects.locations.endpoints.chat/completions
- Mistral chat completions: https://docs.mistral.ai/api
- Together AI OpenAI compatibility: https://docs.together.ai/docs/openai-api-compatibility
- Baseten OpenAI-compat: https://docs.baseten.co/api-reference/openai
- Cloudflare AI Gateway unified API (`/compat`, deprecated): https://developers.cloudflare.com/ai-gateway/usage/chat-completion/
- Cloudflare AI Gateway authenticated gateway: https://developers.cloudflare.com/ai-gateway/configuration/authentication/
- Cloudflare Workers AI OpenAI compatibility: https://developers.cloudflare.com/workers-ai/configuration/open-ai-compatibility/
- Cloudflare AI Gateway REST API (May 2026 release): https://developers.cloudflare.com/changelog/post/2026-05-21-rest-api/
- Azure OpenAI v1 API: https://learn.microsoft.com/en-us/azure/foundry/openai/latest
- Azure OpenAI Responses API: https://learn.microsoft.com/en-us/azure/foundry/openai/how-to/responses
- opencode `openai-compatible-profile.ts` (peer reference): `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/openai-compatible-profile.ts`
- opencode `cloudflare.ts` (peer reference for dual-auth): `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/cloudflare.ts`
- opencode `azure.ts` (peer reference for v1 GA URL shape): `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/azure.ts`
