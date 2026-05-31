# Vercel AI Gateway Preset Audit

## Summary

- Severity tally: **1 critical / 4 high / 5 medium / 3 low / 2 nit** = **15 findings**.
- Top 3 actionable recommendations:
  1. **Fix the default model ids** — squeezy hardcodes `anthropic/claude-opus-4-7` (dash) at `crates/squeezy-core/src/lib.rs:82` and `anthropic/claude-haiku-4-5` at `:56`, but Vercel's canonical ids use dots: `anthropic/claude-opus-4.7`, `anthropic/claude-haiku-4.5`. Every fresh user with no model override 400s on the first request.
  2. **Wire `VERCEL_OIDC_TOKEN` as auth fallback** (VL-2). Vercel deployments inject this 12-hour rotating token automatically; squeezy only honors `AI_GATEWAY_API_KEY`. One-line resolver + a refreshing `ApiKeySource`.
  3. **Expose `providerOptions`** (VL-3). Vercel's whole value-add (gateway.order, gateway.sort, gateway.caching: "auto", BYOK, model-fallback `models: [...]`, per-vendor knobs) is unreachable from `LlmRequest`. Users pay Vercel's markup for a vanilla OpenAI proxy.

## Implementation Overview

Vercel routes through the shared `OpenAiCompatibleProvider`. Preset metadata lives at `crates/squeezy-core/src/lib.rs:1967` (`Vercel` variant), `:2024` (display "Vercel AI Gateway"), `:2064` (`DEFAULT_VERCEL_AI_BASE_URL`), `:2100` (env var `AI_GATEWAY_API_KEY`), `:2135` (default model `anthropic/claude-opus-4-7`), `:2164` (CLI/TOML aliases `vercel | vercel_ai | vercel_ai_gateway`), `:8739` (TOML section `[vercel]`). Constants at `:56` (`VERCEL_SMALL_FAST_MODEL`), `:81` (`DEFAULT_VERCEL_AI_BASE_URL = "https://ai-gateway.vercel.sh/v1"`), `:82` (default model). Per-turn cheap-router wiring at `:72`.

Three curated registry entries at `crates/squeezy-llm/src/models.json:518-592` cover `anthropic/claude-opus-4-7`, `openai/gpt-5.5`, `google/gemini-2.5-pro` (all `pricing: null`, all `prompt_caching: false`).

Request lifecycle in `crates/squeezy-llm/src/compatible.rs`:
- `from_config` (`:78-90`) builds a single provider with `Authorization: Bearer <AI_GATEWAY_API_KEY>` and empty `extra_headers` (Vercel has no preset defaults — verified at `compatible_tests.rs:548-549`).
- `request_body` (`:134-297`) emits standard chat-completions shape with `stream: true`, `stream_options.include_usage: true`, Anthropic-style `cache_control` markers when `model.starts_with("anthropic/")` (driven by `COMPAT_TABLE` at `:374-381`), and `reasoning_effort` + `reasoning: { effort }` unconditionally.
- `stream_response` (`:444-614`) POSTs to `{base_url}/chat/completions` and decodes SSE through the shared `SseDecoder`.

Vercel-specific code is exactly zero: the `anthropic/` branch in `COMPAT_TABLE` is shared with OpenRouter and PortKey. No `providerOptions` builder, no `provider` shorthand emitter, no model-fallback `models` field, no OIDC resolver, no extra-header default.

Costly test at `crates/squeezy-llm/tests/vercel_costly.rs:1-44` runs one preset + one model (`anthropic/claude-haiku-4-5`, also wrong dash form, line 14). No mock coverage.

## Per-Preset Findings

### VL-A — Default model id dash form is rejected by Vercel (critical)

