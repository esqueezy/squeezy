# Cerebras Preset Audit

## Summary

- Severity tally: **1 critical / 4 high / 5 medium / 4 low / 2 nit** = **16 preset-specific findings** (shared-core findings tracked in `openai-compatible.md`).
- Top 3 actionable recommendations:
  1. **Replace the default model**: `crates/squeezy-core/src/lib.rs:105` pins `llama-3.3-70b`, retired **2026-02-16**. Cerebras' own deprecation page names `gpt-oss-120b` as the replacement (production tier, 131k ctx, tool calling, reasoning).
  2. **Adopt `max_completion_tokens`** (or send both). Cerebras' chat-completions reference documents only `max_completion_tokens`. Squeezy emits `max_tokens` at `crates/squeezy-llm/src/compatible.rs:212-214`. Cerebras still accepts it today as an alias but API v2 (default **2026-07-21**) tightens validation, and reasoning models charge thinking tokens against this budget.
  3. **Fix shared-core C1 for Cerebras specifically** — Cerebras emits the trailing `usage` chunk after `finish_reason: "stop"` exactly like Groq, so every Cerebras cost report today is $0. High-throughput streaming makes the UX gap more visible, not less.

## Verified Configuration Surface

| Aspect | squeezy value | Source (file:line) | Verified |
|---|---|---|---|
| Base URL | `https://api.cerebras.ai/v1` | `crates/squeezy-core/src/lib.rs:104` | ✓ |
| Default model | `llama-3.3-70b` | `crates/squeezy-core/src/lib.rs:105` | ✗ **deprecated 2026-02-16** |
| API key env | `CEREBRAS_API_KEY` | `crates/squeezy-core/src/lib.rs:2113` | ✓ |
| Auth header | `Authorization: Bearer <key>` | `crates/squeezy-llm/src/compatible.rs:474` | ✓ |
| Preset key / alias | `"cerebras"` (single canonical) | `crates/squeezy-core/src/lib.rs:2008, 2173` | ✓ |
| Display name | `"Cerebras"` | `crates/squeezy-core/src/lib.rs:2033` | ✓ |
| `is_full_tier` | `false` | `crates/squeezy-core/src/lib.rs:2048-2059` | acceptable |
| `models.json` entries | 0 (`grep -c cerebras` → 0) | `crates/squeezy-llm/src/models.json` | ✗ — shared-core CB-4 |
| Registry-known | yes | `crates/squeezy-llm/src/registry.rs:230` | ✓ |
| Default headers | none | (no preset branch) | ✓ |
| Default body extras | none | `crates/squeezy-llm/src/compatible.rs:134-297` | ✓ |
| Tool-fallback note | acknowledges Cerebras 4xx on tools | `crates/squeezy-llm/src/model_discovery.rs:303` | ✓ |
| Telemetry preset | `Cerebras` | `crates/squeezy-telemetry/src/lib.rs:1024, 1069` | ✓ |

## Implementation Overview

Cerebras is a thin preset on top of `OpenAiCompatibleProvider`. There is no Cerebras-specific code path in `compatible.rs` beyond the routing table entries. Preset configuration is exhausted by the constants above plus the `OpenAiCompatiblePreset::Cerebras` enum arms (16 occurrences in `crates/squeezy-core/src/lib.rs`, one in `crates/squeezy-llm/src/registry.rs:230`, two in `crates/squeezy-telemetry/src/lib.rs`). Documentation lives at `crates/squeezy-skills/external-docs/PROVIDERS.md:330-340`.

Cerebras' selling point — high tokens/sec, batched delivery at "200 evenly-spaced events per second" (2025-10-06 changelog) — means each SSE event carries multiple tokens of `delta.content`. This stresses two shared assumptions: (a) `SseDecoder::decode_sse_event` (`crates/squeezy-llm/src/sse.rs:49-63`) joins data lines with `\n`, so any `[DONE]` batched onto a usage chunk triggers shared-core L4; (b) the trailing usage chunk after `finish_reason` triggers shared-core C1.

## Cerebras-Specific Findings

### CB-PR-1 (critical) — Default model `llama-3.3-70b` is retired

`crates/squeezy-core/src/lib.rs:105` sets `DEFAULT_CEREBRAS_MODEL = "llama-3.3-70b"`. Cerebras' deprecation page (verified June 2026) marks `llama-3.3-70b` retired on **2026-02-16**, recommending `gpt-oss-120b`. Same date retires `qwen-3-32b`; later dates retire `llama3.1-8b` and `qwen-3-235b-a22b-instruct-2507` (2026-05-27). Any new Cerebras account hitting `squeezy --provider cerebras` cold-starts on a 400; `OpenAiCompatibleProvider::from_config` carries no fallback.

