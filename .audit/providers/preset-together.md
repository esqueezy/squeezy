# Together AI Preset Audit

## Summary

- Severity tally: **0 critical / 3 high / 6 medium / 4 low / 1 nit** = **14 preset-specific findings** (shared aggregator findings tracked in `openai-compatible.md`).
- Top three actionable recommendations:
  1. **Refresh the default model id.** `crates/squeezy-core/src/lib.rs:101` pins `meta-llama/Llama-3.3-70B-Instruct-Turbo`. Still active on the serverless catalogue but no longer on Together's marquee roster (now Llama-4 Maverick, DeepSeek-V4-Pro/Flash, GLM-5.1, Qwen3.7-Max, gpt-oss-120b). For a coding-agent default pick `openai/gpt-oss-120b` (reasoning + tools + JSON mode).
  2. **Expose Together extension parameters** (`repetition_penalty`, `top_k`, `min_p`, `safety_model`) via shared-core H3's extra-body escape hatch. None are reachable from `LlmRequest` today; the Llama-Guard safety prefilter is unreachable. This is shared **TG-2**.
  3. **Suppress the noisy reasoning-only-stop notice** at `compatible.rs:1106-1108` for Together-hosted DeepSeek-R1 / gpt-oss. The parser already accumulates both `delta.reasoning` and `delta.reasoning_content` (`compatible.rs:1053-1054`), so reasoning surfaces — but the post-stop hint recommends `tool_choice = "required"`, which is misdirection on a normal thinking-only turn. Same shape as DS-1.

## Verified Configuration Surface

| Aspect | squeezy value | Source | Verified |
|---|---|---|---|
| Base URL | `https://api.together.xyz/v1` | `core/src/lib.rs:100, 2075` | ✓ |
| Default model | `meta-llama/Llama-3.3-70B-Instruct-Turbo` | `core/src/lib.rs:101, 2142` | ✓ active, no longer marquee |
| API key env | `TOGETHER_API_KEY` | `core/src/lib.rs:2111` | ✓ |
| Auth header | `Authorization: Bearer <key>` | `llm/src/compatible.rs:474` | ✓ |
| Preset key / aliases | `"together"`, `"together_ai"` | `core/src/lib.rs:2006, 2171` | ✓ |
| Display name | `"Together AI"` | `core/src/lib.rs:2031` | ✓ |
| `is_full_tier` | `false` | `core/src/lib.rs:2048-2059` | acceptable |
| `models.json` entries | 0 | `llm/src/models.json` | ✗ — TG-3 |
| Registry / config schema | known | `llm/src/registry.rs:228`, `core/src/config_schema.rs:368` | ✓ |
| TUI auth wizard | `SQUEEZY_TOGETHER_KEY` + fallback `TOGETHER_API_KEY` | `cli/src/auth.rs:99-104` | ✓ |
| TOML section | `[providers.together]` | `core/src/lib.rs:8746` | ✓ |
| Default headers / body extras | none | `llm/src/compatible.rs:762-775`, `134-297` | ✓ |
| Telemetry preset | `Together` | `telemetry/src/lib.rs:1022, 1067` | ✓ |
| Docs | `crates/squeezy-skills/external-docs/PROVIDERS.md:308-317` | — | ✓ |

## Implementation Overview

Together is a thin preset on `OpenAiCompatibleProvider`. **There is no Together-specific branch** in `compatible.rs` — every Together call rides the generic chat-completions path (`request_body` at `compatible.rs:134-297`, `stream_response` at `compatible.rs:444-614`, `parse_chat_event` at `compatible.rs:1000-1136`). Footprint: enum arms and constants in `core/src/lib.rs`, one row each in `registry.rs:228`, `config_schema.rs:368`, two in `telemetry/src/lib.rs`, plus the TUI auth entry (`cli/src/auth.rs:99-104`). Docs: `PROVIDERS.md:308-317`.

Wire shape per turn: `stream: true`, `stream_options: { include_usage: true }`, `max_tokens` (if set), `reasoning_effort` + `reasoning: { effort }` (unconditional when `Some(_)`), `prompt_cache_key` (clamped to 64 codepoints), `prompt_cache_retention: "24h"` if Long, `tools`, and `tool_choice` verbatim. No `cache_control` markers — Together never matches `compat_entry` because no `together/` namespace exists in `COMPAT_TABLE` (`compatible.rs:374-403`).