Verified May 2026 against Vercel's `GET https://ai-gateway.vercel.sh/v1/models` and `/docs/ai-gateway/sdks-and-apis/openai-chat-completions` ("Use the model string `anthropic/claude-opus-4.7`"). All Anthropic ids on Vercel are dotted: `claude-opus-4.7`, `claude-haiku-4.5`, `claude-sonnet-4.6`, etc. Squeezy emits dash form at `crates/squeezy-core/src/lib.rs:82` (`anthropic/claude-opus-4-7`) and `:56` (`anthropic/claude-haiku-4-5`). Costly test at `tests/vercel_costly.rs:14` mirrors the bug.

A first-time user with no model override fires `model: "anthropic/claude-opus-4-7"` and gets `400 model not found`. The error is surfaced as `Vercel AI Gateway 400: <message>` via `format_chat_error` (`compatible.rs:976-998`) with no hint about the `-` vs `.` mismatch. The same bug is mirrored in OpenRouter (`:80,:55`) and PortKey (`:84,:57`) but OpenRouter accepts the dash form via its internal alias map — Vercel does NOT.

`models.json:520, 545, 570` (the three registry entries) also use dash form, so `model_info_for` returns a phantom record. The cost projection layer trusts the phantom's `context_window_tokens: 200000` / `max_output_tokens: 64000` for budgeting decisions on a model that doesn't exist on Vercel.

**Fix**: change all four constants to dot form, update `models.json`, update the costly test, and add a CI lint that diffs `models.json` Vercel entries against the unauthenticated `GET /v1/models` snapshot.

### VL-B — No `provider/model` prefix validation; bare model id returns 400 with no hint (high)

Tracked as VL-1 in the shared audit. Vercel rejects model ids without the `creator/` prefix with `400 Bad Request` (verified May 2026: "Models and providers follow the format `creator/model-name`"). Squeezy's `request_body` (`compatible.rs:134-297`) emits `request.model` verbatim. A user TOML

```toml
[model]
provider = "vercel"
model = "claude-opus-4.7"   # forgot the prefix
```

posts `{"model": "claude-opus-4.7"}` and gets a generic 400 with no preset-specific hint.

**Fix**: reject model ids without `/` for the Vercel preset in `build_openai_compatible_config` (`lib.rs:8602-8726`) or in `OpenAiCompatibleProvider::stream_response`. Hint: `"Vercel requires a provider/model id (e.g. anthropic/claude-opus-4.7, openai/gpt-5.5). Call GET https://ai-gateway.vercel.sh/v1/models for the list."`

### VL-C — `VERCEL_OIDC_TOKEN` not honored as auth fallback (high)

Tracked as VL-2 in the shared audit. Vercel deployments auto-inject `VERCEL_OIDC_TOKEN` (12h TTL, verified at `/docs/ai-gateway/authentication-and-byok/oidc`: "Vercel OIDC tokens are only valid for 12 hours … `vercel env pull` to refresh"). Vercel's precedence is `AI_GATEWAY_API_KEY > VERCEL_OIDC_TOKEN`. Squeezy's resolver at `:2100` only knows about `AI_GATEWAY_API_KEY`.

Squeezy running inside a Vercel function 401s on the first request even though `VERCEL_OIDC_TOKEN` is in the env. Worse: `from_config` snapshots the env once via `static_api_key_source`; mid-session OIDC rotation invalidates the cached value with no refresh path (analogous to the Vertex VX-1 bug in the shared audit).

**Fix**:
1. In `build_openai_compatible_config` for the `Vercel` arm, fall back to `VERCEL_OIDC_TOKEN` when `AI_GATEWAY_API_KEY` is unset.
2. Use a refreshing `ApiKeySource` for OIDC-sourced keys (mirror the OAuth refresh pattern used elsewhere).

### VL-D — `providerOptions` and `provider` shorthand not exposed (high)