`gpt-oss-120b` is the canonical successor: production tier, 131k context, MoE (5.1B active per token), tool calling + parallel tool calls + structured outputs + reasoning. Cerebras' OpenAI-compat page notes it accepts both `system` and `developer` message roles; squeezy uses only `system`, so this is transparent.

**Fix**: set `DEFAULT_CEREBRAS_MODEL = "gpt-oss-120b"`; update `PROVIDERS.md:338`; mention `zai-glm-4.7` as the reasoning alternative.

### CB-PR-2 (high) — `max_tokens` vs `max_completion_tokens`

`crates/squeezy-llm/src/compatible.rs:212-214` emits `max_tokens`. Cerebras' chat-completions reference documents only `max_completion_tokens` and notes "reasoning tokens are counted toward total completion tokens even when not displayed". Cerebras today accepts `max_tokens` as an alias, but:
- API v2 (default-switchover **2026-07-21**) tightens schema validation.
- For `gpt-oss-120b`/`zai-glm-4.7` reasoning, sending the legacy alias is fragile — the documented budget controls the visible-output share.

**Fix**: emit `max_completion_tokens` (sending both keys is safe today; switching outright future-proofs against v2).

### CB-PR-3 (high) — Reasoning field name verified: `delta.reasoning` works today

Cerebras' reasoning page confirms:
- Streaming reasoning is delivered in `delta.reasoning` (OpenAI-style), not `delta.reasoning_content` (DeepSeek-style).
- Request-side param: `reasoning_effort ∈ {low, medium, high}` (plus `"none"` on GLM 4.7).
- Reasoning-supporting models: `gpt-oss-120b`, `zai-glm-4.7`.

Squeezy's `parse_chat_event` at `crates/squeezy-llm/src/compatible.rs:1053-1054` concatenates both fields, so reasoning **rendering** works on Cerebras out of the box. But emission of `reasoning: {effort}` + `reasoning_effort` is unconditional (`compatible.rs:215-223`) — non-reasoning Cerebras models historically ignored unknown fields, but API v2 stricter validation may 400 on Llama/Qwen instruct SKUs receiving `reasoning_effort`. Shared-core M6 covers the gate.

Additionally, the `reasoning_only_stop` notice at `compatible.rs:1106-1108` advises `tool_choice = "required"` as a remedy. For a `gpt-oss-120b` turn that legitimately finishes thinking, this advice is misdirected — same shape as DeepSeek DS-1.

**Fix**: gate `reasoning_effort` on a known reasoning model id (`gpt-oss-*`, `zai-glm-*`, `*-thinking-*`).

### CB-PR-4 (high) — Shared-core C1 lands hard on Cerebras (cost = $0)

Cerebras' usage chunk shape: the trailing chunk after `finish_reason: "stop"` carries `usage: { prompt_tokens, completion_tokens, total_tokens, prompt_tokens_details: { cached_tokens } }`. Per shared-core C1 (`crates/squeezy-llm/src/compatible.rs:563-580`), squeezy returns from the outer loop before parsing that chunk, so `state.cost` stays zero on every Cerebras turn.

**Fix**: shared-core C1.

### CB-PR-5 (high) — `prompt_cache_key` already wired, `cached_tokens` already parsed

Changelog 2026-04-22 added top-level `prompt_cache_key` — semantically identical to OpenAI's. Squeezy emits it at `compatible.rs:236`, so this works automatically (subject to shared-core H8 64-codepoint clamp). Cerebras' broader prompt-cache launch (2025-12-10) populates `prompt_tokens_details.cached_tokens`, which `parse_chat_usage` reads. Once C1 is fixed, cached-input accounting surfaces correctly. No Cerebras-specific work needed beyond H8.

### CB-PR-6 (medium) — `stream_options.include_usage` 400 risk lives on dedicated endpoints