## Verified Wire Facts (June 2026)

- Base URL `https://api.together.xyz/v1`; auth `Authorization: Bearer <TOGETHER_API_KEY>`. Source: docs.together.ai/docs/openai-api-compatibility.
- Accepted body fields: full OpenAI shape (`messages`, `stream`, `stream_options`, `tools`, `tool_choice`, `response_format`, `seed`, `stop`, `temperature`, `top_p`, `frequency_penalty`, `presence_penalty`, `max_tokens`, `n`, `logprobs`, `logit_bias`) **plus Together extensions** `repetition_penalty`, `top_k`, `min_p`, `safety_model`. Accepted-but-ignored: `service_tier`, `store`, `metadata`, `prediction`. `/v1/responses` is **not implemented** (404).
- `tool_choice` accepts the full OpenAI form including `{"type":"function","function":{"name":"…"}}` (verified in opencode fixture).
- Reasoning surface: DeepSeek-R1 streams `delta.reasoning_content`; gpt-oss streams `delta.reasoning` + accepts `reasoning_effort: "low"|"medium"|"high"`. Both already accumulated at `compatible.rs:1053-1054`.
- Vision: standard `image_url` content parts (`data:` and `https://` both accepted); `detail` ignored. Vision-capable today: Llama-4 Maverick, Llama-4 Scout, Qwen3.5 9B/397B, Gemma 4 31B IT.
- Stream shape: OpenAI SSE; **terminal `usage` rides the same chunk as `finish_reason`**, immediately before `[DONE]` — NOT a trailing usage-only chunk (verified opencode `togetherai-streams-text.json`). `cached_tokens` is a top-level sibling of `prompt_tokens`, not nested.
- Rate limits: `x-ratelimit-reset` per response; 429 envelope carries `error_type: "dynamic_request_limited" | "dynamic_token_limited"`. No committed `Retry-After`.
- Separate endpoints (out of scope for the chat preset): `/v1/embeddings`; image generation (FLUX.2/.1, Imagen, Sora 2 Pro). Dedicated SLA-pinned deployments live at `https://model-{x}.api.together.ai/...` and are only reachable via the `Custom` preset.

## Shared-Audit Cross-References

- **C1** (lost usage after `finish_reason`): **does NOT apply to Together**. Verified in `togetherai-streams-text.json` — Together emits `usage` on the same chunk as `finish_reason: "stop"`, immediately before `[DONE]`. Cost reports correctly today.
- **H3** (no `temperature`/`seed`/`top_p`/`frequency_penalty`/`presence_penalty`/`stop` forwarding): applies fully; Together accepts all of them plus the TG-2 extensions.
- **M3** (no `response_format` / `output_schema`): applies. Together supports `json_object` and `json_schema` on gpt-oss-120b, Qwen3.7-Max, DeepSeek-V4-Pro/Flash, GLM-5.1.
- **M6** (`reasoning_effort` always emitted): applies; non-reasoning Together SKUs ignore today, may tighten with validation.
- **M11** (registry-only `ensure_vision_support`): hits hard — see TG-PR-10.
- **TG-1 / TG-2 / TG-3**: see TG-PR-3 / TG-PR-4 / TG-PR-2.

## Together-Specific Findings

### TG-PR-1 (medium) — Default model no longer the marquee SKU

`crates/squeezy-core/src/lib.rs:101` sets `DEFAULT_TOGETHER_MODEL = "meta-llama/Llama-3.3-70B-Instruct-Turbo"`. Verified via `docs.together.ai/docs/serverless-models` as **still active** (131k ctx, $1.04/M I/O, function calling, structured outputs). But it has dropped off Together's featured-models landing page; the marquee roster is now Llama-4 Maverick, DeepSeek-V4-Pro/Flash, GLM-5.1, Qwen3.7-Max, gpt-oss-120b. For a coding-agent default the better pick is `openai/gpt-oss-120b` (reasoning + tools + JSON mode + 131k ctx) or Llama-4 Maverick (multimodal + 1M ctx). The Llama-3.x family also carries the **TG-1** streamed `tool_choice = "required"` quirk that newer SKUs don't.