Tracked as VL-3. Vercel's `/chat/completions` accepts:
- `providerOptions.gateway.order: ["vertex","anthropic"]`, `gateway.only: [...]`, `gateway.sort: "cost"|"ttft"|"tps"`
- `providerOptions.gateway.caching: "auto"` — auto-injects `cache_control` for Anthropic/MiniMax
- `providerOptions.gateway.byok: { anthropic: [{apiKey}], vertex: [...], bedrock: [...] }` — per-request BYOK
- `providerOptions.gateway.providerTimeouts: { byok: { anthropic: 3000 } }`
- `providerOptions.gateway.models: [...]` and top-level `models: [...]` for model fallback
- `providerOptions.anthropic.thinkingBudget: 0.001` — upstream Anthropic knobs
- `providerOptions.openai.reasoningEffort` + `reasoningSummary` — gateway *requires both* for `openai/gpt-5.5` reasoning output (verified May 2026)
- `provider: { sort: "tps" }` — shorthand for `providerOptions.gateway.sort`

`LlmRequest` (`crates/squeezy-llm/src/lib.rs:130-176`) has no slot for any of this; `request_body` never emits these fields. Users routing through Vercel are paying Vercel's markup for a vanilla OpenAI proxy — none of the failover/BYOK/cost-routing value-add is reachable.

**Fix**: add `provider_options: Option<serde_json::Value>` to `LlmRequest` and forward verbatim from the chat-completions adapter. Free-form JSON lets squeezy track Vercel's schema churn without per-knob plumbing.

### VL-E — Squeezy's manual `cache_control` markers collide with Vercel's `caching: "auto"` (medium)

`compatible.rs:151-153` decides `anthropic_caching` from `compat_entry(model).supports_cache_control` for `anthropic/*`. Vercel passes manually-marked `cache_control` through verbatim when `providerOptions.gateway.caching` is unset — current squeezy behavior is correct. But: once VL-D is fixed, a user setting `caching: "auto"` AND running on an Anthropic model would get BOTH squeezy's manual markers AND Vercel's auto-inserted ones. Anthropic enforces a 4-breakpoint limit; duplicates can 400. Marker placement may also differ (squeezy: system tail + last user + last stable tool; Vercel: "end of your static content").

**Fix**: when `providerOptions.gateway.caching = "auto"` is set, suppress squeezy's manual marker emission.

### VL-F — `reasoning_effort` shape collides with Vercel's `providerOptions.{openai,anthropic}` routing (medium)

`compatible.rs:215-224` always emits `reasoning_effort: "high"` + `reasoning: { effort: "high" }`. Vercel May 2026:
- `reasoning: { enabled, max_tokens, effort, exclude }` is a Vercel top-level shape; `effort` accepts `"none"|"minimal"|"low"|"medium"|"high"|"xhigh"`. Squeezy's enum likely lacks `xhigh`.
- For `openai/gpt-5.5`, Vercel requires BOTH `providerOptions.openai.reasoningEffort` AND `providerOptions.openai.reasoningSummary` to surface reasoning output. The top-level `reasoning_effort` is silently dropped on this path.
- For `anthropic/*` via Vercel, the correct knob is `providerOptions.anthropic.thinkingBudget` — the top-level `reasoning_effort` is dropped silently.

**Fix**: once VL-D lands, route `reasoning_effort` through `providerOptions.openai.reasoningEffort` + `reasoningSummary: "auto"` for OpenAI namespaces; through `providerOptions.anthropic.thinkingBudget` for Anthropic. Keep the top-level shape only as a fallback for the `Generic` flavor.

### VL-G — `models.json` Vercel entries claim `prompt_caching: false` (medium)

`models.json:531` (anthropic), `:556` (openai), `:581` (google) all set `prompt_caching: false`. Verified May 2026:
- `anthropic/claude-opus-4.7` via Vercel honors `cache_control` markers (squeezy DOES emit them via `COMPAT_TABLE:376-381`, so caching is active on the wire).
- `openai/gpt-5.5` via Vercel uses implicit caching ("OpenAI: Implicit — caching happens automatically").
- `google/gemini-*` via Vercel uses implicit caching.

