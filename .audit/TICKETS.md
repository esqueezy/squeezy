# Provider Audit — Consolidated Tickets

Source: 8 deep audits under `.audit/providers/*.md` (anthropic, openai, google, bedrock, ollama, lmstudio, xai, openai-compatible).

**Totals**: ~185 findings across 8 reports — **18 critical / 47 high / 70 medium / 39 low / 31 nit**.

The tickets below are ordered for action:
1. **Cross-cutting fixes (X)** — one PR closes the same class of bug across multiple providers. Land these first; they shrink the per-provider lists.
2. **Critical per-provider (C)** — concrete, single-provider bugs that silently lose data, money, or correctness.
3. **High per-provider (H)** — visible regressions; ship the next sprint.
4. **Catalog / housekeeping (K)** — stale model IDs, missing registry entries.
5. **Test-coverage tickets (T)** — what to add to lock in the fixes.

Effort estimates: **XS** <1h, **S** 1–4h, **M** 1d, **L** multi-day.

---

## 1. Cross-cutting tickets (do these first)

These are findings that recur across 3+ providers. One shared fix closes many per-provider entries.

### X-01 — Add `with_stream_retry` wrapper to OpenAI, Google, Bedrock
- **Severity**: HIGH (3 providers)
- **Providers**: openai.rs, google.rs, bedrock.rs
- **Today**: Only `anthropic.rs:490` wraps `stream_response` in `with_stream_retry`. Sibling providers return raw `try_stream!`, so any mid-stream RST, idle timeout, or partial frame is terminal.
- **Source**: openai-HIGH, google-HIGH, bedrock-HIGH (each cites `anthropic.rs:490` as the pattern).
- **Fix**: refactor each provider's stream body into a `make_attempt` closure; wrap with `with_stream_retry(provider_name, RetryPolicy::provider_stream(transport), cancel, make_attempt)`. `StreamSkipState` (`retry.rs:355-484`) already dedup-es `TextDelta` / `ReasoningDelta` / `ToolCall` / `Started` events.
- **Effort**: M (3 providers × ~half day).
- **Verification**: extend mock TCP-server tests (pattern from `anthropic_stream_retry.rs:54-121`) to drop mid-stream and assert no duplicate prefix.

### X-02 — Fix shared SSE decoder to no-op on empty `data:` lines
- **Severity**: CRITICAL (affects all SSE providers)
- **Providers**: shared `sse.rs:49-63`. Surfaces on openai.rs, lmstudio.rs, compatible.rs.
- **Today**: `decode_sse_event` returns `Some("")` for keep-alive heartbeats; downstream `serde_json::from_str("")` aborts the turn with `invalid SSE JSON: EOF`.
- **Source**: openai-C1, lmstudio-HIGH, compatible-L4.
- **Fix**: in `decode_sse_event`, drop empty `data_lines` entries; return `None` if all entries are empty. Also trim whitespace around `[DONE]` literal comparisons.
- **Effort**: XS.
- **Verification**: unit test in `sse_tests.rs` feeding `data:\n\n`, `data: \n\n`, `data: [DONE] \n\n`.

### X-03 — Tool-call `call_id` collisions across SSE chunks
- **Severity**: CRITICAL (Google) / HIGH (Ollama)
- **Providers**: google.rs:391,428-432; ollama.rs:382-402.
- **Today**: Both providers use `format!("{provider}_call_{index}")` where `index` is the **chunk-local** part position. Two consecutive SSE chunks each carrying `parts[0]={functionCall:...}` both stamp `..._call_0`. `normalize_tool_ids_for_replay` then collapses two distinct calls into one, dropping the second tool result.
- **Source**: google-C2, ollama-MEDIUM-5.
- **Fix**: lift counter to a per-stream `usize` on the stream-loop state; pass into `parse_*_event`. Reference: `others/opencode/packages/llm/src/protocols/gemini.ts:364`.
- **Effort**: S (per provider; ~2 trivial diffs).
- **Verification**: synthetic SSE with two events each containing `parts[0]={functionCall:...}`; assert distinct `call_id`s reach the downstream consumer.

### X-04 — Add `tool_choice` forwarding to Anthropic / Google / Bedrock
- **Severity**: HIGH (3 providers)
- **Providers**: anthropic.rs:144-242, google.rs:54-103, bedrock.rs:638-672.
- **Today**: `LlmRequest.tool_choice` is silently dropped on three of seven native providers. `tool_choice = "required"` is a no-op there.
- **Source**: anthropic-LOW, google-HIGH, bedrock-MEDIUM.
- **Fix**: per provider, map `Some("auto"|"required"|"none"|"tool:X")` to the vendor shape:
  - Anthropic: `tool_choice: {type:"auto"|"any"|"tool", name}` + `disable_parallel_tool_use`.
  - Google: `toolConfig.functionCallingConfig.mode = AUTO|ANY|NONE`.
  - Bedrock: `ToolChoice::Auto|Any|Tool(name)`.
- **Effort**: S per provider.
- **Verification**: body-shape unit tests asserting the field is on the wire.

### X-05 — Add `output_schema` forwarding to Google, Compatible, LMStudio
- **Severity**: MEDIUM (3 providers)
- **Providers**: google.rs:54-103 (HIGH there), compatible.rs:134-297 (M3), lmstudio.rs:113-141 (LOW).
- **Today**: Only the OpenAI Responses provider emits `text.format`. Structured-output eval/contribution flows lose the strict guarantee on every other provider.
- **Source**: google-HIGH (output_schema), compatible-M3, lmstudio-LOW.
- **Fix**:
  - Chat-completions providers: emit `response_format: {type: "json_schema", json_schema: {name, schema, strict}}`.
  - Google: emit `generationConfig.responseMimeType: "application/json"` + `generationConfig.responseSchema` (after the same sanitize pass needed for Google function-declarations).
- **Effort**: S per provider.

### X-06 — Tool-result content arrays for images (Anthropic + OpenAI)
- **Severity**: CRITICAL (Anthropic), MEDIUM (OpenAI)
- **Providers**: anthropic.rs:373-381, openai.rs:639-643.
- **Today**: `LlmInputItem::FunctionCallOutput.output` is always stringified. An MCP tool returning a PNG screenshot ships as ~110k tokens of base64; vision-capable models can't see the image even though it's "there."
- **Source**: anthropic-C2, openai-MEDIUM.
- **Fix**: extend `LlmInputItem::FunctionCallOutput` to optionally carry a structured `Vec<ToolResultPart>` (text + image variants). Per-provider lower into:
  - Anthropic `tool_result.content: Array<{type:"image"|"text", source:{...}}>`.
  - OpenAI `function_call_output.output: Array<{type:"input_text"|"input_image"}>`.
- **Effort**: M (cross-crate type change + per-provider lowering).
- **Reference**: `others/opencode/packages/llm/src/protocols/anthropic-messages.ts:101-114`, `openai-responses.ts:60-68`.

### X-07 — `MALFORMED_FUNCTION_CALL` / `pause_turn` / `load`/`unload` stop reasons
- **Severity**: CRITICAL (Google, Ollama) / MEDIUM (Anthropic)
- **Providers**: lib.rs:498-510 (Anthropic), 521-529 (Google), 545-551 (Ollama).
- **Today**: `StopReason::from_*` falls through to `Other(...)` for several documented values. Anthropic's `pause_turn` (long extended-thinking pause), Google's `MALFORMED_FUNCTION_CALL` (and 7 other variants), Ollama's `load` / `unload` housekeeping signals all get treated as turn terminals. Ollama is worst: the `load` frame fires `Completed { eval_count: 0 }` and eats the actual generation that follows.
- **Source**: ollama-C1, google-C3, anthropic-MEDIUM, google-NIT.
- **Fix**:
  - Add `StopReason::PauseTurn`, `MalformedFunctionCall`, etc. (or a `LoadEvent` variant for Ollama).
  - Ollama: treat `load`/`unload` as no-op, keep streaming.
  - Google: surface `MALFORMED_FUNCTION_CALL` as terminal error so the agent stops retrying.
  - Anthropic: handle `pause_turn` (caller sends a `continue` no-op).
  - Bonus: `tracing::warn!` on any `Other(...)` so future drift is observable.
- **Effort**: S.

### X-08 — Stale catalogs — refresh `models.json` + add registry entries for the 10 presets with zero entries
- **Severity**: CRITICAL (xAI) / HIGH (DeepSeek) / MEDIUM (Fireworks, OpenAI, Anthropic, Google)
- **Providers**: models.json globally; affects xAI, DeepSeek, Fireworks, plus the 10 OpenAI-compat presets without any entries.
- **Today**:
  - xAI: `grok-4`, `grok-4-fast-reasoning`, `grok-code-fast-1` — **all retired 2026-05-15, silently redirected to `grok-4.3` at new pricing.** Default model in `squeezy-core` is `grok-4`. Cost meter is wrong for every active xAI session.
  - DeepSeek: `deepseek-chat` deprecated 2026-07-24 → `deepseek-v4-flash`.
  - Fireworks: `llama-v3p3-70b-instruct` (default) replaced by `llama-v4-*` SKUs in Q1 2026.
  - Zero entries for: `mistral`, `together`, `fireworks`, `cerebras`, `deepinfra`, `baseten`, `vllm`, `llamacpp`, `cloudflare-workers-ai`, `cloudflare-ai-gateway`, `portkey`, `lmstudio`.
- **Source**: xai-C1, compatible-DS-2, compatible-FW-1, compatible-N4.
- **Fix**: refresh xAI list (`grok-4.3`, `grok-4.20-0309-{reasoning,non-reasoning}`, `grok-4.20-multi-agent-0309`, `grok-build-0.1`), DeepSeek (`deepseek-v4-flash`, `deepseek-reasoner-v4`), Fireworks. Bulk-import known model lists for the 10 missing presets. Set capability flags including `reasoning_effort: true` for actual reasoning models.
- **Effort**: M (a lot of small entries, plus quarterly-refresh CI hook).
- **Verification**: add a snapshot test asserting `default_*_model` is in `models.json`.

### X-09 — Catalog-driven reasoning gates (OpenAI / Google / xAI)
- **Severity**: CRITICAL (Google) / HIGH (xAI) / MEDIUM (OpenAI)
- **Providers**: openai.rs:205-217, google.rs:80-88; capability lookup `registry.rs:capabilities_for`.
- **Today**: `reasoning_effort` capability flag is `false` for every Gemini 2.5 model and every xAI reasoning model in `models.json`. Result: even when the caller asks for `reasoning_effort = "high"`, the request body never sets `reasoning.summary = "auto"` / `thinkingConfig.includeThoughts: true` / `thinking_budget`. Users billed for thinking tokens with no thought summaries surfaced.
- **Source**: google-C1, xai-HIGH (reasoning), openai-MEDIUM (default per model).
- **Fix**: piggyback on X-08; set `reasoning_effort: true` and add `default_reasoning_effort` per-model so the provider auto-emits the right body without a caller knob.
- **Effort**: XS once X-08 is done.