**Fix**: set `DEFAULT_TOGETHER_MODEL = "openai/gpt-oss-120b"`; sync `PROVIDERS.md:315`.

### TG-PR-2 (high) — TG-3: `models.json` zero coverage

Zero `"provider": "together"` entries. Every Together session falls through `fallback_model_info` (`registry.rs:249-253`): `vision: false` (blocks Llama-4 Maverick / Qwen3.5 images at `compatible.rs:445-447` — shared **M11** ricochet); `pricing: None` (cost zero); generic context window (real ceilings are 1M/131k/512k depending on SKU).

**Fix**: add entries for the marquee roster — `openai/gpt-oss-120b`, `openai/gpt-oss-20b`, `meta-llama/Llama-4-Maverick-17B-128E-Instruct-FP8` (vision), `meta-llama/Llama-4-Scout-17B-16E-Instruct` (vision), `meta-llama/Llama-3.3-70B-Instruct-Turbo` (legacy), `deepseek-ai/DeepSeek-R1`+`-R1-0528`, `deepseek-ai/DeepSeek-V4-Pro`+`-Flash`, `Qwen/Qwen3.7-Max`, `zai-org/GLM-5.1`. Pricing from `together.ai/pricing`.

### TG-PR-3 (medium) — TG-1: `tool_choice = "required"` quirk on Llama-3.x streamed

Together docs note that for Llama-3.x models, streamed `tool_choice = "required"` is occasionally ignored. Llama-4 / gpt-oss / DeepSeek-V4 honor it. squeezy passes `tool_choice` verbatim at `compatible.rs:292-294`, which is correct hands-off behavior — the issue is documentation: the cargo-culted `[model] tool_choice = "required"` does not reliably force tool calls on today's default `Llama-3.3-70B-Instruct-Turbo`.

**Fix**: TG-PR-1 retires Llama-3.3 from the default. Document the quirk in `PROVIDERS.md:308-317` for users pinning a Llama-3.x model.

### TG-PR-4 (medium) — TG-2: Together extensions unreachable

Documented Together body extensions: `repetition_penalty` (float `[0,2]`, Llama repetition control, distinct from `presence_penalty`/`frequency_penalty`), `top_k` (int), `min_p` (float — probability floor), `safety_model` (string, e.g. `"Meta-Llama/LlamaGuard-2-8b"` — opt-in Llama-Guard prompt prefilter). None reachable from `LlmRequest`. The Llama-Guard prefilter is unreachable from `[providers.together]`, which matters for multi-tenant deployments.

**Fix**: when shared **H3** adds `extra_body`, expose these four. Document `safety_model` as a security knob.

### TG-PR-5 (medium) — Reasoning-only-stop notice misfires on DeepSeek-R1 / gpt-oss

`compatible.rs:1094-1108` injects a notice and recommends `tool_choice = "required"` whenever `finish_reason: "stop"` arrives with no visible output but a non-empty reasoning buffer. For Together-hosted DeepSeek-R1 (`delta.reasoning_content`) and gpt-oss (`delta.reasoning`) — both already accumulated at `compatible.rs:1053-1054` — this is the *expected* shape of a thinking-only turn before a tool turn. The injected hint is misdirection. Same shape as **DS-1**.

**Fix**: skip the notice for known reasoning prefixes (`deepseek-ai/DeepSeek-R1*`, `openai/gpt-oss-*`, `deepseek-ai/DeepSeek-V4-*`, `Qwen/Qwen3.7-Max`, `zai-org/GLM-5.1`). A `together/` row in `COMPAT_TABLE` (`compatible.rs:374-403`) would drive this.

### TG-PR-6 (high) — Zero test coverage

No `tests/together_costly.rs`, no mock test. Existing costly tests cover 11 other vendors, not Together. opencode ships reusable canned recordings (`togetherai-streams-text.json`, `togetherai-streams-tool-call.json`) under `others/opencode/packages/llm/test/fixtures/recordings/openai-compatible-chat/` — no API key needed.

