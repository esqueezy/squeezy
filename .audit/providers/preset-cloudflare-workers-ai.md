# Cloudflare Workers AI Preset Audit

## Summary

- Severity tally: **0 critical / 2 high / 6 medium / 4 low / 2 nit** = **14 findings**.
- Top 3 actionable recommendations:
  1. **Alias `CLOUDFLARE_API_TOKEN`** alongside `CLOUDFLARE_API_KEY` (`crates/squeezy-core/src/lib.rs:2126`). Cloudflare's cURL + AI Gateway docs use `CLOUDFLARE_API_TOKEN`; users following those docs hit "api_key_env not set" with no hint.
  2. **Suppress the synthetic "reasoning-only stop" notice** when `finish_reason: "stop"` arrives with no `content` but tool calls *or* `<think>`-tagged reasoning are present. Workers AI's reasoning SKUs (`@cf/deepseek-ai/deepseek-r1-distill-qwen-32b`, `@cf/openai/gpt-oss-{120,20}b`, `@cf/moonshotai/kimi-k2.6`, `@cf/google/gemma-4-26b-a4b-it`) embed reasoning inline; `compatible.rs:1094-1108` fires a spurious tool-only notice referencing `tool_choice = "required"`.
  3. **Seed `models.json`** with at least the 8 active text-generation SKUs (CWAI-3); without it `capabilities_for` returns `vision: false` and rejects valid image attachments to vision-capable Kimi/Gemma/Mistral models (CWAI-7).

## Implementation Overview

Workers AI is 1 of 19 presets routed through `OpenAiCompatibleProvider` (`crates/squeezy-llm/src/compatible.rs:39-46`):

| Knob | Value | Source |
|---|---|---|
| Display name | `"Cloudflare Workers AI"` | `crates/squeezy-core/src/lib.rs:2039` |
| TOML key + aliases | `cloudflare_workers_ai`, `cloudflare_workersai`, `workers_ai`, `cf_workers_ai` | `lib.rs:2014`, `:2179-2181` |
| Default base URL | `https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1` | `lib.rs:122-123`, `:2091` |
| Default model | `@cf/meta/llama-3.3-70b-instruct-fp8-fast` | `lib.rs:127`, `:2152` |
| Default API-key env | `CLOUDFLARE_API_KEY` | `lib.rs:2126` |
| Extra headers | none | `compatible.rs:762-775` |
| `is_full_tier` | `false` | `lib.rs:2048-2059` |

`{account_id}` is substituted by `substitute_url_placeholders` (`compatible.rs:701-745`) at provider-build time. The config builder validates `account_id` (env `CLOUDFLARE_ACCOUNT_ID` or TOML `cloudflare_account_id`) up front (`lib.rs:8660-8672`); missing / whitespace values surface as a `Config` error rather than a literal `{account_id}` URL (`compatible_tests.rs:1031-1087`).

Request lifecycle: `from_config` (`compatible.rs:62-98`) → `Authorization: Bearer <key>` via `.bearer_auth(key)` (`compatible.rs:474`) → POST `<base>/chat/completions` (`compatible.rs:451`). Workers AI does not match any `COMPAT_TABLE` namespace (`compatible.rs:374-403`) — `@cf/` is not in the table — so the route falls through to `Generic` flavor: no `cache_control` markers (correct), `reasoning_effort` + `reasoning.effort` always emitted (CWAI-8), no tool-call gating.

Test coverage: `compatible_tests.rs:986-1029` covers URL substitution only. No mock or costly test; `models.json` has zero `cloudflare_workers_ai` entries.

## Verified-against-vendor-docs Table

Reference: Cloudflare Workers AI docs (May–Jun 2026, see Sources).

