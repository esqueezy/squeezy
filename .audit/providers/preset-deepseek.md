# DeepSeek Preset Audit

## Summary

- Severity tally: **2 critical / 5 high / 7 medium / 4 low / 2 nit** = **20 findings** (preset-only; shared `OpenAiCompatibleProvider` findings audited separately in `openai-compatible.md`).
- Top 3 actionable recommendations:
  1. **Migrate the catalog to V4** (`deepseek-v4-flash`, `deepseek-v4-pro`) before 2026-07-24 15:59 UTC, when `deepseek-chat`/`deepseek-reasoner` retire as aliases. Default model id (`crates/squeezy-core/src/lib.rs:91`) and both `models.json` entries (`crates/squeezy-llm/src/models.json:773-832`) point at deprecated SKUs with stale pricing — V4-Flash is now $0.14/$0.28 per MTok vs the cached `deepseek-chat` row at $0.27/$1.10. See **DS-2**.
  2. **Plumb the `thinking` body parameter through `LlmRequest`** so V4 thinking-mode (non-think / think-high / think-max) is controllable. Today (`compatible.rs:215-224`) squeezy sends `reasoning_effort` + `reasoning.effort`, neither of which DeepSeek V4 honors — V4 expects `{"thinking": {"type": "enabled", "budget_tokens": N}}` top-level. The toggle is silently lost; V4-Pro users always pay the thinking premium. See **DS-4**.
  3. **Suppress the empty-completion notice on DeepSeek thinking-mode turns** with non-empty reasoning buffers. The text at `compatible.rs:1106-1108` references `tool_choice = "required"` and `reasoning_effort` — neither makes sense for V4 thinking. The agent loop's one-shot re-prompt (`squeezy-agent/src/lib.rs:6031-6090`) then double-bills the user for a legitimate per-turn boundary. See **DS-1**.

## Implementation Overview

The DeepSeek preset is a thin row in `OpenAiCompatiblePreset` (`crates/squeezy-core/src/lib.rs:1971`) mapping to:

- Display name `"DeepSeek"` (`lib.rs:2028`), section/cli `"deepseek"` (`lib.rs:2003`, `auth.rs:82-86`).
- Default base URL `https://api.deepseek.com/v1` (`DEFAULT_DEEPSEEK_BASE_URL`, `lib.rs:90` → `:2068`).
- Default model `deepseek-chat` (`DEFAULT_DEEPSEEK_MODEL`, `lib.rs:91` → `:2139`).
- Env var `DEEPSEEK_API_KEY` (`lib.rs:2104`), CLI auth accepts `SQUEEZY_DEEPSEEK_KEY` override (`crates/squeezy-cli/src/auth.rs:84`).
- Promoted to `is_full_tier` (`lib.rs:2048-2058`).

No DeepSeek-specific code path branches inside `OpenAiCompatibleProvider`; every request flows through the shared `request_body` / `stream_response` machinery at `crates/squeezy-llm/src/compatible.rs:134-297` / `:439-612`. The only DeepSeek-aware logic is two comments (`compatible.rs:792`, `:1083`) acknowledging that `reasoning_content` and the `reasoning_only_stop` failure mode came from R1/V4-thinking traffic.

Curated metadata at `models.json:773-832` (two rows: `deepseek-chat`, `deepseek-reasoner`). One costly integration test at `crates/squeezy-llm/tests/deepseek_costly.rs`. The agent loop's `reasoning_only_stop` retry branch (`squeezy-agent/src/lib.rs:6031-6090`) fires preferentially against DeepSeek-R1.

## Preset-Specific Findings

### DS-1 — Reasoning-mode "stop without content" notice misfires on `deepseek-reasoner` (high)

`compatible.rs:1081-1109` injects this notice whenever `finish_reason="stop"` arrives with `saw_visible_output=false`:

> `[squeezy] model finished without emitting any content or tool call (finish_reason=stop). Reasoning-mode models can burn their output budget on thinking; try a more concrete prompt, lower reasoning_effort, or set [model].tool_choice = "required" to force a tool call.`