All three should be `true`. Downstream cost projection / UI consumers under-report cache benefit.

**Fix**: flip the three capability bits.

### VL-H — `google/gemini-2.5-pro` Vercel entry is stale (medium)

`models.json:570` lists `google/gemini-2.5-pro`. Vercel's preferred Google entry May 2026 is `google/gemini-3.1-pro-preview` with `google/gemini-2.5-flash` as the budget tier. No CI lint catches drift.

**Fix**: update to `google/gemini-3.1-pro-preview`. Same CI snapshot lint as VL-A.

### VL-I — Anthropic-via-Vercel `reasoning_details` shape not decoded (medium)

Vercel normalizes reasoning across providers into a `reasoning_details: [{type, text|summary|data, signature?, format, index}]` shape that survives tool-calling round-trips for prefix-cache hits. Squeezy's `parse_chat_event` (`compatible.rs:1000-1136`) decodes only the raw `reasoning_content` / `reasoning` text fields; it drops `reasoning_details` and its `signature` field. On Anthropic-via-Vercel, the next turn's prefix is missing the signature and Vercel re-charges for thinking tokens already paid for.

**Fix**: extend the parser to round-trip `reasoning_details` (including `signature`, `format`, `index`) into the conversation history so the next turn's replay carries them.

### VL-J — No costly test for prefix enforcement, BYOK, or `providerOptions` (low)

`tests/vercel_costly.rs:18-43` only verifies a vanilla request returns `squeezy-ok`. No assertions on bare-id 400s, sort-routing, BYOK, cache-marker round-trip, or reasoning forwarding. Extend incrementally; higher priority is the mock test for VL-B validation.

### VL-K — `is_full_tier` claims full-tier with only 3 registry entries (low)

`:2048-2059` marks Vercel full-tier; `models.json` ships 3 entries vs ~80 models in Vercel's catalog. Functional but thin.

**Fix**: add at least top-10 (Anthropic Haiku/Sonnet/Opus across 4.5-4.8, OpenAI gpt-5.5/5.4-mini/5.4-nano, Google gemini-3.1-pro-preview/2.5-flash, xAI grok-4.3).

### VL-L — No `extra_headers` default for app attribution (low)

Vercel's dashboard supports attribution via `http-referer` + `x-title`, mirrored by opencode at `others/opencode/packages/core/src/plugin/provider/vercel.ts:13-14`. Squeezy gets zero attribution credit. Optional cosmetic fix in `preset_default_headers`.

### VL-M — `gateway_id` / `account_id` substitution is dead for Vercel (nit)

`compatible.rs:701-745` substitutes Cloudflare placeholders. Vercel's `base_url` has none, so the path is a no-op. The `OpenAiCompatibleConfig` still carries `account_id` / `gateway_id` (`:1950, :1955`) and the config builder populates them as `None` for Vercel. Cleanup-only.

### VL-N — `n > 1` and OpenAI dot-vs-dash convention inherited (nit)

Shared H1 risk (tool-call accumulator merges across choices indices when `n > 1`) applies — Vercel passes `n` to the upstream. Squeezy doesn't emit `n` today; no Vercel-specific exposure. Separately, OpenAI/xAI/Google ids on Vercel all use dot form (`gpt-5.5`, `grok-4.3`, `gemini-3.1-pro-preview`) — the dash bug is Anthropic-only.

## Test Coverage Gaps