| Knob | Squeezy value | Vendor value | Verdict |
|---|---|---|---|
| Base URL | `…/accounts/{account_id}/ai/v1` | `…/accounts/{ACCOUNT_ID}/ai/v1` | ✓ |
| Chat path | `<base>/chat/completions` | `/v1/chat/completions` (and `/v1/responses` since Aug 2025) | ✓ |
| Auth scheme | `Authorization: Bearer <key>` | `Authorization: Bearer <api_token>` | ✓ |
| Env var | `CLOUDFLARE_API_KEY` | SDK uses `CLOUDFLARE_API_KEY`, cURL uses `CLOUDFLARE_API_TOKEN` | partial (CWAI-4) |
| Default model | `@cf/meta/llama-3.3-70b-instruct-fp8-fast` | active; survives May 30, 2026 deprecation; $0.29/$2.25 per M tok | ✓ |
| Model id wire | `model: "@cf/…"` JSON verbatim (`compatible.rs:207`) | `@` is intentional, never URL-encoded (model only goes in body) | ✓ |
| Streaming | `stream: true, stream_options: {include_usage: true}` (`compatible.rs:209-210`) | per May 2026 changelog: usage and `finish_reason` arrive in the *same* SSE chunk | ✓ but untested (CWAI-5) |
| Tool calling | passed through verbatim | supported on `@cf/meta/llama-3.3-70b-instruct-fp8-fast`, `@cf/openai/gpt-oss-{120,20}b`, `@cf/moonshotai/kimi-k2.6`, `@cf/google/gemma-4-26b-a4b-it`, `@cf/mistralai/mistral-small-3.1-24b-instruct`, `@cf/zai-org/glm-4.7-flash`, `@cf/nvidia/nemotron-3-120b-a12b`, `@cf/qwen/qwen3-30b-a3b-fp8` | ✓ |
| Vision | falls back to `vision: false` (no `models.json`) | supported on Kimi K2.6, Gemma 4, Mistral Small 3.1, Llama-3.2 11B Vision, Llama-4 Scout 17B | ✗ (CWAI-7) |
| Reasoning | `reasoning_effort` + `reasoning.effort` always emitted | `@cf/openai/gpt-oss-*` honors OpenAI shape; R1-distill + Kimi K2.6 use `<think>` tags inline (no `reasoning_content` field) | partial (CWAI-6, CWAI-8) |
| Error envelope | reads `error.message`, `error`, `value.message` (`compatible.rs:976-997`) | native shape is `{success: false, errors: [{code, message}]}`; leaks through on some failures | ✗ (CWAI-9) |

## Findings

### CWAI-1 — `@cf/` model id survives wire serialization (no issue, re-verified)

