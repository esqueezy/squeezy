# Baseten Preset Audit

## Summary

- Severity tally: **0 critical / 2 high / 4 medium / 2 low / 1 nit** = **9 findings** (all preset-specific; shared-core findings referenced by ID).
- Top 3 actionable recommendations:
  1. **Refresh the default model** at `crates/squeezy-core/src/lib.rs:109`. `meta-llama/Meta-Llama-3.1-70B-Instruct` is no longer on Baseten's shared Model APIs catalog (May 2026: catalog is `deepseek-ai/DeepSeek-V4-Pro`, `zai-org/GLM-4.7`/`5`/`5.1`, `moonshotai/Kimi-K2.5`/`K2.6`, `nvidia/Nemotron-120B-A12B`, `openai/gpt-oss-120b`). A bare-defaults config 404s today. Pick `openai/gpt-oss-120b` (cheapest, broadly capable, well-supported by squeezy's chat-completions pipeline since `usage` echoes prompt-cache details) and add a curated `models.json` row.
  2. **Add a per-deployment URL escape hatch (BT-1)** beyond `Custom`. Baseten's dedicated Inference endpoint shape `https://model-{id}.api.baseten.co/environments/production/sync/v1` is the *only* path for SLA-pinned and bring-your-own-checkpoint models. Today users must downgrade to `Custom` (losing `BASETEN_API_KEY` discovery, losing the `baseten` registry label, losing the `baseten` model-alias namespace). Either: (a) add a sibling preset `BasetenDeployment` whose `default_base_url` accepts a `{deployment_id}` placeholder routed through `substitute_url_placeholders` (compatible.rs:701-745); or (b) extend `OpenAiCompatibleConfig` with an optional `deployment_id` and have the `Baseten` preset autoswitch URL shapes when populated.
  3. **Emit `chat_template_args.enable_thinking = true` for reasoning-eligible Baseten models** (DeepSeek/GLM/Kimi/Nemotron flavors that gate reasoning behind the chat template). Today `request_body` (compatible.rs:215-224) emits only `reasoning_effort` + `reasoning.effort`, which Baseten ignores — the inference server reads `chat_template_args` for these checkpoints. opencode does exactly this (`others/opencode/packages/opencode/src/provider/transform.ts:1070-1075`) for `providerID === "baseten"`. Without it, reasoning silently never fires on shared-endpoint DeepSeek/Kimi/GLM, so squeezy ships generic-completion behavior at reasoning-model prices.

## Verified

- Base URL: `https://inference.baseten.co/v1` (`crates/squeezy-core/src/lib.rs:108`) — Verified: ✓ (matches Baseten docs and opencode profile `others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:7`).
- Auth: `Authorization: Bearer <key>` via `bearer_auth(key)` (`crates/squeezy-llm/src/compatible.rs:474`) — Verified: ✓ (Baseten also accepts legacy `Api-Key` header for backward compatibility, but Bearer is the documented modern form).
- Env var: `BASETEN_API_KEY` (`crates/squeezy-core/src/lib.rs:2115`) — Verified: ✓.
- Default model: `meta-llama/Meta-Llama-3.1-70B-Instruct` (`crates/squeezy-core/src/lib.rs:109`) — Verified: ✗ (stale — see BT-3).

## Implementation Overview

Squeezy's Baseten preset is a thin metadata pin on the shared `OpenAiCompatibleProvider` (`crates/squeezy-llm/src/compatible.rs:39-46`): default base URL, default model, default env-var, and a CLI/TOML alias (`crates/squeezy-core/src/lib.rs:108-109, 1978, 2010, 2035, 2079, 2115, 2146, 2175, 2206, 8750`). No preset-specific branches exist in `compatible.rs` — no `preset_default_headers` row (compatible.rs:762-775), no `request_body` body-field branches (compatible.rs:134-297), no SSE quirks. Cost extraction uses generic `parse_chat_usage` (compatible.rs:1138-1164) reading OpenAI-shape `prompt_tokens` / `completion_tokens` / `prompt_tokens_details.cached_tokens` / `completion_tokens_details.reasoning_tokens`. Baseten's Model APIs return all of these (per their changelog + `docs.baseten.co/api-reference/openai`), so usage extraction works structurally — but cost reporting still degrades to zero because `models.json` has no Baseten entries (BT-5).