| Surface | Costly? | Mock? | Gap |
|---|---|---|---|
| Bare model id (no `/`) returns 400 | ✗ | ✗ | VL-B |
| `VERCEL_OIDC_TOKEN` fallback + 12h refresh | ✗ | ✗ | VL-C |
| `providerOptions.gateway.{sort,order,only}` forwarded | ✗ | ✗ | VL-D |
| `providerOptions.gateway.caching: "auto"` collision | ✗ | ✗ | VL-D + VL-E |
| `providerOptions.openai.{reasoningEffort,reasoningSummary}` for gpt-5.5 | ✗ | ✗ | VL-F |
| `providerOptions.anthropic.thinkingBudget` | ✗ | ✗ | VL-F |
| BYOK `providerOptions.gateway.byok` | ✗ | ✗ | VL-D |
| Model-fallback top-level `models: [...]` | ✗ | ✗ | VL-D |
| `models.json` Vercel parity with `/v1/models` | ✗ | ✗ | VL-A, VL-H |
| `reasoning_details` (signature, format) round-trip | ✗ | ✗ | VL-I |
| Anthropic-via-Vercel `cache_control` round-trip cache hit | ✗ | ✗ | VL-E |

## Verification Strategy

1. **Endpoint snapshot lint** (unauthenticated, free in CI): fetch `https://ai-gateway.vercel.sh/v1/models` and assert every Vercel-tagged entry in `models.json` is present. Catches VL-A, VL-H rotting silently.
2. **Model-id validator unit test**: feed `"opus"`, `"claude-opus-4.7"`, `"anthropic/claude-opus-4.7"`, `"anthropic/"` to a validator; assert the first three forms error or hint. Catches VL-B.
3. **Mock SSE harness** (extend the shared one from `lmstudio_mock.rs`): for Vercel, assert
   - `body.model` contains a `/`,
   - `Authorization: Bearer …` falls back to `VERCEL_OIDC_TOKEN` when only OIDC is set,
   - `providerOptions` from `LlmRequest` is forwarded verbatim,
   - Mock SSE replies with `reasoning_details` shape; parser surfaces `{type, signature, format}` on `LlmEvent::ReasoningDelta` (catches VL-I).
4. **OIDC refresh test**: simulate 401 after warm-up; assert the refreshing `ApiKeySource` re-reads the env on retry (catches the snapshot bug from VL-C).

## References

- Vercel AI Gateway OpenAI Chat Completions: https://vercel.com/docs/ai-gateway/sdks-and-apis/openai-chat-completions
- Vercel chat completions reference: https://vercel.com/docs/ai-gateway/sdks-and-apis/openai-chat-completions/chat-completions
- Vercel advanced (reasoning, providerOptions, BYOK, prompt caching): https://vercel.com/docs/ai-gateway/sdks-and-apis/openai-chat-completions/advanced
- Vercel models & providers: https://vercel.com/docs/ai-gateway/models-and-providers
- Vercel provider options (gateway.order/only/sort, byok, caching, model fallbacks): https://vercel.com/docs/ai-gateway/models-and-providers/provider-options
- Vercel automatic caching: https://vercel.com/docs/ai-gateway/models-and-providers/automatic-caching
- Vercel authentication & BYOK: https://vercel.com/docs/ai-gateway/authentication-and-byok
- Vercel OIDC tokens (12h TTL): https://vercel.com/docs/ai-gateway/authentication-and-byok/oidc
- Vercel models endpoint (unauthenticated): https://ai-gateway.vercel.sh/v1/models
- Claude Opus 4.7 on Vercel AI Gateway (model id: `anthropic/claude-opus-4.7`): https://vercel.com/ai-gateway/models/claude-opus-4.7
- Claude Haiku 4.5 on Vercel AI Gateway (`anthropic/claude-haiku-4.5`): https://vercel.com/ai-gateway/models/claude-haiku-4.5
- opencode `vercel.ts` (peer reference for app-attribution headers): /Users/abbassabra/esqueezy/others/opencode/packages/core/src/plugin/provider/vercel.ts
- PI `env-api-keys.ts` (peer reference for `AI_GATEWAY_API_KEY`): /Users/abbassabra/esqueezy/others/pi/packages/ai/src/env-api-keys.ts
- Shared OpenAI-compatible audit (VL-1/2/3 + inherited shared-core risks): /Users/abbassabra/esqueezy/squeezy/.audit/providers/openai-compatible.md
