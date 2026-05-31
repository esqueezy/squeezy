# Fireworks AI Preset Audit

## Summary

- Severity tally: **0 critical / 3 high / 5 medium / 3 low / 2 nit** = **13 findings** (preset-specific; shared-core findings inherited from `.audit/providers/openai-compatible.md` are referenced by ID).
- Top 3 actionable recommendations:
  1. **Refresh the default model** at `crates/squeezy-core/src/lib.rs:103`. `accounts/fireworks/models/llama-v3p3-70b-instruct` is still listed on `fireworks.ai/models` (verified June 2026) but is no longer Fireworks' flagship. Peer agents have moved on: pi defaults to `accounts/fireworks/models/kimi-k2p6` (`others/pi/packages/coding-agent/src/core/model-resolver.ts:36`); Fireworks' "Best Open Source LLMs 2026" roundup promotes DeepSeek-V4-Pro, Kimi K2.6, GLM-5.1, gpt-oss-120B, Qwen3.6-Plus. Pick `accounts/fireworks/models/deepseek-v4-flash` (cheapest reasoning-capable, 1M ctx, $0.14/$0.28 per M) or `kimi-k2p6` (262k ctx, vision) — see FW-1.
  2. **Surface Fireworks' tunables in `LlmRequest`**. `request_body` (`crates/squeezy-llm/src/compatible.rs:134-297`) emits only `model`, `messages`, `stream`, `stream_options`, `max_tokens`, `reasoning_effort`/`reasoning`, `prompt_cache_key`/`prompt_cache_retention`, `tools`, `tool_choice`. Fireworks docs add `prompt_truncate_len`, `top_k`, `min_p`, `typical_p`, `repetition_penalty`, `mirostat_target`, `mirostat_lr`, `reasoning_history`, `thinking.budget_tokens`, plus the OpenAI-standard `seed`/`stop`/`top_p`/`temperature` — none reachable. Cross-cutting **H-26**; the Fireworks-specific overlay is FW-2.
  3. **Add a registry entry per Fireworks model** (FW-3). `grep -c 'provider: "fireworks"' crates/squeezy-llm/src/models.json` returns 0. Result: cost reporting drops to `None`, context-window estimation uses the generic 8k baseline, vision capability defaults to `false` (blocking `kimi-k2p6`/`kimi-k2p5`/`qwen3p6-plus`/`kimi-k2p6-turbo` from images), and reasoning gate stays decorative. Pi already curates 13 Fireworks rows at `others/pi/packages/ai/src/models.generated.ts:3527-3742` — bulk-import.

## Verified