### X-10 — Per-model thinking budget clamps (Google, Bedrock, OpenAI)
- **Severity**: HIGH (Google + Bedrock)
- **Providers**: google.rs:85, bedrock.rs:165-185, anthropic.rs:152-160 (clamp `max_tokens`).
- **Today**: One `ReasoningEffort::thinking_budget_tokens()` returns `60_000` for `XHigh`. Plumbed straight through. Gemini 2.5 Pro caps at `32_768`, Flash at `24_576`. Anthropic Bedrock `budget_tokens` not validated against per-model max.
- **Source**: google-HIGH, bedrock-CRITICAL (`maxTokens` not set at all).
- **Fix**: per-model `(min, max)` thinking-budget table in `models.json`; clamp in each provider before emitting.
- **Effort**: S.

### X-11 — Token-usage convention harmonization (Anthropic / Bedrock / Google)
- **Severity**: MEDIUM
- **Providers**: anthropic.rs:733-757, bedrock.rs:283-303, google.rs:365-370.
- **Today**: Three different conventions for "input_tokens":
  - Anthropic native: reports **uncached delta only**; squeezy folds back to total.
  - Bedrock: reports **inclusive total**; squeezy adds `cacheRead+cacheWrite` on top → **double-counts cache writes** by 80% on cache-heavy flows.
  - Google: reports `candidatesTokenCount` **excluding** thinking tokens; squeezy treats it as total output.
- **Source**: bedrock-MEDIUM, google-MEDIUM, anthropic verified-✓ (but inconsistent across providers).
- **Fix**: document `CostSnapshot.input_tokens` as "total prompt tokens billed including cache." Adjust each provider to that contract. Add per-provider tests pinning numerical examples.
- **Effort**: M (touches cost reporting → TUI + ledger).

### X-12 — Connection pool: add `connect_timeout` + `tcp_keepalive` to shared client
- **Severity**: MEDIUM (all providers)
- **Providers**: transport.rs:96-110.
- **Today**: Only `pool_max_idle_per_host` + `pool_idle_timeout` set. A stuck TLS handshake (captive portal, draconian DoH) leaves `send().await` hanging until the user ctrl-c's; idle timeout only kicks in after the first byte.
- **Source**: anthropic-MEDIUM.
- **Fix**: `.connect_timeout(Duration::from_secs(30))` + `.tcp_keepalive(Duration::from_secs(60))` on the shared builder.
- **Effort**: XS.

### X-13 — User-Agent stamp (`squeezy-cli/<version>`) on all providers
- **Severity**: MEDIUM (analytics + abuse mitigation)
- **Providers**: transport.rs (or per-provider).
- **Today**: Reqwest default UA (`reqwest/<version>`) — squeezy traffic anonymous on every vendor dashboard, lumped into generic-reqwest abuse-mitigation buckets.
- **Source**: xai-MEDIUM, anthropic-LOW.
- **Fix**: `.user_agent(format!("squeezy-cli/{}", env!("CARGO_PKG_VERSION")))` on shared client builder.
- **Effort**: XS.

### X-14 — Retry-After parsing: float seconds + HTTP-date
- **Severity**: MEDIUM (all providers via retry.rs)
- **Providers**: retry.rs:335-344.
- **Today**: Tries `retry-after-ms` u64, then `retry-after` u64 seconds. RFC 7231 also allows HTTP-date format (`Wed, 21 Oct 2026 07:28:00 GMT`) and floats (`0.5`). Non-Anthropic proxies emit these shapes.
- **Source**: anthropic-MEDIUM.
- **Fix**: try u64 → f64 (clamp to ms) → `httpdate::parse_http_date`.
- **Effort**: XS.

### X-15 — Cancellation telemetry (drop vs explicit Cancel)
- **Severity**: LOW (all providers)
- **Today**: Dropping the `LlmStream` cancels via reqwest connection close but emits no `LlmEvent::Cancelled`. Telemetry under-counts user-initiated cancels vs UI-rebuild drops.
- **Source**: openai-MEDIUM.
- **Fix**: document the contract (Cancelled is opt-in via token; drop is silent), OR add a per-stream `Drop` impl that emits a final telemetry beacon. Pick one and align all providers.
- **Effort**: S.

### X-16 — Custom preset URL validation (SSRF surface)
- **Severity**: MEDIUM (security)
- **Providers**: compatible.rs Custom preset, build_openai_compatible_config (squeezy-core/src/lib.rs:8602-8726).
- **Today**: `check_base_url_scheme` runs for `AzureOpenAi` only. A `Custom` `base_url = "file:///etc/passwd"` or `http://169.254.169.254/...` (AWS IMDS) passes straight through.
- **Source**: compatible-M5.
- **Fix**: enforce `https://` for non-loopback hosts; allow `http://` only for `127.0.0.1`, `localhost`, link-local. Reuse `is_loopback_host`.
- **Effort**: XS.

### X-17 — Local-preset auth optionality (LMStudio/vLLM/llama.cpp)
- **Severity**: HIGH (LMStudio default-broken)
- **Providers**: compatible.rs:84 (`resolve_api_key_with_inline`) + lib.rs:2117-2120 (env defaults).
- **Today**: `resolve_api_key_with_inline` errors `ProviderNotConfigured` on empty key. LM Studio / vLLM / llama.cpp default to no-auth in practice. Default env vars (`LMSTUDIO_API_KEY`, `VLLM_API_KEY`, `LLAMACPP_API_KEY`) are squeezy inventions, not vendor conventions.
- **Source**: lmstudio-C1, compatible-N3.
- **Fix**: return `""` for local presets; treat empty as "no auth" in `compatible.rs:84`. Skip `Bearer` injection when key empty.
- **Effort**: XS.

### X-18 — `with_stream_retry` honors `[non-retryable]` marker / classified terminal errors
- **Severity**: HIGH (Anthropic, but cross-cutting via retry.rs)
- **Providers**: retry.rs:583-588, anthropic.rs:568-570.
- **Today**: `format_for_provider_error` prefixes `[non-retryable]` for hard 4xx bodies; `is_retryable_stream_error` never reads the marker. Hard-config 400s get retried 5×.
- **Source**: anthropic-HIGH.
- **Fix**: either strip-and-check the marker in `is_retryable_stream_error`, or introduce `SqueezyError::ProviderRequestNonRetryable`.
- **Effort**: S.

---

## 2. Critical per-provider tickets

### C-01 — Anthropic: mid-stream `event: error` retried 5×
- **Source**: anthropic-CRITICAL #1.
- **Location**: anthropic.rs:1032-1039, retry.rs:583-588.
- **Today**: Post-200 `event: error` (overloaded_error, model_context_window_exceeded, api_error) becomes `ProviderStream`, retried up to 5×. Pre-200 path runs the overflow classifier; post-200 doesn't.
- **Fix**: capture `error.type`; run `classify_terminal`; emit `ContextOverflow` for overflow; route `overloaded_error`/`rate_limit_error` through `ProviderRequest`.
- **Effort**: S.

### C-02 — OpenAI: `response.refusal.delta` silently dropped → empty completions retried forever
- **Source**: openai-CRITICAL #2.
- **Location**: openai.rs:600-608.
- **Today**: Refusal events fall into the `_ =>` unhandled arm. `response.completed` arrives with no `incomplete_details` so squeezy normalizes to `StopReason::EndTurn` → agent loop retries identical prompt forever.
- **Fix**: handle `response.refusal.delta` / `response.refusal.done`; surface visible text and map to `StopReason::Refusal`.
- **Effort**: S.

### C-03 — Bedrock: `inferenceConfig.maxTokens` never set
- **Source**: bedrock-CRITICAL #1.
- **Location**: bedrock.rs:141-200 (no `.inference_config(...)` call exists).
- **Today**: `request.max_output_tokens` discarded; every Converse call runs at the model's vendor default cap. Anthropic native path enforces it (`anthropic.rs:152-160`).
- **Fix**: build `InferenceConfiguration` from `request.max_output_tokens` (+ future temperature/stop_sequences).
- **Effort**: S.

### C-04 — Bedrock: `CacheRetention::Long` silently downgrades to 5-minute caching
- **Source**: bedrock-CRITICAL #2.
- **Location**: bedrock.rs:456-463.
- **Today**: `cache_point_block()` never calls `.ttl(CacheTtl::OneHour)`. Long retention contract honored on Anthropic native, broken on Bedrock.
- **Fix**: thread `request.effective_cache_spec().retention` into the helper.
- **Effort**: XS.

### C-05 — Google: `promptFeedback.blockReason` empty-candidates as silent success
- **Source**: google-CRITICAL #3.
- **Location**: google.rs:160-225, lib.rs:521-529.
- **Today**: Blocked prompts surface as 200 OK with empty candidates + `promptFeedback.blockReason`; `parse_google_event` never inspects that → `Completed { stop_reason: None, output: "" }`. User sees "Gemini returned no output."
- **Fix**: inspect `promptFeedback.blockReason`; error with `Google blocked prompt: {reason}`.
- **Effort**: XS.

### C-06 — Ollama: `OLLAMA_HOST` users see silent 404s on all metadata probes + pulls
- **Source**: ollama-CRITICAL #2.
- **Location**: squeezy-core/src/lib.rs:39 (bakes `/api`), `OLLAMA_HOST` not read.
- **Today**: Default URL hardcodes `/api`. `fetch_ollama_context_window`, `fetch_ollama_model_names`, `pull_model` all concatenate paths without `/api`. Users following upstream Ollama convention (`OLLAMA_HOST=http://x:11434`) hit silent 404s. `fetch_ollama_model_names` swallows errors and returns `Vec::new()`.
- **Fix**: read `OLLAMA_HOST` as fallback; introduce a URL normalizer that maps any input shape (`http://x:11434`, `http://x:11434/api`, `http://x:11434/v1`) to a canonical host root, then always prefix per-endpoint path including `/api`. Reference: codex's `base_url_to_host_root` at `others/codex/codex-rs/ollama/src/url.rs:8-18`.
- **Effort**: S.