Squeezy always emits `stream_options: { include_usage: true }` (`compatible.rs:210`). The public `api.cerebras.ai/v1` accepts it (it's the trigger for the trailing usage chunk). The historical 400 risk (shared-core CB-2) lives on dedicated-endpoint URLs (`https://model-{x}.api.cerebras.ai/...`) running older API versions — reachable only via the `Custom` preset, so the public preset is fine today.

### CB-PR-7 (medium) — `tool_choice` and `parallel_tool_calls`

Cerebras' chat-completions reference: `tool_choice` accepts `"none"`, `"auto"`, `"required"`, `{"type":"function","function":{"name":"..."}}`. `parallel_tool_calls` (bool) supported since **2025-12-17**. Squeezy's `LlmRequest::tool_choice: Option<String>` (shared-core GQ-3) can't express the explicit-function object form. `LlmRequest::parallel_tool_calls` exists but the chat-completions provider ignores it (shared-core M2) — both `gpt-oss-120b` and the GLM family document parallel tool calls as core capability, so this is squarely a Cerebras-relevant gap.

### CB-PR-8 (medium) — `response_format` / `json_schema` dropped

Cerebras' structured-outputs page: `gpt-oss-120b` supports `response_format: { type: "json_schema", json_schema: { name, description, schema, strict: true } }` with token-level constrained decoding when `strict: true` (2026-01-09 GA). Squeezy's `LlmRequest::output_schema` is silently dropped by the chat-completions provider — shared-core M3. Cerebras is a high-value target because squeezy already carries the schema in the request type.

### CB-PR-9 (medium) — Shared-core H3 ricochet: sampling params

Cerebras historically rejected `frequency_penalty`, `presence_penalty`, `logit_bias` with 400; the 2026-03-31 changelog adopted them on the public endpoint. Today squeezy doesn't emit any (shared-core H3). When H3 lands, dedicated-endpoint customers on pinned older versions may regress — call out the carve-out at H3 implementation time.

### CB-PR-10 (medium) — `seed`, `stop`, `temperature` ranges

Cerebras supports `seed`, `stop` (capped at 4 sequences), `top_p ∈ [0,1]`, `temperature ∈ [0,2.0]` (bumped 2026-03-26). When shared-core H3 forwards these, the 4-sequence `stop` cap is the only Cerebras-specific clamp to enforce.

### CB-PR-11 (low) — Rate-limit hints not consumed

Cerebras emits `x-ratelimit-{limit,remaining,reset}-{requests-day,tokens-minute}` on every response. Cerebras docs do not commit to `Retry-After` on 429s, so `crates/squeezy-llm/src/retry.rs:148-153` has nothing to honor. Shared-core GQ-4 (proactive `x-ratelimit-*` backoff) covers Cerebras too.

### CB-PR-12 (low) — Multi-token streaming surfaces shared-core L2

Cerebras delivers ~200 events/sec, each batched. `find_event_boundary` (`crates/squeezy-llm/src/sse.rs:36-47`) re-scans the buffer on every push — at 3000 tok/sec the cumulative re-scan cost is non-trivial. Shared-core L2 fix benefits Cerebras most.

### CB-PR-13 (low) — Vision: confirmed unsupported

Current Cerebras catalog (`gpt-oss-120b`, `zai-glm-4.7`, retired Llama/Qwen SKUs) has no vision-capable model. `CONSERVATIVE_FALLBACK_CAPABILITIES` (`crates/squeezy-llm/src/model_discovery.rs:308-318`) correctly defaults `vision: false`. No action.

### CB-PR-14 (low) — Service Tiers

2026-01-14 changelog introduced Service Tiers. Request-side `service_tier` is not yet a documented field on the chat-completions endpoint (org-level config). No squeezy action today; shared-core H3 forwarding of `service_tier` would be a no-op on Cerebras.

### CB-PR-15 (nit) — `PROVIDERS.md` performance number stale

`crates/squeezy-skills/external-docs/PROVIDERS.md:331-333` claims "Llama 3.1 70B at ~1800 tokens/sec". `llama3.1-70b` was retired **2025-01-17**; the current marquee number is `gpt-oss-120b` at ~3000 tok/sec.

### CB-PR-16 (nit) — `display_name` correct

`crates/squeezy-core/src/lib.rs:2033` = `"Cerebras"`. Matches the vendor's casing. No action.

## Catalog Verification (June 2026)

| Cerebras model id | Status | Tools | Reasoning | Vision | squeezy aware |
|---|---|---|---|---|---|
| `gpt-oss-120b` | production | ✓ + parallel | ✓ (`reasoning_effort`) | ✗ | no (missing from `models.json`) |
| `zai-glm-4.7` | preview | ✓ | ✓ (`reasoning_effort` + legacy `disable_reasoning` until 2026-07-21) | ✗ | no |
| `llama-3.3-70b` | **retired 2026-02-16** | (was ✓) | ✗ | ✗ | yes (set as default — CB-PR-1) |
| `qwen-3-32b` | **retired 2026-02-16** | ✗ | ✗ | ✗ | no |
| `llama3.1-8b` | **retired 2026-05-27** | ✗ | ✗ | ✗ | no |
| `qwen-3-235b-a22b-instruct-2507` | **retired 2026-05-27** | ✓ | ✗ | ✗ | no |
| `qwen-3-coder-480b` | **retired 2025-11-05** | ✓ | ✗ | ✗ | no |
| `qwen-3-235b-a22b-thinking-2507` | **retired 2025-11-14** | ✓ | ✓ | ✗ | no |
| `llama-4-scout-17b-16e-instruct` | **retired 2025-11-03** | ✓ | ✗ | ✗ | no |
| `llama-4-maverick-17b-128e-instruct` | **retired 2025-10-15** | ✓ | ✗ | ✗ | no |
| `deepseek-r1-distill-llama-70b` | **retired 2025-08-12** | ✓ | ✓ | ✗ | no |
| `zai-glm-4.6` | **retired 2026-01-20** | ✓ | ✓ | ✗ | no |

Dedicated-Endpoints tier (per 2026-04-27 changelog): GLM 5, GLM 5.1, Kimi K2.6 — not on public `api.cerebras.ai/v1`; reachable only via per-customer URLs and the `Custom` preset.

## Test Coverage

| Surface | Costly | Mock | `models.json` | Status |
|---|---|---|---|---|
| Cerebras preset | ✗ | ✗ | ✗ (0 entries) | **no coverage at all** |

No `crates/squeezy-llm/tests/cerebras_costly.rs` exists. No mock test. No `cerebras` entries in `crates/squeezy-llm/src/models.json` (`grep -c cerebras` → 0). Matches shared-core CB-4. Existing costly tests cover only: Anthropic, Azure OpenAI, Bedrock, DeepSeek, Google, Groq, OpenAI, OpenRouter, PortKey, Vercel, Vertex, xAI.

## Verification Strategy

1. **Default-model staleness lint**: CI check asserting every preset's hard-coded default appears in `models.json` (or is explicitly tagged "vendor-managed rolling default"). Catches CB-PR-1 next-rotation.
2. **Costly test** (`crates/squeezy-llm/tests/cerebras_costly.rs`): minimal smoke against `gpt-oss-120b` for (a) plain content stream, (b) reasoning with `reasoning_effort=low`, (c) tool call with `tool_choice={"type":"function","function":{"name":...}}`. Assert non-zero `usage.prompt_tokens` (verifies CB-PR-4 / shared-core C1) and that `state.reasoning_buf` populates from `delta.reasoning` (verifies CB-PR-3).
3. **Mock-server case in the parameterized harness** (shared-core §): Cerebras row with the post-`finish_reason` usage chunk + a `delta.reasoning` event. Asserts cost non-zero and reasoning event emitted.
4. **Catalog snapshot**: `squeezy doctor --provider cerebras` queries `/v1/models` and diffs against a baseline, warning when drift exceeds N entries.

## References

- Authentication / base URL: https://inference-docs.cerebras.ai/api-reference/authentication
- Chat completions reference: https://inference-docs.cerebras.ai/api-reference/chat-completions
- Model catalog: https://inference-docs.cerebras.ai/models/overview
- Reasoning capabilities: https://inference-docs.cerebras.ai/capabilities/reasoning
- Structured outputs / json_schema: https://inference-docs.cerebras.ai/capabilities/structured-outputs
- OpenAI compatibility statement: https://inference-docs.cerebras.ai/resources/openai
- Deprecation schedule: https://inference-docs.cerebras.ai/support/deprecation
- Rate limit headers: https://inference-docs.cerebras.ai/support/rate-limits
- Change log: https://inference-docs.cerebras.ai/support/change-log
- `gpt-oss-120b` model page: https://inference-docs.cerebras.ai/models/openai-oss
- Shared-core aggregator audit: `/Users/abbassabra/esqueezy/squeezy/.audit/providers/openai-compatible.md`
- opencode profile entry (peer impl): `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:8`
- opencode preset binding: `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/openai-compatible.ts:60`
- opencode CerebrasPlugin (richer per-model defaults): `/Users/abbassabra/esqueezy/others/opencode/packages/core/src/plugin/provider/cerebras.ts`
- opencode chat-completions parameterized test (Cerebras row): `/Users/abbassabra/esqueezy/others/opencode/packages/llm/test/provider/openai-compatible-chat.test.ts:45`