Per the original CWAI-1 in `.audit/providers/openai-compatible.md`. The model id flows through `json!({"model": &*request.model, ...})` at `compatible.rs:206-207`. `serde_json` does not escape `@` (only `"`, `\`, controls). The URL is `format!("{}/chat/completions", self.base_url)` (`compatible.rs:451`) — model never enters the path, so URL-encoding is moot. **No fix needed.**

### CWAI-2 — Workers AI may omit `usage` on legacy SKUs (medium, standing)

`.audit/providers/openai-compatible.md` CWAI-2. The May 2026 changelog ("Streaming responses now correctly report `finish_reason` only on the usage chunk") suggests usage is consistently emitted on modern text-gen SKUs. Older function-calling SKUs (`@hf/nousresearch/hermes-2-pro-mistral-7b`) may still omit it; `parse_chat_usage` (`compatible.rs:1138-1164`) silently returns zeros.

**Fix**: log `tracing::warn!` once per session on Workers AI streams that complete with no `usage`.

### CWAI-3 — `models.json` has zero Workers AI entries (low, standing)

`.audit/providers/openai-compatible.md` CWAI-3. Verified: `grep -c cloudflare_workers_ai crates/squeezy-llm/src/models.json` → 0. `capabilities_for` (`registry.rs:256-258`) → `fallback_model_info` → `vision: false`, `tool_use: false`, no context window. Token-budget guard-rails display "0% of context used".

**Fix**: seed minimum useful set (Jun 2026): `@cf/meta/llama-3.3-70b-instruct-fp8-fast` (function calling), `@cf/openai/gpt-oss-{120,20}b` (reasoning), `@cf/moonshotai/kimi-k2.6` (vision+reasoning+tools, 256k ctx), `@cf/google/gemma-4-26b-a4b-it` (vision+reasoning+tools, 256k ctx), `@cf/mistralai/mistral-small-3.1-24b-instruct` (vision+tools), `@cf/zai-org/glm-4.7-flash` (131k ctx, tools), `@cf/deepseek-ai/deepseek-r1-distill-qwen-32b` (reasoning).

### CWAI-4 — `CLOUDFLARE_API_TOKEN` env-var alias missing (high)

`lib.rs:2126` sets `CLOUDFLARE_API_KEY` as the env. Cloudflare's official cURL + AI Gateway docs use `CLOUDFLARE_API_TOKEN` ("Authorization: Bearer $CLOUDFLARE_API_TOKEN" — verified at `developers.cloudflare.com/ai-gateway/usage/providers/workersai/`). Only the JS SDK uses `CLOUDFLARE_API_KEY`. opencode's `cloudflare-workers-ai.ts:55` matches squeezy and shares the same blind spot. First-time setup foot-gun.

**Fix**: at `lib.rs:8608-8622`, add `.or_else(|| get_var("CLOUDFLARE_API_TOKEN"))` next to the account-id resolution. Or widen `default_api_key_env` to `&[&str]`.

### CWAI-5 — `finish_reason`+`usage` co-arrival is untested (high)

Per the May 2026 changelog, Cloudflare's wire shape is `data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{...}}\ndata: [DONE]`. `parse_chat_event` reads `usage` (line 1044-1046) BEFORE `finish_reason` (line 1075). The `stop` branch (`compatible.rs:1081-1109`) does NOT set `completed_emitted`; only `[DONE]` does (line 1014). So the shared-core C1 bug probably *doesn't* bite Cloudflare in the merged-chunk shape — but:

- The synthetic notice at `compatible.rs:1094-1108` fires on tool-only turns (`drain_tool_calls` emits the tool call, then the stop arm re-checks `state.saw_visible_output` which is still `false`).
- No fixture proves Cloudflare's merged shape works; a future regression to the split-chunk shape would silently lose cost.

**Fix**: (a) gate the synthetic notice on `state.tool_calls.is_empty()` *and* `!state.saw_visible_output`; (b) add a mock fixture replaying the merged chunk; (c) cross-reference `completed_emitted` and `finish_reason` at the post-loop boundary.

### CWAI-6 — R1-distill `<think>` tags surface as visible content (medium)

Cloudflare's OpenAI-compat path for `@cf/deepseek-ai/deepseek-r1-distill-qwen-32b` emits reasoning embedded in `content` with `<think>…</think>` tags (no `reasoning_content` field — verified May 2026). `parse_chat_event` reads `reasoning_content` and `reasoning` from the delta (`compatible.rs:1053-1054`) but never strips `<think>` from `content`, so the user sees raw thinking in the transcript. Native DeepSeek API exposes `reasoning_content` as a sibling and squeezy handles that correctly (shared DS-1); the Workers AI flavor of the same model does not.

**Fix**: add `tag_based_reasoning: true` flag in `CompatEntry` (or per-model lookup). When set, peel `<think>…</think>` from `content` and route to `ReasoningDelta`; emit `TextDelta` for the residual. Same applies to `@cf/moonshotai/kimi-k2.6`.

### CWAI-7 — Vision-capable Workers AI models rejected (medium)

`stream_response` calls `ensure_vision_support(self.preset.as_str())` (`compatible.rs:445-447`). `capabilities_for("cloudflare_workers_ai", model)` falls back to `vision: false` (CWAI-3). So a user attaching an image to `@cf/moonshotai/kimi-k2.6` (verified vision-capable) gets `provider does not support vision` — false negative. Same root cause as shared M11.

**Fix**: dovetail with CWAI-3 — seed `models.json` with `vision: true` on Kimi K2.6, Gemma 4 26B, Mistral Small 3.1, Llama-3.2 11B Vision, Llama-4 Scout 17B.

### CWAI-8 — `reasoning_effort` always emitted regardless of support (medium)

`compatible.rs:215-224` always emits both `reasoning_effort: "<level>"` and `reasoning: {effort: "<level>"}` when set. On Workers AI: `@cf/openai/gpt-oss-*` honors the OpenAI shape; R1-distill, Kimi K2.6, Gemma 4 all silently ignore both. User sets `reasoning_effort = "high"` expecting deeper thinking; gets unchanged output. Shared M6 root cause.

**Fix**: add `@cf/openai/gpt-oss-` (and similar) entries to `COMPAT_TABLE` with `supports_reasoning: true`; gate emission on `compat_entry(model).map_or(false, |e| e.supports_reasoning)`.

### CWAI-9 — Native Cloudflare `errors` envelope not parsed (medium)

`format_chat_error` (`compatible.rs:976-997`) reads `error.message` / `error` / `value.message`. Cloudflare's native shape is `{"success":false,"errors":[{"code":7000,"message":"No route"}],...}` — plural `errors` array, no singular `error` key. Account-suspended / billing / gateway upstream failures leak this shape through, and `format_chat_error` returns raw JSON as the message. Shared H6 specialized for Cloudflare.

**Fix**: extend the probe at `compatible.rs:982-983` to `value.errors[0].message` and prefix with the code.

### CWAI-10 — Generic 401 hint (low)

`compatible.rs:525-527` wraps the message in a preset-agnostic `ProviderRequest`. Cloudflare 401s have three common causes: (a) wrong env var name (CWAI-4); (b) token missing `Workers AI: Read` permission; (c) token belongs to a different account than the URL. PortKey gets a tailored hint (`compatible.rs:505-524`); Cloudflare doesn't.

**Fix**: add a Cloudflare 401 branch next to the PortKey hint: append `" — hint: ensure CLOUDFLARE_API_KEY (or CLOUDFLARE_API_TOKEN) is scoped Workers AI: Read for account <acct>."`.

### CWAI-11 — `account_id` not validated against `[A-Za-z0-9_-]+` (low)

Shared M8 specialized. `substitute_url_placeholders` does `String::replace` with no encoding. Whitespace is trimmed (`compatible.rs:718`) but embedded `/`, `?`, `#`, `%` slip through. Cloudflare account IDs are always 32-char hex, so this is non-hostile in practice — but a paste-error like `"abc?api-version=…"` becomes a query string.