### C-07 — LMStudio: `provider = "lmstudio"` never reaches `LMStudioProvider`
- **Source**: lmstudio-CRITICAL #1.
- **Location**: registry.rs:351-382, squeezy-core/src/lib.rs:1909-1930.
- **Today**: `ProviderConfig` has no `LMStudio` variant. `OpenAiCompatiblePreset::LMStudio` routes through `OpenAiCompatibleProvider`. The hand-written, comment-laden `lmstudio.rs` is reachable only via Ollama's compat delegate. All custom code (server-model echo, tool-id canonicalization) is invisible to real users.
- **Fix**: pick one:
  - **A**: add `ProviderConfig::LMStudio(LMStudioConfig)` variant; route the preset.
  - **B**: delete `lmstudio.rs`; teach `compatible.rs` to tolerate missing keys for LMStudio/vLLM/llama.cpp presets (overlaps with X-17).
- **Effort**: M.

### C-08 — LMStudio: default base URL drift (`localhost` vs `127.0.0.1`) — Windows IPv6 silent failure
- **Source**: lmstudio-CRITICAL #2.
- **Location**: lmstudio.rs:35 (`http://localhost:1234/v1`) vs squeezy-core/src/lib.rs:112 (`http://127.0.0.1:1234/v1`).
- **Today**: Two `pub const DEFAULT_LMSTUDIO_BASE_URL` symbols disagree. `localhost` resolves IPv6-first on Windows; LM Studio binds IPv4 → silent connect failure.
- **Fix**: delete `lmstudio.rs:35`; reuse `squeezy_core::DEFAULT_LMSTUDIO_BASE_URL`. Standardize on `127.0.0.1`.
- **Effort**: XS.