The structural gap: Baseten serves two distinct surfaces: (1) Model APIs at `https://inference.baseten.co/v1` (eight curated models), and (2) dedicated Inference deployments at `https://model-{model_id}.api.baseten.co/environments/{environment}/sync/v1` (BYO checkpoints). Squeezy only addresses (1); (2) is reachable only via `Custom`, which strips the `baseten` namespace and forces re-spelling base URL + env var. LiteLLM models this split natively (`baseten/{8-digit-id}` vs `baseten/<model-name>` under one provider).

## Findings

### BT-1 (high) — Per-deployment URL unaddressable via preset

- **Location**: `crates/squeezy-core/src/lib.rs:108, 2079`; `crates/squeezy-llm/src/compatible.rs:701-745`.
- **Observed**: `default_base_url` is hard-pinned to `https://inference.baseten.co/v1`. `substitute_url_placeholders` only knows `{account_id}` / `{gateway_id}`.
- **Issue**: Users on a dedicated Baseten deployment must drop to `Custom`, losing (a) `BASETEN_API_KEY` autoload, (b) the `baseten` provider label in transcripts/cost reports, (c) the `[providers.baseten]` TOML section.
- **Impact**: Major friction for Baseten's flagship use case (BYO checkpoints, SLA pinning) — the primary reason Baseten differs from Together/Fireworks/DeepInfra. Promoted from M to H.
- **Fix sketch**: Add `{deployment_id}` (+ optional `{environment}` defaulting to `production`) to the placeholder table, and ship a sibling preset `BasetenDeployment` whose `default_base_url` is `https://model-{deployment_id}.api.baseten.co/environments/{environment}/sync/v1`, sharing env var and label.
- **Reference**: `others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:7`; LiteLLM `https://docs.litellm.ai/docs/providers/baseten`; `https://docs.baseten.co/inference/calling-your-model`.

### BT-2 (high) — Reasoning never fires on shared-endpoint reasoning models

- **Location**: `crates/squeezy-llm/src/compatible.rs:215-224`.
- **Observed**: `request_body` emits `reasoning_effort` and `reasoning: { effort }`; never emits `chat_template_args.enable_thinking`.
- **Issue**: Baseten's reasoning-capable shared models (DeepSeek V4 Pro, Kimi K2.5/K2.6, GLM 4.7/5/5.1, Nemotron Super) gate the thinking pass on `chat_template_args.enable_thinking` — their vLLM/SGLang serving layer reads the chat-template arg, not the OpenAI knob. `[model] reasoning_effort = "high"` against `zai-org/GLM-4.7` pays reasoning-tier rates and receives a non-reasoning answer.
- **Impact**: Silent quality + cost regression on the only reasoning-capable shared models.
- **Fix sketch**: When `request.reasoning_effort` is set AND preset is `Baseten`, also emit `chat_template_args: { enable_thinking: true }`. Cleanest as a per-preset branch in `request_body`; or attach a `requires_chat_template_thinking` flag to a future `baseten/` `CompatEntry`.
- **Reference**: `others/opencode/packages/opencode/src/provider/transform.ts:1070-1075` (`providerID === "baseten"` triggers the same field); Baseten reasoning docs (2026) describe `extra_body={"chat_template_args": {"enable_thinking": True}}`.

### BT-3 (medium) — Default model is decommissioned

- **Location**: `crates/squeezy-core/src/lib.rs:109`.
- **Observed**: `DEFAULT_BASETEN_MODEL = "meta-llama/Meta-Llama-3.1-70B-Instruct"`.
- **Issue**: Verified May 2026 — Llama 3.1 70B Instruct is no longer on Baseten's shared catalog. Current catalog: DeepSeek V4 Pro, GLM 4.7/5/5.1, Kimi K2.5/K2.6, Nemotron Super, OpenAI GPT-OSS 120B. Fresh `squeezy --provider baseten` with no model override 404s.
- **Impact**: First-run failure for new Baseten users.
- **Fix sketch**: Replace with `openai/gpt-oss-120b` (cheapest, default-on tool calling + reasoning) or `zai-org/GLM-4.6` for a non-reasoning default. Wire BT-5 in the same change.
- **Reference**: `https://docs.baseten.co/api-reference/openai`.

### BT-4 (medium) — Interleaved reasoning_content cue dropped