**Fix**: validate `account_id` against `^[A-Za-z0-9_-]+$` in `lib.rs:8660-8672`.

### CWAI-12 — No costly or mock test for the live wire (low)

Construction-time URL substitution is covered (`compatible_tests.rs:986-1087`). No test for: streamed completion with non-zero usage; tool-only turn without spurious notice (CWAI-5); R1-distill `<think>` extraction (CWAI-6); vision attachment to Kimi K2.6 (CWAI-7); 401 hint shape (CWAI-10); native errors envelope (CWAI-9).

**Fix**: `crates/squeezy-llm/tests/cloudflare_workers_ai_mock.rs` paralleling `lmstudio_mock.rs`; optional costly variant gated on `CLOUDFLARE_API_KEY` + `CLOUDFLARE_ACCOUNT_ID`.

### CWAI-13 — Default model OK; AI Gateway shares it incorrectly (nit)

`DEFAULT_CLOUDFLARE_WORKERS_AI_MODEL = "@cf/meta/llama-3.3-70b-instruct-fp8-fast"` is correct (Jun 2026, on the surviving-models list). But `DEFAULT_CLOUDFLARE_AI_GATEWAY_MODEL` (`lib.rs:128`) shares the same string. AI Gateway can route to OpenAI / Anthropic / Groq upstreams where `@cf/meta/…` is a 400. Documentation gap (and arguably a shared-audit CFAG-x finding rather than CWAI).