**Fix**: add a Together row to the parameterized mock harness (shared §). Assert (a) `usage` parses from the `finish_reason` chunk (TG-PR-7 regression), (b) `delta.reasoning_content` accumulates (DeepSeek-R1 path), (c) pinned-function `tool_choice` round-trips (TG-PR-8).

### TG-PR-7 (low) — `cached_tokens` is top-level, not nested

Verified opencode fixture: `{"prompt_tokens":45,"completion_tokens":3,"total_tokens":48,"cached_tokens":0}`. `cached_tokens` is a top-level sibling of `prompt_tokens`, not nested under `prompt_tokens_details.cached_tokens` (OpenAI) or aliased as `prompt_cache_hit_tokens` (DeepSeek). `parse_chat_usage` (`compatible.rs:1147-1151`) probes both alternates but not the flat one — Together prefix-cache hits are silently dropped.

**Fix**: add `.or_else(|| usage.get("cached_tokens"))` to the chain.

### TG-PR-8 (medium) — `tool_choice` cannot express pinned-function shape (shared GQ-3)

The opencode fixture shows Together natively accepts `"tool_choice":{"type":"function","function":{"name":"get_weather"}}`. `LlmRequest::tool_choice: Option<String>` restricts users to bare strings — explicit pinning is unreachable. High-value because Llama-4 Maverick and DeepSeek-V4 behave best with explicit pinning on multi-step tool flows. Shared **GQ-3** covers.

### TG-PR-9 (low) — Rate-limit hints not consumed

Together response headers include `x-ratelimit-reset`; 429 envelopes carry `error_type: "dynamic_request_limited" | "dynamic_token_limited"`. squeezy's `retry.rs:148-153` honors `Retry-After` only — Together does not commit to `Retry-After`. Shared **GQ-4** covers. Lower than Groq/Cerebras because Together's rate is dynamic per-model and the reset is a hint, not a hard cooldown.

### TG-PR-10 (medium) — Vision capability hard-coded `false`

Direct consequence of TG-PR-2: no `models.json` entry means `vision: false` for every Together model, so Llama-4 Maverick / Qwen3.5 image turns reject at `ensure_vision_support` before the request leaves the client. **Fix**: lands with TG-PR-2; interim mitigation is a `compat_entry` row keyed on `meta-llama/Llama-4-`, `Qwen/Qwen3.5-`, `google/gemma-4-` prefixes with a vision flag.

### TG-PR-11 (low) — Dedicated endpoints reachable only via `Custom`

Per-deployment URLs (`https://model-{x}.api.together.ai/...`) for SLA-pinned hosts aren't expressible through the preset; users must use `Custom`. Document at `PROVIDERS.md:308-317`.

### TG-PR-12 (medium) — `/v1/responses` not implemented

Non-issue today (chat-completions only). Call out if a future routing change dispatches Together via the Responses path — it 404s.

### TG-PR-13 (low) — Separate endpoints out of scope

`/v1/embeddings` and image-generation (FLUX.2, FLUX.1, Imagen, Sora 2 Pro) live on separate endpoints. squeezy has neither surface.

### TG-PR-14 (nit) — `display_name` and TUI auth env split

`lib.rs:2031` returns `"Together AI"` (matches vendor casing). `cli/src/auth.rs:99-104` defines `SQUEEZY_TOGETHER_KEY` + `TOGETHER_API_KEY` fallback — matches Fireworks/Cerebras at `auth.rs:106-116`.

## Catalog Verification (June 2026)

All "production (marquee)" entries are featured on the `together.ai/models` landing page; "active" = on the serverless catalogue but no longer marquee.