- **Location**: `crates/squeezy-llm/src/compatible.rs:1053`.
- **Observed**: `parse_chat_event` reads `delta.reasoning_content` into the reasoning accumulator. Functional but boundary-blind.
- **Issue**: Baseten's DeepSeek/Kimi/GLM streams **interleave** `reasoning_content` and `content` within the same choice's delta stream. The parser preserves both texts but tracks them sequentially without per-delta source attribution; the TUI can't cleanly separate thinking from final answer. Compounds BT-2.
- **Impact**: Reasoning trace renders but can't be visually attributed to the thinking pass.
- **Fix sketch**: Track which field carried the latest delta; emit `LlmEvent::ReasoningSegment` boundaries on transitions.
- **Reference**: `others/opencode/packages/opencode/test/tool/fixtures/models-api.json:69505-69509` (`"interleaved": {"field": "reasoning_content"}`).

### BT-5 (medium) — No `models.json` entries

- **Location**: `crates/squeezy-llm/src/models.json` (no `baseten` namespace).
- **Observed**: Zero entries. `is_full_tier` (lib.rs:2048-2059) returns `false` for `Baseten`; registry falls back to generic context + zero cost.
- **Issue**: Context-window detection, cost reporting, vision capability all degrade to defaults. TUI shows `$0.00` per turn; long prompts hit no overflow warning even though shared models have 131k/202k/262k contexts; Kimi K2.5/K2.6 vision flag falls back to false.
- **Impact**: Cost accounting, context warnings, and vision routing all broken.
- **Fix sketch**: Add entries for the eight shared-endpoint models (pricing on `docs.baseten.co/api-reference/openai`), include vision flag for Kimi. Pattern after OpenRouter/Vercel entries.
- **Reference**: `others/opencode/packages/opencode/test/tool/fixtures/models-api.json:69411-69660`.

### BT-6 (medium) — KV-cache discount line missing from cost model