**Fix**: when `CloudflareAiGateway` is selected without a model override, warn that the default targets Workers AI upstreams only.

### CWAI-14 — Display name correct (nit)

`"Cloudflare Workers AI"` (`lib.rs:2039`) matches official branding. No issue.

## Test Coverage Gaps

| Type | Present | Notes |
|---|---|---|
| Costly | ✗ | No `cloudflare_workers_ai_costly.rs` gated on `CLOUDFLARE_API_KEY` + `CLOUDFLARE_ACCOUNT_ID` |
| Mock | ✗ | No SSE-fixture replay for merged finish+usage (CWAI-5), `<think>` reasoning (CWAI-6), native errors envelope (CWAI-9) |
| Unit | partial | `compatible_tests.rs:986-1087` covers URL substitution; nothing for body shape or stream parse |
| `models.json` | ✗ | Zero entries; blocks `capabilities_for` (CWAI-3, CWAI-7) |

## Verification Strategy

`crates/squeezy-llm/tests/cloudflare_workers_ai_mock.rs` should cover:

1. **`finish_reason`+`usage` merged-chunk shape** (CWAI-5): SSE script `content delta → {delta:{}, finish_reason:"stop", usage:{...}} → [DONE]`. Assert `Completed { cost }` has non-zero tokens and no synthetic `[squeezy] model finished without emitting` text.
2. **Tool-only turn** (CWAI-5): tool_calls delta → `finish_reason:"tool_calls"` + usage → `[DONE]`. Assert tool call is emitted, cost captured, no synthetic notice.
3. **R1-distill `<think>` extraction** (CWAI-6): content delta `"<think>...</think>visible"` → expect `ReasoningDelta` for the tagged part and `TextDelta` for residual.
4. **Native errors envelope** (CWAI-9): mock 400 returning `{"success":false,"errors":[{"code":7000,"message":"No route"}]}` → assert error contains `"No route"` and `7000`, not raw JSON.
5. **`CLOUDFLARE_API_TOKEN` alias** (CWAI-4): set `CLOUDFLARE_API_TOKEN`, unset `CLOUDFLARE_API_KEY`, expect provider to build.
6. **401 hint** (CWAI-10): mock 401 → assert `"Workers AI: Read"` appears in error.

Costly smoke against `@cf/meta/llama-3.3-70b-instruct-fp8-fast`: (a) plain content, (b) tool calling, (c) usage reporting.

## References

- Workers AI OpenAI-compat: <https://developers.cloudflare.com/workers-ai/configuration/open-ai-compatibility/>
- Workers AI REST getting-started: <https://developers.cloudflare.com/workers-ai/get-started/rest-api/>
- Workers AI changelog: <https://developers.cloudflare.com/changelog/product/workers-ai/>
- Workers AI deprecations (May 30, 2026): <https://developers.cloudflare.com/changelog/post/2026-05-08-planned-model-deprecations/>
- Workers AI model catalog: <https://developers.cloudflare.com/workers-ai/models/>
- AI Gateway → Workers AI: <https://developers.cloudflare.com/ai-gateway/usage/providers/workersai/>
- `llms-full.txt`: <https://developers.cloudflare.com/workers-ai/llms-full.txt>
- Llama-3.3-70b-instruct-fp8-fast model page: <https://developers.cloudflare.com/workers-ai/models/llama-3.3-70b-instruct-fp8-fast/>
- DeepSeek-R1-distill-qwen-32b: <https://developers.cloudflare.com/workers-ai/models/deepseek-r1-distill-qwen-32b/>
- opencode peer: `/Users/abbassabra/esqueezy/others/opencode/packages/core/src/plugin/provider/cloudflare-workers-ai.ts`
- opencode peer tests: `/Users/abbassabra/esqueezy/others/opencode/packages/core/test/plugin/provider-cloudflare-workers-ai.test.ts`
- Shared aggregator audit: `/Users/abbassabra/esqueezy/squeezy/.audit/providers/openai-compatible.md` (CWAI-1/2/3 originally tracked there)