| Together model id | Status | Tools | Reasoning surface | Vision | squeezy aware |
|---|---|---|---|---|---|
| `openai/gpt-oss-120b` | marquee | ✓+parallel | `delta.reasoning` + `reasoning_effort` | ✗ | no |
| `openai/gpt-oss-20b` | active | ✓ | `delta.reasoning` | ✗ | no |
| `meta-llama/Llama-4-Maverick-17B-128E-Instruct-FP8` | marquee | ✓ | ✗ | ✓ (1M ctx) | no |
| `meta-llama/Llama-4-Scout-17B-16E-Instruct` | active | ✓ | ✗ | ✓ | no |
| `meta-llama/Llama-3.3-70B-Instruct-Turbo` | active (not marquee) | ✓ (TG-1) | ✗ | ✗ | yes (default — TG-PR-1) |
| `deepseek-ai/DeepSeek-R1` / `-R1-0528` | active | ✓ | `delta.reasoning_content` | ✗ | no |
| `deepseek-ai/DeepSeek-V4-Pro` / `-Flash` | marquee NEW | ✓ | `delta.reasoning_content` | ✗ | no |
| `Qwen/Qwen3.7-Max` | marquee NEW | ✓ | ✓ | ✗ | no |
| `Qwen/Qwen3.5-9B` / `-397B-A17B` | active | ✓ | ✓ | ✓ | no |
| `zai-org/GLM-5.1` | marquee NEW | ✓ | ✓ | ✗ | no |
| `google/gemma-4-31B-it` | active | ✓ | ✓ | ✓ | no |
| `meta-llama/Meta-Llama-3.1-8B-Instruct-Turbo` | **retired 2026-03-06** | — | — | — | no |

## Verification Strategy

1. **Mock case in the parameterized harness** (shared §): reuse opencode's `togetherai-streams-text.json` + `togetherai-streams-tool-call.json` as canned SSE. Assert `state.cost.input_tokens == 45` (no C1, plus `cached_tokens` parse via TG-PR-7); assert one `LlmEvent::ToolCall { name: "get_weather", arguments: {"city": "Paris"} }`.
2. **Costly test** (`tests/together_costly.rs`): smoke against `openai/gpt-oss-120b` for (a) plain content, (b) reasoning with `reasoning_effort=low` asserting `state.reasoning_buf` populates from `delta.reasoning`, (c) pinned-function `tool_choice` (TG-PR-8).
3. **Catalog lint**: CI fails when `DEFAULT_TOGETHER_MODEL` is not in `models.json`. Catches TG-PR-1 next rotation; today fires immediately.
4. **401-ping**: mock returning 401 — assert the error includes the `"Together AI"` display name and `format_chat_error` (`compatible.rs:976-998`) doesn't flatten upstream `error.message`. Together's envelope is OpenAI-shaped; should pass with no preset-specific change.
5. **Reasoning-only-stop gate** (TG-PR-5): mock a `gpt-oss-120b` response ending with `finish_reason: "stop"` after only `delta.reasoning` deltas. Assert the `[squeezy] model finished without emitting any content` notice is suppressed after fix.

## References

- OpenAI compatibility: https://docs.together.ai/docs/openai-api-compatibility
- Serverless models: https://docs.together.ai/docs/serverless-models
- Marquee roster: https://www.together.ai/models
- Rate limits: https://docs.together.ai/docs/rate-limits
- Pricing: https://www.together.ai/pricing
- Llama 4: https://www.together.ai/blog/llama-4
- DeepSeek-R1 model page: https://www.together.ai/models/deepseek-r1
- Embeddings (separate): https://www.together.ai/blog/embeddings-endpoint-release
- FLUX.2 image gen (separate): https://www.together.ai/blog/flux-2-multi-reference-image-generation-now-available-on-together-ai
- Shared aggregator audit: `/Users/abbassabra/esqueezy/squeezy/.audit/providers/openai-compatible.md` (TG-1, TG-2, TG-3)
- Sibling audits: `preset-cerebras.md`, `preset-fireworks.md`, `preset-mistral.md`, `preset-deepseek.md` (same `.audit/providers/`)
- opencode profile (peer impl): `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:14`
- opencode preset binding: `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/openai-compatible.ts:65`
- opencode SSE recordings (`/Users/abbassabra/esqueezy/others/opencode/packages/llm/test/fixtures/recordings/openai-compatible-chat/`):
  - `togetherai-streams-text.json` — verifies C1-immunity (`usage` on same chunk as `finish_reason`) and top-level `cached_tokens` shape
  - `togetherai-streams-tool-call.json` — verifies pinned-function `tool_choice` (TG-PR-8 / GQ-3)