- **Location**: `crates/squeezy-llm/src/compatible.rs:1138-1164`.
- **Observed**: `parse_chat_usage` reads `prompt_tokens_details.cached_tokens` — Baseten echoes the count correctly on this field.
- **Issue**: Cost is computed downstream from token counts × `models.json` rates. With no Baseten `models.json` rows (BT-5), there's no `input_cached_per_token_usd` to consult; even with rows added, the shared model assumes a single cached rate per model and Baseten's discount varies by checkpoint family.
- **Impact**: Cost overstated on KV-cache hits (Baseten's documented discount is 50-80% off input rate); reported $/turn can be 2-3× actual on cache-warm sessions.
- **Fix sketch**: As part of BT-5, populate per-model `input_cached_per_token_usd` from Baseten's posted discount table.
- **Reference**: `https://www.baseten.co/resources/changelog/baseten-is-fully-openai-compatible/` confirms OpenAI-shape `prompt_tokens_details.cached_tokens`.

### BT-7 (low) — No costly integration test

- **Location**: `crates/squeezy-llm/tests/` — no `baseten_costly.rs`.
- **Observed**: Zero coverage (no costly test, no mock test, no fixture).
- **Issue**: Shared-core regressions (C1 post-`finish_reason` usage, H4 `index` partition, H6 error shape) ship without a Baseten-side regression check.
- **Fix sketch**: Add `tests/baseten_costly.rs` modeled on `groq_costly.rs` — single turn against `openai/gpt-oss-120b` with `tool_choice = "required"`, assert `cached_tokens` parse, then a reasoning fixture once BT-2 lands.
- **Reference**: `crates/squeezy-llm/tests/groq_costly.rs`.

### BT-8 (low) — Vision input shape unverified end-to-end

- **Location**: `crates/squeezy-llm/src/compatible.rs:202` (`chat_message`).
- **Observed**: Generic OpenAI-shape `image_url` content blocks.
- **Issue**: Kimi K2.5/K2.6 are the shared catalog's only vision models; no test asserts image round-trip against Baseten. The capability flag is also missing (gated by BT-5).
- **Fix sketch**: Set `vision: true` in BT-5 rows for Kimi; add a small image-URL fixture to BT-7.
- **Reference**: `https://docs.baseten.co/api-reference/openai`.

### BT-9 (nit) — Display name

- **Location**: `crates/squeezy-core/src/lib.rs:2035`. `"Baseten"` matches brand and opencode. Fine.

## Catalog

| Model id (shared endpoint, May 2026) | Context | Max output | Tool calling | Reasoning gate | Vision | In squeezy `models.json`? |
|---|---|---|---|---|---|---|
| `deepseek-ai/DeepSeek-V4-Pro` | 131k | 131k | ✓ | default-on | ✗ | ✗ |
| `zai-org/GLM-4.7` | 200k | 200k | ✓ | opt-in via `chat_template_args.enable_thinking` | ✗ | ✗ |
| `zai-org/GLM-5` | 202k | 202k | ✓ | opt-in | ✗ | ✗ |
| `zai-org/GLM-5.1` | 202k | 202k | ✓ | opt-in | ✗ | ✗ |
| `moonshotai/Kimi-K2.5` | 262k | 262k | ✓ | opt-in | ✓ | ✗ |
| `moonshotai/Kimi-K2.6` | 262k | 262k | ✓ | opt-in | ✓ | ✗ |
| `nvidia/Nemotron-120B-A12B` | 202k | 202k | ✓ | default-on | ✗ | ✗ |
| `openai/gpt-oss-120b` | 128k | 128k | ✓ | default-on | ✗ | ✗ |

Squeezy's `models.json` has **zero** Baseten entries. The current `DEFAULT_BASETEN_MODEL = "meta-llama/Meta-Llama-3.1-70B-Instruct"` (lib.rs:109) is **not** in the catalog — Llama 3.1 70B is no longer served on the shared endpoint (BT-3).

## Test Coverage Gaps

Nothing today. No costly test, no mock test, no fixture, no `models.json` row. The preset rides exclusively on the shared `OpenAiCompatibleProvider` test surface (`crates/squeezy-llm/src/compatible_tests.rs`), which exercises generic chat-completions wiring but not a single Baseten-specific assertion.

The dedicated-deployment URL split (BT-1) is doubly invisible because there is no test covering it under any preset — `Custom` has no SSRF/URL-shape test either (CT-1 in the shared audit).

## Verification Strategy

- **401-ping**: `curl -i -H "Authorization: Bearer wrong" -X POST https://inference.baseten.co/v1/chat/completions -d '{"model":"openai/gpt-oss-120b","messages":[{"role":"user","content":"hi"}]}'` returns `401` with a JSON error envelope; usable as a config-check fixture without spending tokens.
- **Mock harness**: Stand up a `wiremock`-style server returning fixed SSE chunks (one `reasoning_content` delta, one `content` delta, one `usage` chunk with `prompt_tokens_details.cached_tokens: 1024`) and assert the parser surfaces both segments and the cached-token count. This covers BT-4 and BT-6 without a Baseten key.
- **Smoke costly** (1¢): Single 200-token completion against `openai/gpt-oss-120b` with `tool_choice = "auto"` and a no-op tool; assert finish_reason, usage parse, and non-zero `cached_input_tokens` on a second identical call.
- **Reasoning costly** (5¢, once BT-2 lands): Request against `zai-org/GLM-4.7` with `chat_template_args.enable_thinking = true`; assert at least one `reasoning_content` delta arrives before the first `content` delta.

## References

- `crates/squeezy-core/src/lib.rs:108-109, 1978, 2010, 2035, 2079, 2115, 2146, 2175, 2206, 8750` — all Baseten preset metadata.
- `crates/squeezy-llm/src/compatible.rs:134-297` (`request_body`), `:1138-1164` (`parse_chat_usage`), `:1053` (reasoning_content), `:701-745` (`substitute_url_placeholders`), `:762-775` (`preset_default_headers`) — shared-core touch points.
- `.audit/providers/openai-compatible.md:351-359` — prior Baseten section (BT-1, BT-2 IDs introduced there but redefined here with deeper sourcing).
- `others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:7` — opencode's Baseten profile (matches squeezy's base URL).
- `others/opencode/packages/opencode/src/provider/transform.ts:1070-1075` — opencode's `chat_template_args: { enable_thinking: true }` injection for `providerID === "baseten"`.
- `others/opencode/packages/opencode/test/tool/fixtures/models-api.json:69411-69660` — Baseten model catalog reference (use as `models.json` starting point).
- Baseten Model APIs overview: https://docs.baseten.co/development/model-apis/overview
- Baseten OpenAI compat reference: https://docs.baseten.co/api-reference/openai
- Baseten dedicated deployments: https://docs.baseten.co/inference/calling-your-model
- Baseten Frontier Gateway (separate product, not relevant to this preset): https://www.baseten.co/blog/introducing-baseten-frontier-gateway/
- LiteLLM Baseten provider (models the dedicated-vs-shared split natively): https://docs.litellm.ai/docs/providers/baseten
- Baseten changelog "now fully OpenAI compatible": https://www.baseten.co/resources/changelog/baseten-is-fully-openai-compatible/
- Cloudflare AI Gateway Baseten: https://developers.cloudflare.com/ai-gateway/usage/providers/baseten/