- Base URL: `https://api.fireworks.ai/inference/v1` (`crates/squeezy-core/src/lib.rs:102`) — ✓ (matches https://docs.fireworks.ai/api-reference/post-chatcompletions and opencode profile `others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:11`).
- Auth: `Authorization: Bearer <key>` (`crates/squeezy-llm/src/compatible.rs:474`) — ✓.
- Env var: `FIREWORKS_API_KEY` (`crates/squeezy-core/src/lib.rs:2112`) — ✓.
- Default model: `accounts/fireworks/models/llama-v3p3-70b-instruct` (`crates/squeezy-core/src/lib.rs:103`) — shape ✓; flagship stale (FW-1).
- 401-ping: **none.** No `verify_credentials` / `auth_ping` / `GET /models` path (verified via grep). First real chat call is the credential check; bad keys surface as `Fireworks AI 401: <body>` at `compatible.rs:525-528`. opencode probes `GET /v1/models` (`others/opencode/packages/llm/script/setup-recording-env.ts:212`); squeezy does not.
- `reasoning_content` parsing: shared core at `compatible.rs:1053-1054` correctly buffers + emits `ReasoningDelta { kind: Summary }` for Fireworks' `delta.reasoning_content` (gpt-oss-120b/20b, GLM 4.5/4.5-Air). The spurious empty-content notice on legitimate reasoning-only completions is cross-preset **DS-1 / H-31** (also bites Fireworks — FW-8).

## Implementation Overview

Squeezy's Fireworks preset is a thin metadata pin on `OpenAiCompatibleProvider`: default base URL, default model, default env-var, CLI/TOML aliases, telemetry tag (`crates/squeezy-core/src/lib.rs:102-103,1975,2007,2032,2076,2112,2143,2172,2203,8747`; `crates/squeezy-telemetry/src/lib.rs:1023,1068`; `crates/squeezy-llm/src/registry.rs:229`; `crates/squeezy-cli/src/auth.rs:105-110`). No preset-specific branches exist in `compatible.rs`: no entry in `preset_default_headers` (`compatible.rs:762-775`), no body-field branch in `request_body`, and `accounts/fireworks/models/...` matches no prefix in `COMPAT_TABLE` (`compatible.rs:374-403`), so `compat_entry` returns `None`, `supports_anthropic_caching` is `false`. Model id travels verbatim end-to-end.

Fireworks ships **three** API surfaces on the same host: (1) `/inference/v1/chat/completions` (squeezy's path); (2) `/inference/v1/responses` (OpenAI Responses-compatible with first-class MCP, GA June 2025); (3) `/v1/messages` at bare `https://api.fireworks.ai/inference` (Anthropic-compatible). Pi exposes Fireworks exclusively via (3) — all 13 entries are `api: "anthropic-messages"`. Squeezy reaches only (1).

`parse_chat_usage` (`compatible.rs:1138-1164`) reads `prompt_tokens`, `completion_tokens`, `prompt_tokens_details.cached_tokens` (Fireworks does emit on cached prefixes), and `completion_tokens_details.reasoning_tokens`. The Fireworks-specific `fireworks-prompt-tokens` / `fireworks-server-time-to-first-token` headers and the `perf_metrics_in_response: true` body knob are not consumed.

## Findings

### FW-1 (high) — Default model is no-longer-flagship

- **Location**: `crates/squeezy-core/src/lib.rs:103` (`DEFAULT_FIREWORKS_MODEL`); propagated to `crates/squeezy-skills/external-docs/PROVIDERS.md:326`.
- **Observed**: `accounts/fireworks/models/llama-v3p3-70b-instruct`. Still listed on `fireworks.ai/models` (June 2026), but absent from Fireworks' own "Best Open Source LLMs 2026" roundup. Pi defaults to `kimi-k2p6` (`others/pi/packages/coding-agent/src/core/model-resolver.ts:36`).
- **Issue**: Bare-default `[providers.fireworks]` configs route to a $0.90/M-token non-reasoning Llama 3.3 while Fireworks' price-leadership is now DeepSeek-V4-Flash ($0.14/$0.28) and gpt-oss-20b ($0.07/$0.30). Coding-agent flows relying on `reasoning_effort` silently never engage it — Llama 3.3 70B is not a reasoning SKU.
- **Impact**: Medium cost; high capability (default is non-reasoning).
- **Fix sketch**: Switch to `accounts/fireworks/models/deepseek-v4-flash` (reasoning, 1M ctx). Alternatives: `kimi-k2p6` (262k ctx, matches pi), `gpt-oss-120b`. Update `PROVIDERS.md:326`.
- **Reference**: https://fireworks.ai/blog/best-open-source-llms; `.audit/providers/openai-compatible.md` FW-1.

### FW-2 (high) — Fireworks-specific body fields unreachable

- **Location**: `crates/squeezy-llm/src/compatible.rs:134-297` (`request_body`); `crates/squeezy-llm/src/lib.rs:130-176` (`LlmRequest`).
- **Observed**: Fireworks' chat-completions docs add `prompt_truncate_len`, `top_k`, `min_p`, `typical_p`, `repetition_penalty`, `mirostat_target`, `mirostat_lr`, `reasoning_history` (`"disabled"`/`"interleaved"`/`"preserved"`), `thinking: { type, budget_tokens }`, plus OpenAI-standard `seed`/`stop`/`top_p`/`temperature`. None reachable.
- **Issue**: (i) `prompt_truncate_len` — Fireworks' context-fit + cost-control knob. (ii) `reasoning_history: "preserved"` — multi-turn reasoning replay; without it every turn re-burns reasoning tokens. (iii) `thinking.budget_tokens` — per-call thinking budget Fireworks accepts on reasoning SKUs; `reasoning_effort` is a coarser proxy and ignored on some models. Worst case: reasoning models burn the entire output budget on thinking.
- **Fix sketch**: Cross-cutting **H-26** covers the OpenAI-standard subset. Fireworks overlay: add `LlmRequest::prompt_truncate_tokens: Option<u32>`; gate `reasoning_history` + `thinking.budget_tokens` on a new `COMPAT_TABLE` row keyed by `accounts/fireworks/models/` / `accounts/fireworks/routers/`.
- **Reference**: https://docs.fireworks.ai/api-reference/post-chatcompletions; `.audit/providers/openai-compatible.md` FW-2.

### FW-3 (high) — Zero registry coverage in `models.json`

- **Location**: `crates/squeezy-llm/src/models.json` (`grep -c 'provider: "fireworks"'` = 0); `crates/squeezy-llm/src/registry.rs:229` lists `"fireworks"` with no entries.
- **Observed**: `model_info_for("fireworks", "...")` falls through to `fallback_model_info` (`registry.rs:249-253`), leaking a per-call `ModelInfo` with a generic 8k context-window estimate and no cost data.
- **Issue**: (i) Cost reporting drops to `None` — `parse_chat_usage` extracts tokens but `estimated_usd_micros` stays `None`. (ii) Context-window estimation uses the 8k baseline, masking that DeepSeek-V4-Flash is 1M and Kimi-K2.6 is 262k. (iii) Vision defaults `false`, so `ensure_vision_support` (`compatible.rs:445-447`) blocks `kimi-k2p6`/`kimi-k2p5`/`qwen3p6-plus`/`kimi-k2p6-turbo` from images despite docs confirming support. (iv) Reasoning gate stays `false` (**X-09**) so `reasoning_effort` is decorative.
- **Fix sketch**: Bulk-import the 13 rows pi curates at `others/pi/packages/ai/src/models.generated.ts:3527-3742` with `cost.input`/`cost.output`/`cost.cacheRead` (Fireworks does not bill cache-write), `contextWindow`, `maxTokens`, `reasoning: true`, `input: ["text"]` or `["text", "image"]`. Flip `reasoning_effort: true` only on documented reasoning SKUs (GLM 4.5/4.5-Air, gpt-oss-120B/20B, DeepSeek-V4 family).
- **Reference**: pi catalog above; `.audit/providers/openai-compatible.md` FW-3; cross-cutting **X-08**, **X-09**.

### FW-4 (medium) — `tool_choice` cannot pin a specific function

- **Location**: `crates/squeezy-llm/src/compatible.rs:283-294`; `crates/squeezy-llm/src/lib.rs:159`.
- **Observed**: Fireworks accepts `"auto"`/`"none"`/`"any"`/`"required"`/`{ "type": "function", "name": "..." }`. Squeezy only handles strings.
- **Issue**: Pinning a specific tool by name is impossible. Fireworks accepts both `"required"` and `"any"` so the cross-vendor normalization gap is benign for this preset, but function-pin is unreachable.
- **Fix sketch**: Promote `tool_choice` to enum `LlmToolChoice { Auto, None, Required, Function(String) }`. Covered partially by **H-29**.

### FW-5 (medium) — Fireworks cache-affinity headers not emitted; `prompt_cache_key` is dead weight

- **Location**: `crates/squeezy-llm/src/compatible.rs:225-237`.
- **Observed**: `prompt_cache_key` is forwarded after clamp. Fireworks docs do not document this body field on shared `/chat/completions` — accepted-but-ignored. The real cache-affinity mechanism is `session_id` / `x-session-affinity` / `x-client-request-id` request headers (per pi's `others/pi/packages/ai/src/providers/openai-completions.ts:476-478`).
- **Issue**: Cache hits stay 0 across many turns of the same prefix. Pi's CHANGELOG notes 20-50% input-cost savings once the headers ship (`others/pi/packages/ai/CHANGELOG.md:301`). Also cross-cutting **H-33** (truncate-vs-hash).
- **Fix sketch**: Per-preset header injector: when `preset == Fireworks` and `cache_spec.key.is_some()`, set `session_id`/`x-session-affinity`/`x-client-request-id` to the (hashed) cache key.

### FW-6 (medium) — Anthropic-compat (`/v1/messages`) and Responses (`/v1/responses`) surfaces unreachable

- **Location**: Architectural — `crates/squeezy-core/src/lib.rs:1965-1991` has one Fireworks entry; `crates/squeezy-llm/src/registry.rs:370-376` routes only through `OpenAiCompatibleProvider`.
- **Observed**: Fireworks publishes (a) an Anthropic-Messages endpoint at `https://api.fireworks.ai/inference/v1/messages` and (b) an OpenAI-Responses endpoint at `/inference/v1/responses` with first-class MCP. Pi consumes Fireworks exclusively via (a).
- **Issue**: Users wanting the Anthropic shape for better tool-calling on Kimi/DeepSeek (pi's empirical reason) must drop to `Custom`, which only routes chat-completions — escape hatch fails. MCP-via-Fireworks (server-side hosted tools) is unreachable. Cleaner reasoning surfacing via Responses is unreachable.
- **Fix sketch**: Two presets. `FireworksAnthropic`: build `AnthropicProvider` against `https://api.fireworks.ai/inference` (no `/v1` on host — Fireworks docs explicit) with `FIREWORKS_API_KEY` bearer; mirror pi's `sendSessionAffinityHeaders = true`; document unsupported tool-schema fields. `FireworksResponses` (or `OpenAiCompatibleConfig.use_responses_endpoint: bool`): mirror the xAI dispatcher at `crates/squeezy-llm/src/xai.rs:30-78`. Smaller alternative: at least update `PROVIDERS.md`.
- **Reference**: https://docs.fireworks.ai/tools-sdks/anthropic-compatibility; https://fireworks.ai/blog/response-api.

### FW-7 (medium) — `parse_chat_usage` ignores Fireworks header metrics

- **Location**: `crates/squeezy-llm/src/compatible.rs:1138-1164`; no header read in `crates/` (verified via grep).
- **Observed**: Fireworks emits `fireworks-prompt-tokens` and `fireworks-server-time-to-first-token` headers, and accepts `perf_metrics_in_response: true`. Squeezy reads only the body `usage`.
- **Issue**: Dedicated deployments configured with body-`usage` off report 0 tokens. TTFT telemetry observable from headers but unused.
- **Fix sketch**: Fall back to `fireworks-prompt-tokens` when `usage.prompt_tokens` is absent.

### FW-8 (medium) — Reasoning-only-stop notice fires on legitimate Fireworks reasoning completions

- **Location**: `crates/squeezy-llm/src/compatible.rs:1094-1109`.
- **Observed**: Fireworks gpt-oss / GLM 4.5 / DeepSeek-V4 with `reasoning_effort = "high"` may finish with reasoning-only output. The injected notice's suggested fix is `tool_choice = "required"`.
- **Issue**: Same root cause as cross-preset **DS-1 / H-31** (DeepSeek). Fix lives in shared code (suppress when `reasoning_buf` non-empty); listed here for visibility.

### FW-9 (low) — `ReasoningEffort` cannot reach Fireworks' `"none"` or `"max"`

- **Location**: `crates/squeezy-core/src/lib.rs:2446-2466`.
- **Observed**: Squeezy: `Low|Medium|High|XHigh`. Fireworks: `"none"`/`"low"`/`"medium"`/`"high"`/`"xhigh"`/`"max"` plus boolean shorthand.
- **Issue**: Cannot disable reasoning per-request (`"none"`) without omitting the field, which means provider-default reasoning still fires. Cannot reach `"max"`.
- **Fix sketch**: Extend enum with `None_` and `Max`.

### FW-10 (low) — `stream_options.include_usage: true` always emitted

- **Location**: `crates/squeezy-llm/src/compatible.rs:210`.
- **Observed**: Always-on. Fireworks shared-serverless accepts; older dedicated-deployment serving images may reject (cross-cutting **CB-2**/**LC-1**).
- **Fix sketch**: Per-preset model-id gate (covered cross-cuttingly).

### FW-11 (low) — Bare-slug model id 400s with no hint

- **Location**: `crates/squeezy-llm/src/compatible.rs:483-528`.
- **Observed**: User typing `model = "kimi-k2p6"` (bare slug) gets a 400 with no nudge toward `accounts/{account}/models/{slug}` / `accounts/{account}/routers/{slug}`. PortKey has a parallel hint at `compatible.rs:505-524`.
- **Fix sketch**: When `preset == Fireworks` and the 400 body mentions "model not found", append a hint linking `https://fireworks.ai/models`.

### FW-12 (nit) — Stale module-doc comment

- **Location**: `crates/squeezy-llm/src/compatible.rs:5`. Comment lists xAI as routed through this provider; Grok 3+ goes through `XaiProvider` (cross-cutting **N2**). Optionally also note Fireworks' three surfaces.

### FW-13 (nit) — `PROVIDERS.md` Fireworks docstring is stale

- **Location**: `crates/squeezy-skills/external-docs/PROVIDERS.md:319-328`. "Llama, Mixtral, and DeepSeek with function-calling fine-tunes" is stale — Mixtral is gone; current stars are Kimi, DeepSeek-V4, GLM-5.1, gpt-oss, MiniMax, Qwen3.6. Bump default-model example to match FW-1.

## Test Coverage Gaps

No `fireworks_costly.rs` or mock test in `crates/squeezy-llm/tests/` (verified). Fireworks lives in the universal compat-test gap (cross-cutting **T-53**) plus preset-specific surface:

- **F-T1**: Mock SSE with `delta.reasoning_content` chunks → assert `ReasoningDelta { kind: Summary }`.
- **F-T2**: Mock chat-completions with `usage.prompt_tokens_details.cached_tokens` populated → assert `CostSnapshot.cached_input_tokens` round-trip (catches FW-3 once registry rows ship).
- **F-T3**: `tool_choice = "any"` and `tool_choice = "required"` both forwarded verbatim with no mangling.
- **F-T4**: 401 against a mock returning Fireworks' `HTTPValidationError` shape → assert error message says `Fireworks AI 401:` and surfaces the `detail` array (probes the **H6** `format_chat_error` gap on Fireworks' envelope).
- **F-T5**: Stream that mixes `delta.content` and `delta.reasoning_content` chunks → assert no notice on `finish_reason: stop` (covers FW-8 once **H-31** lands).

Endpoint snapshot (cross-cutting **T-55**) should include `("fireworks", "https://api.fireworks.ai/inference/v1", "<refreshed-default>")`.

## Verification Strategy

For preset-isolation tests (no key required), a mock server that:
1. Accepts `POST /chat/completions` and verifies `Authorization: Bearer test-fireworks-key`.
2. Asserts request body contains the expected `model`, `messages`, `stream: true`, `stream_options.include_usage: true`, and (when applicable) `reasoning_effort` + `reasoning.effort`.
3. Streams the canned SSE scripts behind F-T1..F-T5.
4. Returns a `data: [DONE]` after a usage-only chunk to exercise the cross-cutting **C-10** gap on the Fireworks path.

For optional `fireworks_costly.rs` (gated on `SQUEEZY_RUN_COSTLY_TESTS=1`): one turn against `accounts/fireworks/models/gpt-oss-20b` (cheapest reasoning SKU) with `reasoning_effort = "low"`; assert `ReasoningDelta` fires and `usage.cached_input_tokens` is `Some(0)` first turn, `> 0` on a session-affinity-attached second turn (validates FW-5 once the headers ship).

## References

- Fireworks chat completions API reference (June 2026): https://docs.fireworks.ai/api-reference/post-chatcompletions
- Fireworks Anthropic compatibility: https://docs.fireworks.ai/tools-sdks/anthropic-compatibility
- Fireworks Response API + MCP launch: https://fireworks.ai/blog/response-api
- Fireworks vision models: https://docs.fireworks.ai/guides/querying-vision-language-models
- Fireworks serverless rate limits: https://docs.fireworks.ai/serverless/rate-limits
- Fireworks pricing: https://fireworks.ai/pricing
- Fireworks "Best Open Source LLMs 2026": https://fireworks.ai/blog/best-open-source-llms
- opencode Fireworks profile: `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:11`
- pi Fireworks catalog (Anthropic-Messages flavored): `/Users/abbassabra/esqueezy/others/pi/packages/ai/src/models.generated.ts:3527-3742`
- pi Fireworks session-affinity headers contract: `/Users/abbassabra/esqueezy/others/pi/packages/ai/src/providers/openai-completions.ts:476-478`
- Cross-cutting shared-core findings inherited from `.audit/providers/openai-compatible.md`: **H-26** (missing seed/top_p/temperature/stop), **H-29** (tool_choice shape), **H-31** (reasoning-only-stop noise), **H-33** (prompt_cache_key hash-not-truncate), **C-10** (usage after finish_reason), **X-08**/**X-09** (registry refresh + reasoning gates).