### C-09 — xAI: `is_responses_capable` predicate brittle
- **Source**: xai-CRITICAL #2.
- **Location**: xai.rs:59-78.
- **Today**: Matches first character after `grok-` against `'3'..='9'`. `grok-build-0.1` and `grok-imagine-image` (Responses-only) fall through to Chat Completions → silent 404. Future non-numeric generations break too.
- **Fix**: explicit allow-list of base families; default "unknown grok → Responses" (xAI's preferred surface). `grok-imagine-*` → structured `ProviderNotConfigured` (image endpoint not routed).
- **Effort**: S.

### C-10 — Compatible: usage payload lost after `finish_reason: stop`
- **Source**: compatible-C1.
- **Location**: compatible.rs:1006-1015 + outer loop at 563-565.
- **Today**: `parse_chat_event` sets `completed_emitted = true` inline when `finish_reason` arrives; outer loop short-circuits before the *next* SSE chunk (which carries usage) is parsed. Affects **Groq, OpenRouter-via-Groq**, and any provider that ships usage in a separate trailing chunk. **Cost reporting silently broken.**
- **Fix**: don't set `completed_emitted` inside `finish_reason` handler; keep parsing until `[DONE]` or stream end.
- **Effort**: S.

### C-11 — Compatible: Cloudflare AI Gateway dual-auth inverted
- **Source**: compatible-C2.
- **Location**: squeezy-core/src/lib.rs:2127, compatible.rs:474.
- **Today**: `CLOUDFLARE_API_KEY` ends up in `Authorization: Bearer` slot AND `cf-aig-authorization`. CF's `/compat` endpoint expects the **upstream provider's key** in `Authorization`, **CF gateway token** in `cf-aig-authorization`. Only works when proxying to Workers AI; OpenAI/Anthropic upstreams 401.
- **Fix**: add `upstream_api_key_env` field; populate `Authorization` from upstream key, `cf-aig-authorization` from CF token. Reference: opencode `cloudflare.ts:42-51`.
- **Effort**: S.

### C-12 — Compatible: Cloudflare AI Gateway `/compat` deprecated
- **Source**: compatible-C3.
- **Location**: squeezy-core/src/lib.rs:124-125.
- **Today**: Verified May 2026: CF's `/compat/chat/completions` deprecated; recommended is REST API path.
- **Fix**: switch default template to the new REST shape (or `https://gateway.ai.cloudflare.com/v1/{account_id}/{gateway_id}/openai`).
- **Effort**: XS.

---

## 3. High per-provider tickets (most impactful next)

### H-01 — Anthropic: auto cache breakpoints + caller markers can exceed 4-cap
- **Source**: anthropic-HIGH #1.
- **Location**: anthropic.rs:236-239, cache_policy.rs:206-256.
- **Fix**: 4-slot allocator with invalidation-priority order (tools → system → messages); decrement + drop-and-warn when exhausted. Reference: opencode `Cache.Breakpoints`.
- **Effort**: S.

### H-02 — Anthropic: `redacted_thinking.data` accumulation broken via `signature_delta`
- **Source**: anthropic-HIGH #2.
- **Location**: anthropic.rs:873-887, 944-957.
- **Fix**: for `Redacted` kind, treat `signature_delta.signature` as `data` accumulation.
- **Effort**: S.

### H-03 — Anthropic: `reasoning_only_stop` hard-coded `false`
- **Source**: anthropic-HIGH #3.
- **Location**: anthropic.rs:1024-1029.
- **Fix**: `reasoning_only_stop = end_turn && !saw_visible_output && !finished_thinking.is_empty();`
- **Effort**: XS.

### H-04 — Anthropic: `model_uses_adaptive_thinking` substring match on `opus-` / `sonnet-`
- **Source**: anthropic-HIGH #4.
- **Location**: anthropic.rs:54-68.
- **Fix**: anchor on `claude-opus-` / `claude-sonnet-`; require version digits followed by `-`/`@`/`:`/EOS.
- **Effort**: XS.

### H-05 — Anthropic: auth-retry layer always retries 401 even for `StaticApiKey`; OAuth `force_refresh` failure leaves `dirty=true` stuck
- **Source**: anthropic-HIGH #5.
- **Location**: retry.rs:79-102; credentials.rs:452-454; oauth/anthropic.rs:613-624.
- **Fix**: add `ApiKeySource::can_rotate() -> bool` (false for `StaticApiKey`); skip auth retry when false. In `force_refresh`, on error keep `dirty=true` AND short-circuit re-entry via a `last_refresh_err` flag.
- **Effort**: S.

### H-06 — OpenAI: `response.failed` envelope discards `code` / `param` / partial usage
- **Source**: openai-HIGH (#3).
- **Location**: openai.rs:591-599.
- **Fix**: parse `error.{code, message, param}`; map `context_length_exceeded` → `OverflowSignal::Detected`, `rate_limit_exceeded` → retry with embedded delay, `insufficient_quota` / `cyber_policy` → terminal stop. Reference: codex `responses.rs:312-345`.
- **Effort**: M.

### H-07 — OpenAI: no `response.function_call_arguments.delta` handling
- **Source**: openai-HIGH (tool streaming).
- **Location**: openai.rs:504-546.
- **Fix**: add `LlmEvent::ToolCallDelta { call_id, name, arguments_chunk }` and parse `response.function_call_arguments.delta`. Falls back to `output_item.done` aggregation when buffered.
- **Effort**: S.

### H-08 — OpenAI: `response.output_text.done` reconcile skipped
- **Source**: openai-HIGH (reconcile).
- **Location**: openai.rs:504-512.
- **Fix**: add `response.output_text.done`; compare cumulative delta buffer to `text`; emit corrective `TextDelta` for divergent suffix.
- **Effort**: S.

### H-09 — Google: tool `parameters` schema passed through unsanitized
- **Source**: google-HIGH.
- **Location**: google.rs:90-101.
- **Fix**: add `sanitize_for_gemini` (drop `additionalProperties`, deref `$ref`, ensure non-empty object properties, coerce `[...,"null"]` → `nullable:true`). Or switch to `parametersJsonSchema` which supports full JSON Schema. Reference: opencode `gemini.ts:144-162`, pi `google-shared.ts:272-288`.
- **Effort**: M.

### H-10 — Google: `functionResponse.response` always `{output: str}`; no error signal
- **Source**: google-HIGH.
- **Location**: google.rs:256-268.
- **Fix**: extend `LlmInputItem::FunctionCallOutput` with `is_error: bool`; switch key in `google.rs:265`.
- **Effort**: S.

### H-11 — Bedrock: no inference-profile / cross-region prefix detection
- **Source**: bedrock-HIGH.
- **Location**: bedrock.rs:141.
- **Fix**: detect `arn:aws:bedrock:...inference-profile/...`; auto-prefix `us.` / `eu.` / `apac.` based on region. Emit `LlmEvent::ServerModel` when rewrite fires. Claude 4.5/4.6 on Bedrock require cross-region inference profile. Reference: `others/clear-code/src/utils/model/bedrock.ts:189-265`.
- **Effort**: M.

### H-12 — Bedrock: adaptive-thinking schema wrong for Claude 4.6+
- **Source**: bedrock-HIGH.
- **Location**: bedrock.rs:165-185.
- **Fix**: mirror `anthropic.rs:186-223`. Claude 4.6+ wants `{"thinking":{"type":"adaptive"}, "output_config":{"effort":...}}`.
- **Effort**: S.

### H-13 — Bedrock: `AWS_BEARER_TOKEN_BEDROCK` cached in `OnceCell`, never refreshes
- **Source**: bedrock-HIGH.
- **Location**: bedrock.rs:60-119.
- **Fix**: re-read `AWS_BEARER_TOKEN_BEDROCK` per `client()` call. Surface `ProviderTokenExpired` for 401-ExpiredToken from Bedrock.
- **Effort**: S.

### H-14 — Ollama: `num_ctx` never set; default 4096 cripples agent flows
- **Source**: ollama-HIGH #1.
- **Location**: ollama.rs:60-67.
- **Fix**: pick 16k–32k default; stamp `options.num_ctx`. Better: probe model's `model_info.*.context_length` from `/show` (helper at `ollama.rs:139-158` already exists) and pick `min(probed, server_max)`.
- **Effort**: S.

### H-15 — Ollama: tool-call `arguments` as JSON string mishandled
- **Source**: ollama-HIGH #2.
- **Location**: ollama.rs:377-402.
- **Fix**: when `arguments` is `Value::String`, attempt `serde_json::from_str(s)`; on failure attach `INVALID_TOOL_ARGUMENTS_KEY` markers (mirror lmstudio.rs:407-419). Extract shared helper.
- **Effort**: S.

### H-16 — Ollama: `keep_alive` not plumbed; every idle resume pays load tax
- **Source**: ollama-HIGH #3.
- **Location**: ollama.rs:60-67.
- **Fix**: add `keep_alive: Option<String>` on `OllamaConfig`; plumb to request body when set.
- **Effort**: XS.

### H-17 — Ollama: thinking-model support missing (`think` param never sent)
- **Source**: ollama-HIGH #4.
- **Location**: ollama.rs:49-86.
- **Fix**: when `reasoning_effort.is_some()` or model in thinking-capable list, set `body["think"] = true` (or low/medium/high string for gpt-oss). Parse `message.thinking` → `ReasoningDelta { kind: Native }`.
- **Effort**: S.

### H-18 — LMStudio: reasoning content dropped (`delta.reasoning` / `delta.reasoning_content`)
- **Source**: lmstudio-HIGH.
- **Location**: lmstudio.rs:474-501.
- **Fix**: lift `collect_delta_text` + `reasoning_buf` + `drain_reasoning` + `reasoning_only_stop` from `compatible.rs:1052-1066`. Read `completion_tokens_details.reasoning_tokens`.
- **Effort**: S.

### H-19 — LMStudio: JIT-load 400 surfaces as raw JSON; no `ttl` field
- **Source**: lmstudio-HIGH.
- **Location**: lmstudio.rs:172-185.
- **Fix**: detect `status==400 && body.contains("not loaded")` → append hint pointing at Developer→Server Settings JIT toggle. Add `jit_ttl_seconds` config field; emit `body["ttl"] = ...`.
- **Effort**: S.

### H-20 — LMStudio: empty/whitespace SSE chunks error the stream (covered by X-02)

### H-21 — xAI: costly test bypasses `XaiProvider` — dispatcher has zero E2E coverage
- **Source**: xai-HIGH.
- **Location**: tests/xai_costly.rs:18-43.
- **Fix**: rebuild test on `XaiProvider::from_config`. Add a wiremock-based unit pair asserting `grok-4` → `/v1/responses` and `grok-2` → `/v1/chat/completions`.
- **Effort**: S.

### H-22 — xAI: extra_headers asymmetry (Responses route silently drops them)
- **Source**: xai-HIGH.
- **Location**: openai.rs:88-112.
- **Fix**: honor `extra_headers` on both routes (or partition by allowed-list per preset).
- **Effort**: S.

### H-23 — xAI: Live Search (`search_parameters` / `web_search`) unreachable
- **Source**: xai-HIGH (also relates to OpenAI hosted tools).
- **Location**: lib.rs:459-464, compatible.rs:247-296, openai.rs:218-240.
- **Fix**: extend `LlmToolSpec` with hosted-tool kind (web_search / file_search / computer_use). Wire chat: merge `search_parameters` into body. Wire Responses: append `{type:"web_search"}` tool entry. Surface `citations` via `LlmEvent::Citation` (new variant or fold into TextDelta).
- **Effort**: M.

### H-24 — Compatible: `n=1` not explicitly emitted; tool-call `index` partition missing
- **Source**: compatible-H1, H4.
- **Location**: compatible.rs request_body, 1069-1071.
- **Fix**: emit `n: 1` explicitly; partition tool-call accumulator by `(choice_index, tool_index)`; treat missing `index` as continuation of most recent active index, not 0.
- **Effort**: S.

### H-25 — Compatible: tool-call arguments unbounded accumulator (potential OOM)
- **Source**: compatible-H2.
- **Location**: compatible.rs:847-849.
- **Fix**: cap `entry.arguments.len()` at 1 MiB; synthesize invalid-arguments error past that.
- **Effort**: XS.

### H-26 — Compatible: missing `seed` / `top_p` / `temperature` / `stop` / penalty fields
- **Source**: compatible-H3.
- **Location**: compatible.rs:134-297.
- **Fix**: extend `LlmRequest` with the OpenAI-standard parameter slots; forward in `request_body`. Provides eval/determinism plumbing.
- **Effort**: M.

### H-27 — Compatible: inline mid-stream `error` JSON → `ProviderStream`, but stream-retry policy doesn't retry it even when classified-retryable
- **Source**: compatible-H7.
- **Location**: compatible.rs:1022-1027; retry.rs:46-55.
- **Fix**: classify inline error → escalate to `ProviderRequest` for retryable shapes, or extend stream policy.
- **Effort**: S.

### H-28 — Compatible: Vertex OAuth token snapshot — dies after ~1h
- **Source**: compatible-VX-1.
- **Location**: compatible.rs:93 `static_api_key_source`.
- **Fix**: introduce a `VertexOAuthSource` that calls `gcloud auth print-access-token` (or reads `GOOGLE_APPLICATION_CREDENTIALS`) on `current_key`; mirror the GitHub Copilot OAuth source pattern.
- **Effort**: M.

### H-29 — Compatible: Mistral `tool_choice = "required"` is ignored; Mistral calls it `"any"`
- **Source**: compatible-MS-1.
- **Location**: compatible.rs request_body.
- **Fix**: per-preset `tool_choice` normalization map.
- **Effort**: S.

### H-30 — Compatible: Groq `usage` chunk arrives after `finish_reason: stop` — covered by C-10 (compatible-C1).

### H-31 — Compatible: DeepSeek `reasoning_content` injection emits spurious "model finished without emitting any content" notice
- **Source**: compatible-DS-1.
- **Location**: compatible.rs:1094-1109.
- **Fix**: when `reasoning_buf` is non-empty and visible output empty, treat as normal reasoning completion; don't inject notice.
- **Effort**: XS.

### H-32 — Compatible: `parallel_tool_calls` ignored on chat path
- **Source**: compatible-M2.
- **Location**: compatible.rs request_body.
- **Fix**: forward when `Some(...)`.
- **Effort**: XS.

### H-33 — Compatible: `prompt_cache_key` clamp via truncate can silently collide across distinct prompts
- **Source**: compatible-H8.
- **Location**: compatible.rs:236.
- **Fix**: hash long keys (BLAKE3 or SHA-256 → first 32 hex) before clamping rather than truncate.
- **Effort**: XS.

---

## 4. Medium tickets (worth doing in same sprint)

Grouped by theme; cite each source.

### Errors & classification
- **M-01** Anthropic: `pause_turn` lands in `Other` — covered by X-07.
- **M-02** OpenAI: `instructions: ""` always serialized — clobbers `previous_response_id` chains. **Fix**: skip when empty. `openai.rs:160-166`. XS.
- **M-03** OpenAI: `tool_choice` dropped when `tools.is_empty()`. **Fix**: move out of guard. `openai.rs:218-240`. XS.
- **M-04** OpenAI: no org/project/`service_tier`/`OpenAI-Beta` knobs. **Fix**: add `organization`/`project`/`service_tier` to `OpenAiConfig`. `openai.rs:335-355`. S.
- **M-05** OpenAI: stale `previous_response_id` (404 `previous_response_not_found`) not gracefully recovered. **Fix**: structured `SqueezyError::ProviderStateExpired`. `openai.rs:167-169`. S.
- **M-06** OpenAI: `LlmInputItem::UserText` shape should be `[{type:"input_text"}]` array (current string form drifts from prompt-cache prefix). **Fix**: drop the string fast-path. `openai.rs:611-628`. S.
- **M-07** Google: `response_id` hard-coded to `None` despite Gemini emitting `responseId`. **Fix**: extract + pipe to `Completed`. `google.rs:216-217`. XS.
- **M-08** Google: `thoughtSignature` shape too narrow for Gemini 3 (single Option<String>). **Fix**: `Vec<(text, Option<sig>)>`. `google.rs:282-300`. S.
- **M-09** Google: `thoughtSignature` not preserved on text/tool-call parts. `google.rs:391-433`. S.
- **M-10** Google: implicit cache not surfaced via `CacheSpec`. **Fix**: flip `models.json` `prompt_caching: true` for Gemini 2.5 + document implicit-only. XS.
- **M-11** Google: `Started` emitted before any chunk → dangling on first-chunk parse error. **Fix**: defer to first parsed event. `google.rs:151`. XS.
- **M-12** Google: candidates token count excludes thinking; output_tokens under-counts (covered by X-11).
- **M-13** Google: 20MB inline image cap unenforced. **Fix**: reject upfront. `google.rs:269-277`. XS.
- **M-14** Bedrock: wildcard `_ => Ok(Vec::new())` swallows future variants. **Fix**: `tracing::debug!` with discriminant. `bedrock.rs:433`. XS.
- **M-15** Bedrock: no `ServerModel` echo on inference-profile resolution. `bedrock.rs:209-253`. S.
- **M-16** Bedrock: image block doesn't enforce 3.75 MB Claude limit. `bedrock.rs:680-699`. XS.
- **M-17** Bedrock: reasoning `signature` deltas concatenated (should be authoritative-replacement). `bedrock.rs:373-378`. XS.
- **M-18** Bedrock: 4-breakpoint cache budget tracker missing (latent bomb). `bedrock.rs:144-159, 465-477`. S.
- **M-19** Ollama: pull endpoint no concurrent-pull dedup. **Fix**: `Mutex<HashMap<String, broadcast::Receiver>>`. `ollama.rs:457-514`. M.
- **M-20** Ollama: `pull_model` uses unconfigured `reqwest::Client::new()` — no idle timeout, no shared pool. `ollama.rs:458`. S.
- **M-21** Ollama: no `/api/show` capability check before tool-calling. `ollama.rs:68-83`. S.
- **M-22** Ollama: cancellation drops usage emit; no terminal `Completed` event. `ollama.rs:222-225`. S.
- **M-23** Ollama: parse_num_ctx fallback fragile for quoted values. `ollama.rs:160-168`. XS.
- **M-24** LMStudio: `length` / `content_filter` finish reasons emit no notice. `lmstudio.rs:491-501`. XS.
- **M-25** LMStudio: no `saw_visible_output` / `reasoning_only_stop` detection. `lmstudio.rs:357-372`. S.
- **M-26** LMStudio: error envelope parsed only one level deep — `OverflowSignal` classification breaks on context-overflow. `lmstudio.rs:449-457`. S.
- **M-27** LMStudio: no arrayed `content` delta handling. `lmstudio.rs:478-481`. S.
- **M-28** LMStudio: incomplete tool call errors whole stream. `lmstudio.rs:398-427`. S.
- **M-29** LMStudio: empty `arguments` silently coerced to `{}` with no marker. `lmstudio.rs:408-419`. XS.
- **M-30** xAI: no mock unit tests for chat/Responses streaming. M.
- **M-31** xAI: `usage.cached_tokens` top-level fallback missing. `compatible.rs:1147-1151`. XS.
- **M-32** xAI: redundant API-key resolution; OAuth refresh race. `xai.rs:30-36`. S.
- **M-33** xAI: `grok-imagine-*` returns 404 with no actionable hint. S (add structured error). XS.
- **M-34** Compatible: trailing-slash + placeholder validation. **Fix**: reject placeholder values with `/`, `?`, `#`, whitespace, non-ascii. `compatible.rs:701-745`. XS.
- **M-35** Compatible: `extra_headers` clobber `HTTP-Referer` etc. due to case-insensitive collision. **Fix**: normalize header keys (or use `HeaderMap`). `compatible.rs:85-90`. XS.
- **M-36** Compatible: `reasoning_effort` sent unconditionally; Mistral 422s on unknown body fields. `compatible.rs:215-224`. Already partially covered by X-09 (catalog-driven gates).
- **M-37** Compatible: tool-call `arguments` set to `"{}"` masks server intent. **Fix**: emit `Value::Null`. `compatible.rs:885-887`. XS.
- **M-38** Compatible: `account_id` containing URL-encoded characters double-encoded. `compatible.rs:730-743`. XS.
- **M-39** Compatible: `serde_json::to_string(arguments)` loses key ordering → busts Anthropic prefix cache. **Fix**: carry raw `arguments_text` through. `compatible.rs:650-657`. S.
- **M-40** Compatible: `server_model` echo loses mid-stream provider fallback on OpenRouter. **Fix**: re-emit `ServerModel` on different echo. `compatible.rs:1033-1042`. XS.
- **M-41** Compatible: `ensure_vision_support` doesn't consult resolved Anthropic flavor when routed via OpenRouter etc. `compatible.rs:445-447`. S.
- **M-42** Compatible: OpenRouter `usage.cost` USD not surfaced. `compatible.rs:1138-1164`. XS.
- **M-43** Compatible: OpenRouter `X-OpenRouter-Title` (new) vs `X-Title` (legacy). XS.
- **M-44** Compatible: Vercel requires `provider/model` prefix; no validation. `compatible.rs:445-447`. XS.
- **M-45** Compatible: PortKey canonical header is `x-portkey-api-key`, not Bearer. S.
- **M-46** Compatible: PortKey routing-header allowlist incomplete (missing `x-portkey-trace-id`, `-metadata`, etc.). `compatible.rs:747-760`. XS.
- **M-47** Compatible: Vertex Anthropic-on-Vertex flavor unsupported; document. XS.
- **M-48** Compatible: Cloudflare Workers AI `@cf/...` model id; verify no escape. XS.
- **M-49** Compatible: Cerebras rejects `stream_options.include_usage` on legacy SKUs. **Fix**: per-model branching. `compatible.rs:210`. S.
- **M-50** Compatible: llama.cpp `stream_options.include_usage` rejected on older builds. Same as above.
- **M-51** Azure: `from_azure_config` lacks `extra_headers` slot; Entra ID auth unreachable. S.
- **M-52** Azure: deployment-name map assumes v1 GA; classic URL flow breaks. S.

---

## 5. Catalog & infrastructure tickets (housekeeping)

- **K-01** Refresh `models.json` for xAI, DeepSeek, Fireworks (covered by X-08).
- **K-02** Add registry entries for the 10 presets with zero coverage (covered by X-08).
- **K-03** Per-model `thinking_budget` ranges (covered by X-10).
- **K-04** Set `reasoning_effort: true` on actual reasoning models (covered by X-09).
- **K-05** Quarterly catalog-refresh CI lint that diffs `models.json` against vendor docs.
- **K-06** Default model migrations: `grok-4` → `grok-4.3`; `deepseek-chat` → `deepseek-v4-flash`; Fireworks llama-v3p3 → llama-v4-*.
- **K-07** `is_full_tier`/`models.json` divergence: PortKey claims full-tier but has zero entries — pick a side. `lib.rs:2048-2059`.
- **K-08** Display name "Mistral La Plateforme" — pick canonical brand. `lib.rs:2030`.
- **K-09** Stale comment in `compatible.rs:5-6` claims xAI routes through compat (it doesn't for Grok 3+). Remove.

---

## 6. Test-coverage tickets

A parameterized mock-server harness covers most of the 18 OpenAI-compat presets in one file without API keys. Plus per-provider gaps:

### Universal (shared core, all SSE providers)
- **T-01** Mock SSE empty `data:` heartbeat (covers X-02). Test in `sse_tests.rs`. XS.
- **T-02** Mock mid-stream truncation → assert `with_stream_retry` reconnect, no duplicate prefix (X-01). M.
- **T-03** Mock post-`finish_reason` usage chunk → assert cost is captured (C-10 / Groq). S.
- **T-04** Inline mid-stream `error` JSON → assert classification (H-27). S.

### Anthropic
- **T-05** Mid-stream `event: error` → `ProviderRequest + ContextOverflow` (not retried) (C-01). S.
- **T-06** Redacted thinking round-trip via `signature_delta` (H-02). S.
- **T-07** Adaptive thinking + zero text → `reasoning_only_stop=true` (H-03). S.
- **T-08** 4-breakpoint cap exhaustion (H-01). S.
- **T-09** Tool-result with image bytes → `tool_result.content: Array<image>` (X-06). S.
- **T-10** OAuth `force_refresh` failure doesn't loop on next `current_key` (H-05). S.
- **T-11** Non-Claude model id with `opus-4-7` substring does NOT activate adaptive (H-04). XS.
- **T-12** OAuth costly smoke (gated on `SQUEEZY_RUN_OAUTH_COSTLY_TESTS=1`). M.

### OpenAI
- **T-13** `response.refusal.delta` → `StopReason::Refusal` with visible output (C-02). S.
- **T-14** `response.failed` with each `error.code` (`context_length_exceeded`, `rate_limit_exceeded`, `insufficient_quota`, `cyber_policy`) (H-06). M.
- **T-15** `response.function_call_arguments.delta` → incremental `ToolCallDelta` (H-07). S.
- **T-16** `response.output_text.done` reconcile (H-08). S.
- **T-17** Stale `previous_response_id` 404 → typed error (M-05). S.
- **T-18** Org/project/service_tier headers on the wire (M-04). XS.

### Google
- **T-19** `MALFORMED_FUNCTION_CALL` finishReason (X-07 / C-05). S.
- **T-20** Empty-candidates with `promptFeedback.blockReason` (C-05). S.
- **T-21** Parallel tool calls across separate SSE chunks → distinct `call_id`s (X-03). S.
- **T-22** `thoughtSignature` round-trip preservation (M-08). S.
- **T-23** `reasoning_effort: xhigh` budget within model max (X-10). XS.
- **T-24** Tool-schema sanitization (H-09). M.
- **T-25** `tool_choice` forwarding into `toolConfig` (X-04). XS.
- **T-26** `output_schema` into `generationConfig.responseSchema` (X-05). S.
- **T-27** 20MB inline-image guardrail (M-13). XS.

### Bedrock
- **T-28** `inferenceConfig.maxTokens` round-trip (C-03). XS.
- **T-29** Inference profile prefix rewriting (H-11). S.
- **T-30** Mid-stream `ModelStreamErrorException` retry (X-01). S.
- **T-31** Adaptive-thinking schema selection by model id (H-12). S.
- **T-32** `CacheRetention::Long` ttl assertion (C-04). XS.
- **T-33** Token-usage accounting (X-11) — `inputTokens=1000, cacheRead=900, cacheWrite=50` → `input_tokens == 1000`. XS.
- **T-34** Bearer-token rotation per `client()` (H-13). S.
- **T-35** Add `bedrock_smoke.rs` against free `bedrock-runtime list-foundation-models` endpoint (gated `SQUEEZY_RUN_FREE_TESTS=1`). M.

### Ollama
- **T-36** `done_reason: "load"` / `"unload"` no-op then content keeps streaming (X-07 / ollama-C1). XS.
- **T-37** URL-shape variations: `{http://x:11434, http://x:11434/, /api, /v1}` → correct paths (C-06). S.
- **T-38** Mocked end-to-end `/api/chat` test (currently only network-gated smoke). M.
- **T-39** Tool_calls with string-encoded arguments (H-15). S.
- **T-40** OpenAI-compat delegate streaming through `LMStudioProvider` (C-07 unblocks). S.
- **T-41** Mid-stream cancel emits `Completed` (M-22). XS.

### LMStudio
- **T-42** `reasoning_content` / `delta.reasoning` (H-18). S.
- **T-43** "model not loaded" 400 hint (H-19). XS.
- **T-44** Empty `data:\n` / whitespace `[DONE]` (X-02). XS.
- **T-45** Arrayed `content` delta (M-27). S.
- **T-46** Finish-reason `length` / `content_filter` notice (M-24). XS.
- **T-47** Incomplete tool-call skip (M-28). XS.

### xAI
- **T-48** Dispatcher: `grok-4` → `/v1/responses`, `grok-2` → `/v1/chat/completions` (H-21). S.
- **T-49** `reasoning_effort` reaches Responses body for reasoning-capable Grok (X-09). XS.
- **T-50** `extra_headers` asymmetry (H-22). S.
- **T-51** xAI `usage.cached_tokens` top-level fallback (M-31). XS.
- **T-52** Multi-segment aggregator prefix (`openrouter/xai/grok-4`) (xai-LOW). XS.

### Compatible (parameterized harness)
- **T-53** Mock-server matrix: spin one fake `/chat/completions` per preset; assert auth header, URL after substitution, canned SSE script:
  - Plain content + `finish_reason: stop` + usage chunk + `[DONE]` (catches C-10).
  - Two tool calls overlapping `index` (X-03 / H-24).
  - Inline `error: {...}` mid-stream (H-27).
  - `[DONE]` joined to previous chunk (compatible-L4).
  - `reasoning_content` arrayed shape (M-27).
  - Effort: L (one harness, 18 cases).
- **T-54** Per-preset 401-ping: assert error message names the preset and signals auth-failed (catches PK-1, CFAG-1, VX-1). M.
- **T-55** Endpoint snapshot test: each preset `default_base_url()` matches a hand-curated table; CI lint to refresh quarterly (catches CFAG-2, FW-1, DS-2 rot). S.

---

## 7. Quick wins (<1 hour each — land in one PR)

These are XS-effort fixes worth bundling. Most are 1–5 LoC.

| # | Provider | What | Where |
|---|----------|------|-------|
| Q1 | All | Stamp `User-Agent: squeezy-cli/<v>` (X-13) | transport.rs:96-110 |
| Q2 | All | `.connect_timeout(30s)` + `.tcp_keepalive(60s)` (X-12) | transport.rs:96-110 |
| Q3 | All | Stop reasons: log `tracing::warn!` on `Other(...)` (X-07 partial) | lib.rs:498-551 |
| Q4 | Anthropic | `tool_choice` forwarding (X-04) | anthropic.rs:144-242 |
| Q5 | Anthropic | `User-Agent` on API-key path | anthropic.rs:517-525 |
| Q6 | Anthropic | `merge_oauth_beta_header` case-insensitive dedup | anthropic.rs:272-294 |
| Q7 | Anthropic | `max_tokens` clamp against registry max | anthropic.rs:152-160 |
| Q8 | OpenAI | Skip `instructions: ""` (M-02) | openai.rs:160-166 |
| Q9 | OpenAI | `tool_choice` outside tools guard (M-03) | openai.rs:218-240 |
| Q10 | OpenAI | Skip empty-`type` trace line | openai.rs:480-484 |
| Q11 | OpenAI | Remove `[DONE]` dead branch (Responses never emits it) | openai.rs:474-476 |
| Q12 | Google | Pipe `responseId` to `Completed` (M-07) | google.rs:216-217 |
| Q13 | Google | Defer `Started` to first parsed event (M-11) | google.rs:151 |
| Q14 | Google | Reject inline-image > 20 MB upfront (M-13) | google.rs:269-277 |
| Q15 | Google | Set `models.json: prompt_caching = true` for Gemini 2.5 | models.json |
| Q16 | Bedrock | `tracing::debug!` on wildcard variant (M-14) | bedrock.rs:433 |
| Q17 | Bedrock | Image size guard (3.75 MB Claude) (M-16) | bedrock.rs:680-699 |
| Q18 | Bedrock | Reasoning `signature` authoritative-replace (M-17) | bedrock.rs:373-378 |
| Q19 | Ollama | Add `keep_alive` config field (H-16) | ollama.rs, squeezy-core |
| Q20 | Ollama | Bump `fetch_ollama_*` timeouts (250ms → 1s) | ollama.rs:88-110 |
| Q21 | LMStudio | Standardize `DEFAULT_LMSTUDIO_BASE_URL` on `127.0.0.1` (C-08) | lmstudio.rs:35 |
| Q22 | LMStudio | `length`/`content_filter` notice (M-24) | lmstudio.rs:491-501 |
| Q23 | xAI | Default model → `grok-4.3` after K-01 | squeezy-core/src/lib.rs:89 |
| Q24 | xAI | `rsplit_once('/')` for multi-segment prefix (xai-LOW) | xai.rs:64 |
| Q25 | Compatible | Cap tool-arg accumulator at 1 MiB (H-25) | compatible.rs:847-849 |
| Q26 | Compatible | Hash long `prompt_cache_key` instead of truncate (H-33) | compatible.rs:236 |
| Q27 | Compatible | Drop duplicate `eprintln!` in tool-call drain (compatible-L1) | compatible.rs:866-883 |
| Q28 | Compatible | URL placeholder value validation (M-34) | compatible.rs:701-745 |
| Q29 | Compatible | Update Cloudflare `/compat` → REST default (C-12) | squeezy-core/src/lib.rs:124-125 |
| Q30 | Compatible | Skip `Bearer` on empty key for local presets (X-17) | compatible.rs:84 |

---

## 8. Suggested PR sequencing

1. **PR 1 — shared infra quick wins**: Q1, Q2, Q3, X-02 (SSE empty), X-12, X-13. *Lands in <1 day.*
2. **PR 2 — catalog refresh**: X-08, X-09, K-06 (default model migrations), Q15, Q23, Q29. *Critical correctness, also unlocks X-10.*
3. **PR 3 — stream-retry rollout**: X-01 (covers H-30, T-30, T-02). *Closes 3 HIGHs in one go.*
4. **PR 4 — tool/output schema fan-out**: X-04, X-05, Q4, Q9. *Closes 6 tickets across providers.*
5. **PR 5 — Ollama fundamentals**: C-06 (URL), H-14 (num_ctx), H-15 (tool args), H-16 (keep_alive), H-17 (thinking), X-07 partial. *Most-painful local-provider gaps closed.*
6. **PR 6 — LMStudio routing decision**: C-07 (pick A or B), C-08, X-17, H-18, H-19. *Resolves drift between two LM Studio paths.*
7. **PR 7 — Bedrock contract fixes**: C-03, C-04, H-11, H-12, H-13, T-28, T-32. *Removes the cache + token + reasoning silent-correctness gap.*
8. **PR 8 — Compatible aggregator core**: C-10, C-11, C-12, H-24, H-25, X-16 (SSRF). *Touches every preset.*
9. **PR 9 — Compatible test harness (T-53)**: parameterized mock matrix. *Unlocks zero-key CI coverage for the 10 untested presets.*
10. **PR 10 — xAI dispatcher**: C-09, H-21, H-22, H-23, T-48–T-52. *Decoupled from above; can ship in parallel.*

---

## 9. Quick reference — count by severity

| Severity | Count | Notes |
|----------|------:|-------|
| Critical | 18 | All listed in §2; X-02 / X-03 / X-06 / X-07 / X-08 close 8 of them as cross-cutting work |
| High | 47 | §1 cross-cutting closes ~15 of them; §3 lists the rest |
| Medium | 70 | §1 closes ~12; §4 enumerates 52, remaining ~6 are duplicates of cross-cutting |
| Low | 39 | Mostly absorbed by §7 quick wins or §4 cleanup |
| Nit | 31 | §5 housekeeping or Q-list |

**Tickets to track**: 18 cross-cutting (X-01..X-18) + 12 critical (C-01..C-12) + 33 high (H-01..H-33) + 52 medium (M-01..M-52) + 30 quick wins (Q1..Q30) + 9 catalog (K-01..K-09) + 55 tests (T-01..T-55) = **~209 actionable tickets**.

Many overlap; the §8 PR sequencing collapses them into ~10 PRs.

---

## 10. Per-preset addenda (18 dedicated audits)

Section 1–9 above was generated after one aggregated audit of `OpenAiCompatibleProvider` covered all 18 presets together. After the user pointed out per-preset depth was needed, we ran 18 additional dedicated audits (one per preset, including Azure OpenAI which routes through the OpenAI Responses path, not `compatible.rs`). Reports under `.audit/providers/preset-*.md`.

Three corrections to tickets above, then new per-preset findings.

### 10.1 Corrections to earlier tickets

- **H-29 is partly wrong (Mistral `tool_choice`)** — Mistral's current OpenAPI schema accepts `tool_choice = "required"` alongside `"auto"`, `"none"`, `"any"`. Squeezy's existing string pass-through at `compatible.rs:292-294` is correct. (The capabilities guide page lists only `"any"|"auto"|"none"` and misled the shared audit; the API schema is authoritative.) Drop H-29; replace with **MIS-2/3** below.
- **N3 (`VLLM_API_KEY` invented)** — partly wrong. `VLLM_API_KEY` IS the official env var per vLLM security docs (interchangeable with `--api-key` flag). The empty-key blocker (X-17) still applies, but the env-var name is canonical. Keep X-17, drop the "invented" claim from N3 for vLLM.
- **C-10 doesn't apply to Together AI** — verified via opencode recording fixture: Together's terminal `usage` rides the same chunk as `finish_reason: "stop"`, not a trailing usage-only chunk. C-10 still applies to Groq, Cerebras, and OpenRouter (when proxying Groq). Note in the C-10 ticket: per-preset behavior varies.

### 10.2 New per-preset criticals

| ID | Preset | Title | Source | Location |
|---|---|---|---|---|
| **C-13** | Azure | `?api-version=v1` is wrong default for v1 GA Responses — needs `?api-version=preview` until Microsoft removes Next-Generation-APIs feature flag. **Default config 404s out of the box.** | `preset-azure-openai.md` AZ-C1 | `squeezy-core/src/lib.rs:35` |
| **C-14** | Azure | Azure content-filter envelope (input-blocked 400 with `error.code == "content_filter"` carrying per-category severity; mid-stream `response.incomplete.content_filter_result`) dropped — both treated as bare provider errors. | `preset-azure-openai.md` AZ-C2 | `openai.rs:600-608` + HTTP-error path |
| **C-15** | Cerebras | `DEFAULT_CEREBRAS_MODEL = "llama-3.3-70b"` retired 2026-02-16 → cold-start 400 on fresh `CEREBRAS_API_KEY`. Recommended replacement: `gpt-oss-120b`. | `preset-cerebras.md` CB-PR-1 | `squeezy-core/src/lib.rs:105` |
| **C-16** | DeepSeek | `deepseek-chat` retires **2026-07-24 15:59 UTC**. Squeezy catalog still uses V3 pricing — but DeepSeek already silently routes traffic to V4-Flash. **Every DeepSeek call today overcharges ~2×** in cost telemetry; context window (131k → 1M) and `max_output_tokens` (8k → 384k) also stale. Confirmed by opencode fixture `.../deepseek-streams-text.json:24` echoing `"model":"deepseek-v4-flash"` for a `deepseek-chat` request. | `preset-deepseek.md` DS-2 escalated | `squeezy-core/src/lib.rs:91`, `models.json:773-832` |
| **C-17** | Groq | `kimi-k2-instruct` registry row retired 2025-09-10; no curated rows for current flagships (`gpt-oss-{120,20}b`, `llama-4-scout`, `qwen-3-32b`). Catalog drift breaks cost reporting + vision gating. | `preset-groq.md` GQ-PR-1 | `models.json:594-682` |
| **C-18** | Groq | `vision: false` on every Groq registry row → `ensure_vision_support` (`lib.rs:344-355`) blocks Llama-4-Scout image attachments at the adapter before they hit the wire. | `preset-groq.md` GQ-PR-2 | `models.json:594-682` |
| **C-19** | Vercel | Default model id uses **dash** form (`anthropic/claude-opus-4-7` at `lib.rs:82`) but Vercel requires **dot** form (`anthropic/claude-opus-4.7`). Bug mirrored in `models.json:520, 545, 570` and `tests/vercel_costly.rs:14`. **First-time users 400 on first request.** | `preset-vercel.md` VL-A | `squeezy-core/src/lib.rs:56,82`, `models.json`, `tests/vercel_costly.rs:14` |

### 10.3 New per-preset highs

| ID | Preset | Title | Source | Location |
|---|---|---|---|---|
| **H-34** | Azure | `AzureOpenAiConfig` has no `extra_headers` slot. Blocks APIM (`Apim-Subscription-Key`), Entra `Authorization`, `x-ms-*` correlation. | AZ-H1 | `squeezy-core/src/lib.rs:2258-2275` |
| **H-35** | Azure | No Entra ID / managed-identity Bearer path. Auth hard-coded to `api-key` via `provider_name == "azure_openai"`. | AZ-H2 | `openai.rs:342-346` |
| **H-36** | Azure | Classic `/openai/deployments/{deployment}/responses` URL shape silently 404s — opencode/pi/codex detect; squeezy doesn't. | AZ-H3 | `openai.rs:316-355` |
| **H-37** | Azure | `store: true` Azure default not applied. Codex flips it; squeezy forwards `previous_response_id` unconditionally → multi-turn replay intermittently 400 with `response not found`. | AZ-H4 | `openai.rs:167-169` |
| **H-38** | Baseten | Per-deployment URL (`https://model-{id}.api.baseten.co/environments/production/sync/v1`) unaddressable via preset — users downgrade to `Custom` and lose env autoload + provider label + TOML section. Add `{deployment_id}` placeholder + `BasetenDeployment` sibling preset. | BT-PR-1 | `squeezy-core/src/lib.rs:108-109` |
| **H-39** | Baseten / vLLM | `chat_template_args.enable_thinking` not wired. All reasoning-capable shared models on Baseten (DeepSeek V4 Pro, Kimi K2.5/K2.6, GLM 4.7/5/5.1, Nemotron) silently never enter thinking mode. Same for vLLM with `--reasoning-parser`-enabled servers. Reference: `others/opencode/.../transform.ts:1070-1075`. | BT-PR-2, VL_VLLM-4 | `compatible.rs:215-224` |
| **H-40** | CF AI Gateway | When CFAG-2 fix migrates to the new REST API, gateway selection moves from URL segment to `cf-aig-gateway-id` HEADER. Squeezy emits no such header → gateway id silently dropped on migration, routes through default gateway. | `preset-cloudflare-ai-gateway.md` CFAG-3 | `squeezy-core/src/lib.rs:124-125` (after C-12 migration) |
| **H-41** | CF AI Gateway | Of 18 documented `cf-aig-*` request/response headers, squeezy exposes exactly one (`cf-aig-authorization`) as typed config. Cache, cost, retry, metadata, observability all require raw-string `extra_headers` paste, locked at provider-construction with no per-request override. | CFAG-4 | `compatible.rs:85-90` |
| **H-42** | CF Workers AI | `CLOUDFLARE_API_TOKEN` env alias missing — vendor docs use this name in cURL examples; squeezy only honors `CLOUDFLARE_API_KEY`. | `preset-cloudflare-workers-ai.md` CWAI-4 | `squeezy-core/src/lib.rs:2126` |
| **H-43** | CF Workers AI | DeepSeek-R1-distill / Kimi K2.6 / Gemma 4 emit reasoning as inline `<think>` tags on Workers AI's OpenAI-compat path (no `reasoning_content` field). Squeezy renders them as visible text instead of routing to `ReasoningDelta`. | CWAI-6 | `compatible.rs:1053-1054` |
| **H-44** | Custom | SSRF: `check_base_url_scheme` only policies `http://` — `https://169.254.169.254/...` (AWS IMDS via HTTPS), `https://metadata.google.internal/...`, `https://[::1]:443` all pass cleanly; API key flows there via `bearer_auth`. **M-5 covered only the http:// half.** | `preset-custom.md` CT-SEC-1 | `squeezy-core/src/lib.rs:8564-8590` |
| **H-45** | Custom | DNS rebinding bypasses any string-level allow-list — `shared_client` uses reqwest's default re-resolving GAI resolver. Vaultwarden / activitypub-federation-rust CVEs document this exact class. | CT-SEC-2 | `transport.rs:68-110` |
| **H-46** | Custom | `api_key_env = ""` + user-supplied `Authorization` in `extra_headers` → silent empty-Bearer override at `compatible.rs:474` (`bearer_auth("")` clobbers the user's header). | `preset-custom.md` CT-FN-1 | `compatible.rs:474` |
| **H-47** | DeepInfra | DeepInfra missing from `PROVIDERS` const at `registry.rs:212-237` AND zero `models.json` rows. Compound: cost reports `None`, vision-capable Llama 4-Scout / Qwen3.6 blocked by `ensure_vision_support`, `PROVIDERS`-iterating tooling silently skips DeepInfra. | `preset-deepinfra.md` DI-3 | `registry.rs:212-237`, `models.json` |
| **H-48** | DeepInfra | No CLI auth row (`squeezy-cli/src/auth.rs:28-151`); `DEEPINFRA_TOKEN` (canonical per vendor docs) not recognized — only Vercel-AI-SDK-style `DEEPINFRA_API_KEY`. | DI-4 | `squeezy-cli/src/auth.rs:28-151` |
| **H-49** | DeepSeek | V4 controls thinking via `body["thinking"] = {"type":"enabled", "budget_tokens": N}` (not `reasoning_effort`). V4-Pro defaults to thinking-on; users can't disable it for cheap turns. V4-Pro promo discount ended 2026-05-31 15:59 UTC (price tripled). | `preset-deepseek.md` DS-4 escalated | `compatible.rs:215-224` |
| **H-50** | DeepSeek | Reasoning/content ordering loss — `compatible.rs:1052-1066` doesn't flush `reasoning_buf` when a `content` delta arrives. V4-Pro's documented interleaved `reasoning → content → reasoning → content` pattern collapses into one concatenated `ReasoningDone` at end-of-turn, out of position. | `preset-deepseek.md` DS-5 | `compatible.rs:1052-1066` |
| **H-51** | Groq | `service_tier` (`on_demand`/`flex`/`auto`) unreachable; paid-tier users can't opt into Groq's flex tier (10× rate limits); `498 capacity_exceeded` falls through `format_chat_error` default. | `preset-groq.md` GQ-PR-6 | `LlmRequest` schema |
| **H-52** | Groq | `reasoning_format` and `include_reasoning` missing from `LlmRequest`. Squeezy's unconditional `reasoning_effort` + nested `reasoning.effort` emission is OpenRouter-shaped (Groq ignores nested form). Actual Groq controls are model-family-specific: `gpt-oss-*` → `include_reasoning`; Qwen/DeepSeek-R1-distill → `reasoning_format`, mutually exclusive. | `preset-groq.md` GQ-PR-7 | `compatible.rs:215-224` |
| **H-53** | LlamaCPP | No loopback gate when auth empty — `base_url = "http://gpu-cluster.internal:8080/v1"` against a `--api-key`-less server ships prompts plaintext over the network. | `preset-llamacpp.md` LC-PR-3 | `compatible.rs:439-528` |
| **H-54** | LlamaCPP | Tool calling requires server-side `--jinja`; without it, `tools: [...]` returns HTTP 500 surfaced as raw text with no actionable hint. | `preset-llamacpp.md` LC-PR-4 | `compatible.rs:976-998` |
| **H-55** | Mistral | `compatible.rs:215-224` emits both `reasoning_effort` AND `reasoning: {effort}`. The second 422s on Mistral with `extra_forbidden`. Also Mistral enum is `"none"\|"high"` only — `"low"/"medium"/"minimal"` 422 with `enum_violation`. | `preset-mistral.md` MIS-2 | `compatible.rs:215-224` |
| **H-56** | Mistral | `compatible.rs:245` emits `prompt_cache_retention: "24h"` for long retention — not in Mistral schema, 422 every request that flips the knob. | MIS-3 | `compatible.rs:245` |
| **H-57** | Mistral | Empty `models.json` + fallback `vision: false` → every image-bearing Mistral prompt hard-fails `ensure_vision_support`, even on vision-capable Pixtral Large, Ministral 3, `mistral-large-2512`. | MIS-10 | `models.json` (no rows), `lib.rs:344-357` |
| **H-58** | Mistral | `DEFAULT_MISTRAL_MODEL = "mistral-large-latest"` — `-latest` aliases removed June 2026 (target id rotates between dev/prod accounts). Pin to `mistral-large-2512`. | MIS-1 | `squeezy-core/src/lib.rs:99` |
| **H-59** | PortKey | Auth canonically inverted — PortKey's docs and PortKey's own Claude Code integration use `x-portkey-api-key` for the PortKey key and reserve `Authorization: Bearer` for the upstream provider's credential. Squeezy `bearer_auth(key)` works as legacy alias but blocks BYO-upstream-key mode. (Same class as CFAG-1.) | `preset-portkey.md` P1 | `compatible.rs:474` |
| **H-60** | PortKey | Default model `anthropic/claude-opus-4-7` likely broken on Model-Catalog accounts. PortKey 2026 routes by `@<integration-slug>/<model>`, virtual key, config id, or `x-portkey-provider`. Bare `anthropic/...` matches none → 400. User's "works manually" baseline likely depends on virtual key configured out-of-band. | P2 | `squeezy-core/src/lib.rs:84` |
| **H-61** | Together | Zero `models.json` entries → `vision: false` fallback blocks Llama-4 Maverick / Qwen3.5 images at `compatible.rs:445-447`. | `preset-together.md` TG-PR-2 | `models.json` (no rows) |
| **H-62** | Vercel | `reasoning_effort` shape: `openai/gpt-5.5` on Vercel requires `providerOptions.openai.{reasoningEffort, reasoningSummary}`; Anthropic on Vercel wants `providerOptions.anthropic.thinkingBudget`. Squeezy's top-level `reasoning_effort` silently dropped for both. | `preset-vercel.md` VL-F | `compatible.rs:215-224` |
| **H-63** | Vertex | Default `google/gemini-2.5-pro` retirement window Oct 16 2026. | `preset-vertex.md` VX-A | `squeezy-core/src/lib.rs:96`, `models.json:837-846` |
| **H-64** | Vertex | `vertex_base_url` (`lib.rs:133-137`) hard-codes regional `{location}-aiplatform.googleapis.com` shape. The `global` location requires bare `aiplatform.googleapis.com`. Gemini 3 models are only addressable via `global` → invalid DNS host. | `preset-vertex.md` VX-B | `squeezy-core/src/lib.rs:133-137` |
| **H-65** | Vertex | Vertex's OpenAI-compat layer expects Gemini thinking budgets through `extra_body.google.thinking_config.thinking_budget`. Squeezy emits OpenAI-style `reasoning_effort` which Vertex doesn't translate. `--reasoning high` is a silent no-op. | `preset-vertex.md` VX-C | `compatible.rs:215-224` |
| **H-66** | vLLM | Spurious "no content" notice (DS-1 / H-31 class) hits vLLM hardest because `--reasoning-parser deepseek_r1/qwen3/gpt_oss` enables reasoning streaming on a wide model surface. | `preset-vllm.md` VL_VLLM-2 | `compatible.rs:1094-1109` |

### 10.4 New per-preset mediums (compressed list)

- **M-53** Azure: vision gate fails closed for unmapped custom deployment ids (AZ).
- **M-54** Azure: `models.json` lacks `o3` / `o4-mini` / `gpt-4o` / `gpt-4.1` / `gpt-5.4-pro` (AZ).
- **M-55** Azure: `?api-version=` concatenation isn't URL-encoded (AZ).
- **M-56** Azure: `doctor` probes `/models` which Azure doesn't serve under `/openai/v1` (AZ).
- **M-57** Baseten: Reasoning-only-stop notice misfires on Baseten DeepSeek-R1/V4 (BT-PR-5, shares DS-1).
- **M-58** Cerebras: `max_tokens` vs `max_completion_tokens` — alias accepted today but Cerebras API v2 default-switchover 2026-07-21 tightens validation (CB-PR-2).
- **M-59** CF AI Gateway: `cf-aig-cache-status: HIT` response header ignored → squeezy bills cached responses (CF bills $0 on hits) (CFAG-8).
- **M-60** CF AI Gateway: Neither sends `cf-aig-custom-cost` nor parses CF-reported cost (CFAG-9).
- **M-61** CF Workers AI: Merged finish_reason+usage chunk untested → spurious tool-only-stop notice fires on tool-only turns (CWAI-5).
- **M-62** CF Workers AI: Native Cloudflare `errors` envelope (`{success:false, errors:[…]}`) not parsed by `format_chat_error` (CWAI-9).
- **M-63** Custom: `extra_headers` not redacted in `inspect_redacted` because `ProviderSettings::headers` carries no `#[serde(serialize_with = "redact_secret_opt")]` guard — CT-2 workaround leaks the secret (CT-SEC-4).
- **M-64** Custom: No operator-mode allow-list, no first-use confirmation, no project-scope hijack guard. Malicious `./squeezy.toml` with `base_url = "https://attacker/v1"` + `api_key_env = "OPENAI_API_KEY"` exfiltrates the user's OpenAI key (CT-SEC-5).
- **M-65** Custom: Header `\r\n` smuggling blocked by `http` crate but surfaced as opaque "builder error" at `.send()` time, not config-load (CT-SEC-3). Codex sanitizes at config-load.
- **M-66** DeepInfra: DeepInfra's chat docs only enumerate `tool_choice ∈ {"auto","none"}` — squeezy's recommended `"required"` may 4xx or coerce (DI-6).
- **M-67** DeepInfra: No `GET /v1/openai/models` credential probe (DI-9). opencode uses this for free validation.
- **M-68** DeepSeek: `deepseek-vision-preview` beta exists; squeezy hard-codes `vision: false` for all DeepSeek models, blocking image attachments (DS-6).
- **M-69** DeepSeek: `finish_reason: "insufficient_system_resource"` (concurrency-limit) falls through `compatible.rs:1129` to `_ => {}` — no notice, no tool-call drain, no retry classification (DS-7).
- **M-70** DeepSeek: `prompt_caching: false` in `models.json` contradicts DeepSeek's automatic context-prefix caching (DS-9).
- **M-71** Fireworks: Fireworks ships 3 API surfaces (chat-completions, /responses with MCP, Anthropic-compat /v1/messages). Squeezy reaches only chat-completions. Pi exposes Fireworks exclusively via Anthropic surface (13 curated models). FW alternative.
- **M-72** Mistral: `format_chat_error` doesn't parse Mistral's `{ object:"error", message:{detail:[...]}, type, raw_status_code }` envelope — message is an object not a string (MIS-11). Users see raw body, not the `extra_forbidden.loc` field telling them which body field was rejected.
- **M-73** Mistral: Mistral's canonical prefix-cache is the `x-affinity` HEADER, not body field. Squeezy's body-only emission yields zero cache hits (MIS-4).
- **M-74** Mistral: When H-26 lands, `seed` needs per-preset renaming to `random_seed` for Mistral (MIS-5).
- **M-75** Mistral: Mistral tool-call ids must match `^[a-zA-Z0-9]{9}$`; squeezy's canonical `call_<N>` rejected on replay (MIS-8).
- **M-76** OpenRouter: No OAuth/PKCE flow despite OpenRouter publishing one explicitly for CLI integration (OR-5).
- **M-77** OpenRouter: `prompt_tokens_details.cache_write_tokens` dropped, masking cache-write billing on first turn of Anthropic-via-OpenRouter sessions (OR-7).
- **M-78** OpenRouter: Per-chunk `provider` field (canonical "which upstream answered" signal) read nowhere — pairs with OR-4 / M-10 fix (OR-15).
- **M-79** PortKey: `portkey_routing_header_present` allow-list (`compatible.rs:747-760`) missing `x-portkey-custom-host`, `x-portkey-trace-id`, `x-portkey-metadata`, `x-portkey-cache-namespace`, and the v2.8.0 `x-portkey-azure-entra-scope` + v2.9.0 `x-portkey-sensitive-headers` (P4). Note: shared PK-2 referenced `x-portkey-router` — agent confirmed this header **does not exist** in PortKey's changelog (speculative claim).
- **M-80** PortKey: Error hint `@open-ai/gpt-5.5` slug at `compatible.rs:515-519` is fabricated — PortKey slugs are user-defined (`@openai-prod`, etc.) (P5/P6). Also recommends `GET /v1/models` which returns catalog, not workspace slugs.
- **M-81** Vercel: `models.json` claims `prompt_caching: false` for all 3 Vercel entries despite both explicit (Anthropic) and implicit (OpenAI/Google) caching being live (VL-G).
- **M-82** Vercel: `google/gemini-2.5-pro` stale; current is `google/gemini-3.1-pro-preview` (VL-H).
- **M-83** Vercel: `reasoning_details` shape (with `signature`, `format`, `index`) not decoded — Anthropic prefix-cache benefit lost on next turn (VL-I).
- **M-84** Vertex: PROVIDERS.md promises `service_account_json` config and automatic token refresh — neither exists. `grep service_account_json crates/` is empty (VX-D).
- **M-85** Vertex: No Workload Identity Federation hook (VX-I).
- **M-86** vLLM: Empty default `model = ""` 400s with no hint; should probe `GET /v1/models` (VL_VLLM-6).
- **M-87** vLLM: Prefix-cache hits never populate `usage.prompt_tokens_details.cached_tokens` on vLLM; ledger shows `None` (VL_VLLM-5).

### 10.5 Catalog updates absorbed into X-08

- xAI default `grok-4` → `grok-4.3` (K-06) — already tracked.
- DeepSeek default `deepseek-chat` → `deepseek-v4-flash` (K-06, **CRITICAL via C-16**).
- Fireworks default `llama-v3p3-70b` → newer SKU (K-06).
- Cerebras default `llama-3.3-70b` → `gpt-oss-120b` (**CRITICAL via C-15**).
- Together default `Llama-3.3-70B-Instruct-Turbo` → `openai/gpt-oss-120b` recommended.
- Baseten default `Meta-Llama-3.1-70B-Instruct` → stale; shared-catalog 404 (BT-PR-3).
- DeepInfra default `Meta-Llama-3.1-70B-Instruct` → no longer flagship (DI-2).
- Mistral default `mistral-large-latest` → `mistral-large-2512` (H-58).
- Vertex default `google/gemini-2.5-pro` → `google/gemini-3.1-pro-preview` once available; pin retirement window (H-63).
- Vercel default model id dash→dot conversion (**CRITICAL via C-19**).

### 10.6 Test coverage additions

- **T-56** Azure: `?api-version=preview` round-trip; classic URL detection. AZ-C1/H3.
- **T-57** Azure: content-filter envelope per-category surfacing. AZ-C2.
- **T-58** Cerebras: registry refresh smoke. C-15.
- **T-59** CF AI Gateway: `cf-aig-cache-status: HIT` → cost = 0. M-59.
- **T-60** CF Workers AI: `<think>` tag extraction → `ReasoningDelta`. H-43.
- **T-61** Custom: SSRF — block `https://169.254.169.254`, `https://metadata.google.internal`, `https://[::1]`. H-44.
- **T-62** Custom: DNS-rebinding integration test (mock DNS resolver). H-45.
- **T-63** Custom: header `\r\n` rejected at config-load. M-65.
- **T-64** DeepSeek: V4-Pro `thinking: {type, budget_tokens}` round-trip. H-49.
- **T-65** DeepSeek: interleaved `reasoning → content → reasoning` ordering preserved. H-50.
- **T-66** Groq: `vision: false` flag updated for Llama-4-Scout. C-18.
- **T-67** Mistral: `prompt_cache_retention` NOT emitted on Mistral preset. H-56.
- **T-68** Mistral: tool-call id matches `^[a-zA-Z0-9]{9}$` on Mistral replay. M-75.
- **T-69** OpenRouter: post-finish_reason usage chunk captured (covers C-10 / OR-12). S.
- **T-70** PortKey: `x-portkey-api-key` header path. H-59.
- **T-71** Vercel: dot-form model id default + body. C-19.
- **T-72** Vertex: `global` location URL = bare `aiplatform.googleapis.com`. H-64.
- **T-73** Vertex: `extra_body.google.thinking_config.thinking_budget` round-trip. H-65.
- **T-74** vLLM: no `bearer` injection when key empty. X-17.

### 10.7 PR sequencing addenda

Insert between PR-2 (catalog refresh) and PR-3:

- **PR 2b — per-preset catalog + defaults**: C-15, C-16, C-19, H-58, H-63, K-06 entries. Tiny but eliminates "default config 4xx-s out of the box" failures on Azure / Cerebras / DeepSeek / Vercel / Mistral.

Insert after PR-8:

- **PR 8b — Custom hardening**: H-44, H-45, H-46, M-63, M-64, M-65, T-61..T-63. Security posture for hosted/multi-tenant scenarios.
- **PR 8c — CF AI Gateway migration**: C-12 (default `/compat` → REST), H-40 (`cf-aig-gateway-id` header), H-41 (typed knobs for cache/cost/metadata/observability), M-59, M-60.
- **PR 8d — Mistral / Vertex reasoning body shape**: H-49 (DeepSeek), H-55 (Mistral `reasoning` 422), H-56 (Mistral `prompt_cache_retention` 422), H-65 (Vertex `extra_body.google.thinking_config`). Closes the per-preset "we emit fields the vendor 422s" class.

### 10.8 Updated severity tally

| Severity | Count after §10 | Delta |
|----------|----------------:|------:|
| Critical | 25 | +7 (C-13..C-19) |
| High     | 80 | +33 (H-34..H-66) |
| Medium   | 105 | +35 (M-53..M-87) |
| Low      | 39 | 0 |
| Nit      | 31 | 0 |

**Updated total: ~280 actionable tickets, ~15 PRs.**

The 18 per-preset reports under `.audit/providers/preset-*.md` carry the full source-line citations and reference impls for each finding.