Double-wrong for native DeepSeek:

1. DeepSeek's documented thinking mode terminates a turn with `finish_reason="stop"` after streaming `reasoning_content` chunks — contracted shape per the [thinking mode guide](https://api-docs.deepseek.com/guides/thinking_mode). Injecting an apology notice mid-transcript makes a legitimate completion look like an error.
2. The remediation text recommends `reasoning_effort` (not honored by V4, see DS-4) and `tool_choice = "required"` (would force a tool call on every conversational turn).

The agent loop's `reasoning_only_branch` retry at `squeezy-agent/src/lib.rs:6031-6090` then re-issues the turn with a synthetic "Respond directly to the user now" nudge — **double-billing** for what was a legitimate boundary.

**Fix**: gate notice + `reasoning_only_branch` retry on `provider != "deepseek" || !model_is_thinking_mode(model)`.

### DS-2 — Default model `deepseek-chat` retires 2026-07-24 15:59 UTC (critical)

The [DeepSeek pricing page](https://api-docs.deepseek.com/quick_start/pricing) confirms: `deepseek-chat` and `deepseek-reasoner` retire **2026-07-24 15:59 UTC**. Until then, they alias to `deepseek-v4-flash` non-thinking / thinking. After retirement, `model: "deepseek-chat"` will 400.

Affected surfaces:

- `crates/squeezy-core/src/lib.rs:91` — `DEFAULT_DEEPSEEK_MODEL = "deepseek-chat"`.
- `crates/squeezy-llm/src/models.json:775, :805` — both rows use deprecated ids and older pricing (`270000/1100000` micros for chat; `550000/2190000` for reasoner).
- `crates/squeezy-llm/src/lib_tests.rs:602, :673` — assertions hard-code `deepseek-chat`.
- `crates/squeezy-llm/tests/deepseek_costly.rs:14` — `DEFAULT_MODEL = "deepseek-chat"`.
- Fixture at `/Users/abbassabra/esqueezy/others/opencode/packages/llm/test/fixtures/recordings/openai-compatible-chat/deepseek-streams-text.json:24` confirms upstream echoes `"model":"deepseek-v4-flash"` even when request asked for `deepseek-chat`. `ServerModelEcho` (`crates/squeezy-llm/src/lib.rs:617-660`) surfaces this on every DeepSeek call today.

Current V4 pricing per MTok:

| SKU | Input miss | Input hit | Output | Context | Max out |
|---|---|---|---|---|---|
| `deepseek-v4-flash` | $0.14 | $0.0028 | $0.28 | 1M | 384K |
| `deepseek-v4-pro` | $1.74 ($0.435 promo) | $0.0145 ($0.003625 promo) | $3.48 ($0.87 promo) | 1M | 384K |

V4-Pro promo discount ended **2026-05-31 15:59 UTC**.

Squeezy's `deepseek-chat` row (`models.json:788-793`) carries `input_usd_micros_per_mtok: 270000, output_usd_micros_per_mtok: 1100000, cache_read_usd_micros_per_mtok: 27000` — V3-Chat prices, not V4-Flash. **Every DeepSeek call today over-bills by ~2×** in cost telemetry because the upstream silently routes to V4-Flash at $0.14/$0.28. `context_window_tokens: 131072` and `max_output_tokens: 8192` are also stale — V4 has 1M / 384K.

**Fix**: rotate `DEFAULT_DEEPSEEK_MODEL` to `deepseek-v4-flash`, replace both `models.json` rows with V4-Flash and V4-Pro, refresh pricing/context/max-output, add `is_thinking_default` flag for V4-Pro (auto-`max` for agent stacks per the V4 docs), update the costly test. Keep deprecated ids as registry aliases until 2026-07-24.

### DS-3 — `prompt_cache_miss_tokens` is dropped (medium)

`parse_chat_usage` at `compatible.rs:1138-1164` reads `prompt_tokens`, `completion_tokens`, `prompt_tokens_details.cached_tokens` / `prompt_cache_hit_tokens`, and `completion_tokens_details.reasoning_tokens`. DeepSeek's documented `usage` envelope ([chat completion ref](https://api-docs.deepseek.com/api/create-chat-completion)) includes `prompt_cache_miss_tokens` as a peer of hit-tokens; the fixture carries the field (`"prompt_cache_miss_tokens":14`). Today squeezy infers miss as `prompt_tokens - cached_input_tokens`, which differs on V4 thinking-mode turns where `prompt_tokens` is reported after server-side context-cache compaction.

**Fix**: add `cache_miss_input_tokens: Option<u64>` to `CostSnapshot` and surface DeepSeek's explicit miss count.

### DS-4 — `thinking` parameter is not plumbed; `reasoning_effort` is silently ignored on V4 (high)

DeepSeek V4 controls thinking via a top-level `thinking` object ([thinking mode guide](https://api-docs.deepseek.com/guides/thinking_mode), [V4 dev guide](https://framia.converge.ai/page/en-US/news/deepseek-v4-api)):

```json
{"thinking": {"type": "disabled"}}                              // Non-think
{"thinking": {"type": "enabled", "budget_tokens": 8000}}        // Think High (V4-Pro default)
{"thinking": {"type": "max"}}                                   // Think Max
```

Squeezy's `request_body` at `compatible.rs:215-224` emits `reasoning_effort: "high"` + `reasoning: { effort: "high" }` — neither is recognized by V4. User-supplied `reasoning_effort` is dropped at the wire; V4-Pro uses its default (`effort: high` for agent stacks).

Consequences:

1. **V4-Pro users can't disable thinking** to save cost (V4-Pro non-thinking = $0.435/$0.87 promo, vs V4-Flash $0.14/$0.28 always).
2. **V4-Flash users can't enable thinking** — stuck in non-think, materially weaker on competition-class problems (31.7% vs 95.2% on HMMT 2026 Feb per [thinking modes comparison](https://framia.converge.ai/page/en-US/news/deepseek-v4-thinking-modes)).
3. **Think Max requires 384K headroom** — squeezy's max-output clamp at `models.json:797` is 8192, which truncates max-mode mid-thought.

V4 thinking also rejects `temperature`, `top_p`, `presence_penalty`, `frequency_penalty`. Since squeezy doesn't emit those (shared H3), non-issue until H3 is fixed.

**Fix**: add `LlmRequest::thinking: Option<ThinkingSpec>` and forward as `body["thinking"]` when model is a V4 SKU. Map squeezy's `Low/Medium/High` to `disabled / enabled budget=4000 / enabled budget=8000`. Add `DeepSeekFlavor` row in `COMPAT_TABLE` (`compatible.rs:374-403`) with `supports_thinking: true`.

### DS-5 — Interleaved `reasoning_content` followed by `content` loses ordering (high)

`parse_chat_event` at `compatible.rs:1052-1066` processes a delta: reasoning first, then content, then tool calls. Within a single chunk this is correct. But across chunks, the reasoning buffer keeps accumulating **after** the first `content` delta arrives. When V4-Pro streams `reasoning_content` → `content` → `reasoning_content` → `content` (legitimate per protocol, the model can interleave summaries mid-answer), the agent sees all reasoning fragments collected into a single `ReasoningDone` event at end-of-turn (`drain_reasoning` at `compatible.rs:927-937`), in textual concat order but **out of position relative to the content stream**. Transcript renders thinking BEFORE visible content even when stream order placed some thinking AFTER content.

**Fix**: flush a `ReasoningDone` event whenever a `content` delta arrives and `reasoning_buf` is non-empty.

### DS-6 — Vision support: V4 supports image inputs but squeezy hard-codes `vision: false` (medium)

DeepSeek now exposes a `deepseek-vision-preview` beta SKU ([V4 review](https://pixverse.ai/en/blog/deepseek-v4-multimodal-model-coming-to-pixverse)). Squeezy's `models.json` hard-codes `vision: false` for both rows (line 781, 811). The vision-gate at `compatible.rs:445-447` calls `ensure_vision_support("deepseek")`; for unknown ids, registry falls back via `fallback_model_info` with `vision: false`. A user configuring `model = "deepseek-vision-preview"` hits the `does not support image inputs` error at `lib_tests.rs:631`.

**Fix**: add a `deepseek-vision-preview` row with `vision: true`.

### DS-7 — `finish_reason: "insufficient_system_resource"` is not handled (medium)

DeepSeek docs document `insufficient_system_resource` as a `finish_reason` value (request interrupted by concurrency limit). `parse_chat_event` at `compatible.rs:1075-1130` handles `tool_calls`, `function_call`, `stop`, `length`, `content_filter`; everything else falls to `_ => {}` (line 1129), so:

1. No `drain_tool_calls()` — pending tool deltas lost.
2. No visible notice — stream cuts off unexplained.
3. `chat_stop_reason` maps it to `StopReason::Other("insufficient_system_resource")` (`compatible.rs:917-924`); agent has no semantics for it, turn completes silently.

Functionally a server-side 503 mid-stream. Retry policy at `retry.rs:46-55` (`provider_stream`) sets `retry_5xx: false`, so even correct classification wouldn't trigger auto-retry — but at minimum the user should see WHY the turn ended.

**Fix**: extend the `match finish_reason` arms to recognize `insufficient_system_resource`, drain pending tool calls, emit a notice, and surface a `StopReason::ServerOverloaded` for the agent's retry policy.

### DS-8 — Concurrency limit (HTTP 429) headers ignored (medium)

DeepSeek's [rate-limit page](https://api-docs.deepseek.com/quick_start/rate_limit) documents 500 concurrent connections on `deepseek-v4-pro`, 2500 on `deepseek-v4-flash`, 10-min connection timeout if inference hasn't started. No documented `x-ratelimit-*` schema; community sources confirm `Retry-After` on 429s. Squeezy honors `Retry-After` for the initial POST via `retry.rs:148-153`. But the **mid-stream `insufficient_system_resource`** at DS-7 is also a concurrency shape and isn't retried.

**Fix**: combined with DS-7, classify inline `insufficient_system_resource` as retryable.

### DS-9 — `models.json` claims `prompt_caching: false` but DeepSeek caches automatically (medium)

Both DeepSeek rows set `prompt_caching: false` (line 786, 816). DeepSeek operates an **automatic context-prefix cache** on every chat completion — no opt-in. The pricing rows confirm cache_read prices (line 791: $0.027/MTok, line 821: $0.055/MTok). Setting `prompt_caching: false` likely causes the cache-aware estimator (`estimate_request_context` at `registry.rs:260`) and TUI cost panel to undercount savings.

**Fix**: set `prompt_caching: true` for both rows.

### DS-10 — Default `max_output_tokens` cap of 8192 is ~1/47 of Think Max ceiling (medium)

`models.json:796, :826` report `max_output_tokens: 8192`. V4-Flash and V4-Pro both support **384K** max output tokens. Think Max needs 32768+ for visible answer + 50K reasoning. Squeezy's clamp will refuse `[providers.deepseek] max_output_tokens = 200000` because the row caps at 8192.

**Fix**: bump to 384000 when V4 is wired up.

### DS-11 — Server model alias (`deepseek-chat` → `deepseek-v4-flash`) re-trips per session (low)

Fixture confirms DeepSeek echoes `"model":"deepseek-v4-flash"` even when request body sent `"model":"deepseek-chat"`. `ServerModelEcho::observe` (`lib.rs:617-660`) emits the mismatch once **per session**, but new sessions re-trip the same warning. Cosmetic, confusing for users.

**Fix**: when preset is DeepSeek and server id is a known V4 alias of the requested id, suppress the echo or emit `LlmEvent::ServerModelAliasResolved`.

### DS-12 — `json_mode` advertised but `output_schema` is not forwarded as `response_format` (low)

`models.json:780, :810` advertise `json_mode: true`. DeepSeek accepts `response_format: {"type": "json_object"}`. Squeezy's `output_schema` is never emitted by the chat-completions builder (shared M3). For DeepSeek, the looser `json_object` mode is also absent, so a call with `output_schema` set silently returns markdown / free text instead of guaranteed JSON.

**Fix**: when `output_schema.is_some() && provider == "deepseek"`, emit `response_format: {"type": "json_object"}` and inline the schema description into the system prompt (DeepSeek doesn't honor full `json_schema` strict mode).

### DS-13 — `deepseek-reasoner` tool-call streaming format is non-standard (low)

V4-Pro thinking handles tool calls AFTER a `reasoning_content` burst; historically `deepseek-reasoner` (V3-era reasoner) did NOT support tool calls. Transition to tools-in-thinking is recent (Q1 2026). Squeezy's `models.json:809` claims `tool_calling: true` — for V4-thinking correct; for R1-era reasoner likely false. With the model deprecating 2026-07-24 largely moot, but registry overstates capability until then.

**Fix**: costly test confirming `tools: [...]` against `deepseek-reasoner` (now aliased to V4-Flash thinking) succeeds. If R1-era reasoner replies 400, flip the flag.

### DS-14 — No `costly` test exercises `reasoning_content`, thinking mode, or tool-call streaming (medium)

`deepseek_costly.rs:18-43` is a single echo-smoke test. No coverage for:

- Streaming `reasoning_content` deltas (V4-Pro thinking).
- The `thinking` parameter (DS-4) once plumbed.
- `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` parsing (DS-3).
- Post-`finish_reason: "stop"` usage chunk loss (shared C1 — affects DeepSeek per the fixture).
- `insufficient_system_resource` finish reason (DS-7).

**Fix**: extend `deepseek_costly.rs` with one test per shape, gated on `--features costly-tests`.

### DS-15 — `DEEPSEEK_BASE_URL` env recognized by tests but not config (low)

`deepseek_costly.rs:27` reads `DEEPSEEK_BASE_URL` to override base URL during tests. Runtime config builder at `crates/squeezy-core/src/lib.rs:8602-8726` does NOT honor this env — only `DEEPSEEK_API_KEY` is resolved. A user setting `DEEPSEEK_BASE_URL=https://my-proxy/...` expects routing through the proxy; squeezy ignores it and hits `api.deepseek.com`.

**Fix**: document that base-URL overrides go through `providers.deepseek.base_url` in TOML, or add env-var resolution.

### DS-16 — Display name casing vs telemetry casing (nit)

`lib.rs:2028` returns `"DeepSeek"` (CamelCase); `crates/squeezy-telemetry/src/lib.rs:1019` declares a `DeepSeek` variant whose serialized form (line 1064-1065) is unverified — confirm it serializes to `"deepseek"` so dashboards aggregating by `provider_name` don't double-count.

### DS-17 — `auth.rs` fallback chain doesn't cover `SQUEEZY_DEEPSEEK_API_KEY` (nit)

`auth.rs:81-86` recognizes `SQUEEZY_DEEPSEEK_KEY` → `DEEPSEEK_API_KEY`. Some users set `SQUEEZY_DEEPSEEK_API_KEY` (matching upstream env-var naming with the `SQUEEZY_` prefix). Not picked up.

## Wire-Shape Verification (May 2026)

| Aspect | Squeezy default | DeepSeek docs | Status |
|---|---|---|---|
| Base URL | `https://api.deepseek.com/v1` (`lib.rs:90`) | `https://api.deepseek.com/v1` | ✓ |
| Auth | `Authorization: Bearer <key>` (`compatible.rs:474`) | `Authorization: Bearer <key>` | ✓ |
| Env var | `DEEPSEEK_API_KEY` (`lib.rs:2104`) | `DEEPSEEK_API_KEY` | ✓ |
| Default model | `deepseek-chat` (`lib.rs:91`) | deprecating 2026-07-24 → `deepseek-v4-flash` | ✗ DS-2 |
| Streaming | `stream: true, include_usage` (`compatible.rs:209-210`) | supported | ✓ |
| `thinking` body field | not emitted | `thinking.{type, budget_tokens}` | ✗ DS-4 |
| `reasoning_effort` body field | emitted (`compatible.rs:222-223`) | not recognized by V4 | ✗ DS-4 |
| `response_format: json_object` | not emitted | supported | ✗ DS-12 |
| `prompt_cache_hit_tokens` | parsed (`compatible.rs:1147-1151`) | shipped in `usage` | ✓ |
| `prompt_cache_miss_tokens` | not parsed | shipped in `usage` | ✗ DS-3 |
| `reasoning_content` delta surfaced | yes (`compatible.rs:1053-1054`) | streamed at `delta.reasoning_content` | ✓ |
| Reasoning/content ordering | not preserved across chunks | interleavable per docs | ✗ DS-5 |
| `insufficient_system_resource` finish | falls to `_ => {}` (`compatible.rs:1129`) | documented | ✗ DS-7 |
| Vision support | `vision: false` | `deepseek-vision-preview` (beta) | ✗ DS-6 |
| Max output tokens | 8192 (`models.json:797`) | 384000 on V4 | ✗ DS-10 |
| Context window | 131072 (`models.json:795`) | 1000000 on V4 | ✗ DS-10 |
| Concurrency limits | not surfaced | 500 (v4-pro) / 2500 (v4-flash) | ⚠ DS-8 |
| `Retry-After` 429 | honored via `retry.rs:148-153` | returned on 429 | ✓ |
| Tool-choice values | string-only (`compatible.rs:292-294`) | `auto`/`none`/`required` + function-pin | ⚠ shared GQ-3 |
| `prompt_caching` flag | `false` for both rows | automatic prefix caching | ✗ DS-9 |
| Server model alias echo | once per session | always V4 SKU for V3 ids | ⚠ DS-11 |

## Test Coverage

- **Costly**: `crates/squeezy-llm/tests/deepseek_costly.rs` (echo smoke only).
- **Mock**: none. Shared `reasoning_only_stop_emits_done_and_visible_notice` (`compatible_tests.rs:260-298`) covers the synthetic notice but doesn't exercise the DeepSeek-specific noise misfire (DS-1).
- **Registry**: `compatible_tests.rs:925-944` asserts DeepSeek in `is_full_tier`; doesn't check curated rows against upstream pricing.
- **Vision negative**: `lib_tests.rs:598-638` confirms `deepseek-chat` rejects images.
- **Vision-capable DeepSeek SKU positive**: missing (DS-6).
- **Cache miss tokens**: not asserted (DS-3).
- **Thinking parameter round-trip**: not asserted (DS-4).
- **`finish_reason=insufficient_system_resource`**: not asserted (DS-7).

## References

- DeepSeek pricing + V4 SKU table: https://api-docs.deepseek.com/quick_start/pricing
- DeepSeek thinking mode guide: https://api-docs.deepseek.com/guides/thinking_mode
- DeepSeek chat-completion API reference: https://api-docs.deepseek.com/api/create-chat-completion
- DeepSeek rate-limit + concurrency: https://api-docs.deepseek.com/quick_start/rate_limit
- DeepSeek error codes: https://api-docs.deepseek.com/quick_start/error_codes
- V4 developer guide (third-party): https://framia.converge.ai/page/en-US/news/deepseek-v4-api
- V4 thinking modes comparison: https://framia.converge.ai/page/en-US/news/deepseek-v4-thinking-modes
- V4 multimodal preview: https://pixverse.ai/en/blog/deepseek-v4-multimodal-model-coming-to-pixverse
- opencode DeepSeek profile: `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:10`
- opencode DeepSeek fixture (alias echo evidence): `/Users/abbassabra/esqueezy/others/opencode/packages/llm/test/fixtures/recordings/openai-compatible-chat/deepseek-streams-text.json`
