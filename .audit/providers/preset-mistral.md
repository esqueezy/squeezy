# Mistral Preset Audit

## Summary

- Severity tally: **0 critical / 4 high / 7 medium / 2 low / 2 nit** = **15 findings** (Mistral-specific; shared aggregator findings tracked separately).
- The shared aggregator audit (see `/Users/abbassabra/esqueezy/squeezy/.audit/providers/openai-compatible.md`) found the bulk of the wire-shape bugs that hit Mistral universally (C1, H3, M3, M6, MS-1..MS-4). This file enumerates the Mistral-specific evidence and corrects two stale claims from the shared file that 2026-vintage Mistral docs invalidate.
- Top three actionable recommendations:
  1. **Stop emitting `prompt_cache_retention: "24h"` to Mistral** (`compatible.rs:245`). Mistral's chat-completions schema rejects unknown top-level fields with `extra_forbidden` 422 errors, and `prompt_cache_retention` is not in the schema as of June 2026. Any `[providers.mistral]` config that asks for long cache retention via the shared cache policy hard-fails.
  2. **Refresh the default model id** and add at least one curated entry to `models.json`. Today `DEFAULT_MISTRAL_MODEL = "mistral-large-latest"` (`lib.rs:99`) plus zero `models.json` entries means every Mistral session falls through `fallback_model_info` (`registry.rs:161-181`) with `vision: false`, `pricing: None`, and a 272k context-window guess that doesn't match any current Mistral SKU. Pixtral / `mistral-large-3` users get a hard `does not support image inputs` error even though the underlying model is multimodal.
  3. **Correct the shared audit's MS-1 claim**. Mistral *does* accept `tool_choice = "required"` as of June 2026 (verified in the OpenAPI schema). Squeezy's existing pass-through at `compatible.rs:292-294` works correctly; the per-vendor mapping that MS-1 recommended is unnecessary. The shared audit text should be updated to note that `"required"` and `"any"` are both accepted (they are documented synonyms today).

## Implementation Overview

Mistral routes through `OpenAiCompatibleProvider` (`crates/squeezy-llm/src/compatible.rs:39-46`). Preset metadata: enum at `lib.rs:1973`; `as_str() = "mistral"` (`lib.rs:2005`); `display_name() = "Mistral La Plateforme"` (`lib.rs:2030`); `default_base_url() = "https://api.mistral.ai/v1"` (`lib.rs:98, 2074`); `default_api_key_env() = "MISTRAL_API_KEY"` (`lib.rs:2110`); `default_model() = "mistral-large-latest"` (`lib.rs:99, 2141`); `is_full_tier() = false` (`lib.rs:2048-2058`); `parse()` accepts `mistral` / `mistral_ai` (`lib.rs:2170`); section-name alias `["mistral"]` only (`lib.rs:8745`).

Auth is `Authorization: Bearer ${MISTRAL_API_KEY}` via `bearer_auth(key)` at `compatible.rs:474`. No preset-specific extra headers; `preset_default_headers` (`compatible.rs:762-775`) returns the OpenRouter pair only.

Request shape: `request_body` (`compatible.rs:134-297`) emits `stream: true, stream_options: { include_usage: true }`, `max_tokens`, both `reasoning_effort` and `reasoning: { effort }`, `prompt_cache_key`, optional `prompt_cache_retention`, `tools`, `tool_choice` (verbatim string). No per-vendor branching on the `Mistral` variant — rides the generic path.

## Verified Wire Facts (June 2026)

| Aspect | Value | Source |
|---|---|---|
| Base URL | `https://api.mistral.ai/v1` | docs.mistral.ai/api |
| Auth | `Authorization: Bearer <MISTRAL_API_KEY>` | docs.mistral.ai/api |
| `tool_choice` accepted | `"auto"`, `"none"`, `"any"`, `"required"`, `{ "type": "function", "function": { "name": "…" } }` | docs.mistral.ai/api (Chat schema, June 2026) |
| `response_format` types | `"text"`, `"json_object"`, `"json_schema"` | docs.mistral.ai/api |
| Reasoning surface | `reasoning_effort: "none" \| "high"` AND `prompt_mode: "reasoning"` (NATIVE top-level fields now) | docs.mistral.ai/api |
| Unknown-field handling | HTTP 422 `{ "object": "error", "message": { "detail": [{ "type": "extra_forbidden", "loc": [...], "msg": "Extra inputs are not permitted", "input": ... }] }, "type": "invalid_request_error", "raw_status_code": 422 }` | open-webui/open-webui#10167 verified shape |
| Accepted top-level body fields | `frequency_penalty`, `guardrails`, `max_tokens`, `messages`, `metadata`, `model`, `n`, `parallel_tool_calls`, `prediction`, `presence_penalty`, `prompt_cache_key`, `prompt_mode`, `random_seed`, `reasoning_effort`, `response_format`, `safe_prompt`, `stop`, `stream`, `temperature`, `tool_choice`, `tools`, `top_p` | docs.mistral.ai/api |
| NOT in schema (will 422) | `prompt_cache_retention`, `reasoning` (top-level object), `stream_options`, `max_completion_tokens` | docs.mistral.ai/api + open-webui#10167 |
| Vision shape | `content: [{ "type": "text", "text": "…" }, { "type": "image_url", "image_url": "data:image/jpeg;base64,…" \| "https://…" }]` | docs.mistral.ai/capabilities/vision |
| Vision models | `mistral-large-2512` (Large 3), `mistral-medium-2508` (Med 3.1), `mistral-small-2506` (Small 3.2), Ministral 3 (14B/8B/3B), Pixtral Large | docs.mistral.ai/capabilities/vision |
| Codestral FIM | `POST /v1/fim/completions` with `codestral-2404` / `codestral-2405` / `codestral-2508`; NOT addressable via chat-completions | docs.mistral.ai/api/endpoint/fim |
| Prefix caching hint | `x-affinity: <session-id>` header re-uses KV cache for the same session prefix | peer `pi/packages/ai/src/providers/mistral.ts:228-232` |
| `usage` cache fields | Not in published schema; no `cached_tokens` / `cache_creation_input_tokens` | docs.mistral.ai/api |

## Shared-Audit Cross-References

- **C1** (lost usage after `finish_reason`): applies — Mistral's SSE follows OpenAI shape with terminal usage chunk; cost reports zero.
- **H3** (no temperature/seed/top_p forwarding): applies. Note Mistral uses `random_seed` not `seed` — H3 fix needs per-preset name projection (see MIS-5).
- **M3** (no `response_format`/`output_schema`): applies. Mistral natively supports `json_schema`; silently dropped.
- **M6**: see MIS-2/MIS-3 — more nuanced for Mistral.
- **MS-1**: **STALE**. `"required"` IS accepted by current Mistral schema. See MIS-6.
- **MS-2**: still valid generally; specific examples re-classified — `reasoning_effort` now in schema, `reasoning` object is not. See MIS-2.
- **MS-3** (no `models.json` entries): confirmed (verified grep → 0).
- **MS-4** / **N1** (display-name): see MIS-13.
- **M11** (`ensure_vision_support` registry-only): hits Mistral hard — see MIS-10.

## Mistral-Specific Findings

### MIS-1 — Default model id is a deprecated alias (high)

`crates/squeezy-core/src/lib.rs:99` defines `DEFAULT_MISTRAL_MODEL = "mistral-large-latest"`. As of June 2026:

- Mistral's models-overview page no longer documents `-latest` aliases; the canonical id is `mistral-large-3` (`mistral-large-2512`).
- The `-latest` alias still resolves (verified via the changelog) but its target is changing: it currently points to `mistral-large-2412` on some accounts and `mistral-large-2512` on others, depending on workspace tier. Users get inconsistent behavior between dev and production.
- The shared aggregator audit equivalent (DS-2 for DeepSeek) was filed when the alias rotation cycle changed mid-2026; Mistral has the same pattern.

**Fix**: pin the default to `mistral-large-2512` (or `mistral-medium-2505` for cost-sensitive default), and document in the TOML schema that users override via `[providers.mistral] model = "…"`. Optionally add `mistral_aliases` table to keep `mistral-large-latest` resolving for legacy configs.

### MIS-2 — `reasoning: { effort }` sent alongside `reasoning_effort` 422s on Mistral (high)

`compatible.rs:215-224` emits both shapes when `request.reasoning_effort` is `Some(…)`. Mistral's schema includes top-level `reasoning_effort` (enum `"none" | "high"`) but NOT a `reasoning` object — 422 `extra_forbidden` on `["body", "reasoning"]`. Any Mistral session with `reasoning_effort` set in `[model]` config 422s every request.

Second concern: Mistral's `reasoning_effort` enum is `"none" | "high"` only. `LlmReasoningEffort.as_str()` at `compatible.rs:221` will emit `"low"` / `"medium"` / `"minimal"`, none of which Mistral accepts — 422 with `enum_violation`.

**Fix**: gate the `reasoning` object emission on `flavor != Generic` (today's `COMPAT_TABLE` at `compatible.rs:374-403` doesn't match bare Mistral ids, so it falls through to Generic and the object gets emitted regardless). Either omit the object universally for non-OpenAI flavors, or add a `supports_reasoning_object` flag. Also project the effort string through a per-vendor map that clamps Mistral to `"high"` / `"none"`.

### MIS-3 — `prompt_cache_retention: "24h"` triggers 422 on Mistral (high)

`compatible.rs:238-246` emits `prompt_cache_retention` when `cache_retention == CacheRetention::Long`. Mistral's schema does not include this field — 422 `extra_forbidden` on `["body", "prompt_cache_retention"]`. The in-file justification ("non-OpenAI flavors ignore unknown fields") is empirically false for Mistral (and Cerebras per CB-2).

**Fix**: gate emission on `compat_entry(model).map(|e| e.flavor) == Some(CompatFlavor::OpenAi)` plus a new `supports_prompt_cache_retention` flag. Pair with MIS-2 — same anti-pattern.

### MIS-4 — `prompt_cache_key` body field misses Mistral's `x-affinity` header convention (medium)

`compatible.rs:225-237` emits `prompt_cache_key` body field. Mistral accepts it (schema-verified) but per `pi/packages/ai/src/providers/mistral.ts:228-232` the canonical KV-cache reuse mechanism on La Plateforme is the `x-affinity: <session-id>` HEADER. Squeezy's body-only emission likely produces zero cache hits.

**Fix**: when preset is `Mistral`, route `prompt_cache_key` to an `x-affinity` header (keep emitting body field for forward-compat).

### MIS-5 — `seed` → `random_seed` field-rename gap (medium)

Once shared-audit H3 lands and adds `seed` to `LlmRequest`, Mistral will 422 because the schema-accepted name is `random_seed`. Pre-track to avoid relitigating in H3 follow-up.

**Fix**: per-preset `body_field_aliases: BTreeMap<&'static str, &'static str>`, or open-code the rename when preset is `Mistral`.

### MIS-6 — Shared-audit MS-1 is stale; `tool_choice = "required"` works (medium → resolved-by-vendor)

Shared MS-1 claims Mistral rejects `tool_choice = "required"`. Re-verified June 2026 OpenAPI schema: `tool_choice` accepts `ToolChoice | "auto" | "none" | "any" | "required"`. The capabilities guide page (`docs.mistral.ai/capabilities/function_calling`) is out of date; the API schema is authoritative.

Squeezy's pass-through (`compatible.rs:292-294`) is correct. The `[squeezy] model finished without emitting any content…` notice at `compatible.rs:1107` recommending `tool_choice = "required"` is appropriate for Mistral too.

**Fix**: amend `openai-compatible.md` to mark MS-1 resolved. No squeezy code change.

### MIS-7 — No Codestral FIM support (medium)

Mistral's Codestral has a dedicated `POST /v1/fim/completions` with a different request shape (`prompt` + `suffix`). Squeezy hard-codes `/chat/completions` at `compatible.rs:451`. Codestral works for instruction-style use via chat-completions; FIM (the canonical IDE use case) is unaddressable.

**Fix**: out of scope for chat-completions; document, consider a separate `MistralFimProvider`.

### MIS-8 — Mistral tool-call ids must match `^[a-zA-Z0-9]{9}$` (medium)

Per `pi/packages/ai/src/providers/mistral.ts:32, 154-184`, Mistral rejects tool-call ids that aren't 9-char alphanumeric. Squeezy's `normalize_tool_ids_for_replay` (`lib.rs:360`) emits `call_<N>` — 6-7 chars with underscore — which Mistral rejects when replaying historical tool calls (mid-conversation model switches).

**Fix**: per-preset id sanitizer projecting canonical ids through `shortHash(id).slice(0, 9)` for Mistral.

### MIS-9 — `stream_options.include_usage` may 422 on Mistral (medium)

Schema does not list `stream_options`. `compatible.rs:210` emits unconditionally. Mistral may silently strip nested objects (verified `extra_forbidden` only seen on scalar/string fields per open-webui#10167) or 422 — unverified. SDK example does include usage in `event.data.usage` (`pi:308-315`), suggesting silent-strip. Live verification needed.

### MIS-10 — Vision permanently denied for vision-capable Mistral models (high)

`lib.rs:344-357` (`ensure_vision_support`) → `capabilities_for("mistral", model)` → `fallback_model_info` (`registry.rs:161-181`) → `capabilities = TEXT_TOOLS` (vision: false). Every image-bearing Mistral prompt hard-fails with `model X on provider mistral does not support image inputs`, even though `mistral-large-2512`, `mistral-medium-2508`, `mistral-small-2506`, Ministral 3 (3B/8B/14B), and Pixtral Large all support vision. M11 in shared audit; severe for Mistral.

**Fix**: add a curated `mistral` block to `models.json` with vision flags for the SKUs above; pair with MIS-1.

### MIS-11 — `format_chat_error` does not parse Mistral's `{ object: "error", message: { detail: [...] } }` (medium)

Mistral 422 envelope (verified open-webui#10167):

```json
{ "object": "error", "message": { "detail": [{ "type": "extra_forbidden", "loc": ["body","X"], "msg": "Extra inputs are not permitted" }] }, "type": "invalid_request_error", "raw_status_code": 422 }
```

`error` wrapper is absent; `message` is an object, not a string. `format_chat_error` (`compatible.rs:976-998`) returns `default_message` (raw body) — verbose, not actionable. Shared H6 captures pattern; Mistral shape needs explicit handling.

**Fix**: detect `object == "error"` at top level and synthesize `"Mistral 422 invalid_request_error: Extra inputs are not permitted at body.<field>"`. Surface `type` for retry classifier.

### MIS-12 — `usage` does not surface KV-cache reuse (low)

Mistral's `x-affinity` reuse is invisible — no `cached_tokens` documented. `parse_chat_usage` (`compatible.rs:1138-1164`) has no fallback. Accounting gap; track when Mistral publishes the field.

### MIS-13 — Display-name nit (nit)

`lib.rs:2030`: "Mistral La Plateforme". Same as shared N1.

### MIS-14 — Settings-key alias is `["mistral"]` only (nit)

`lib.rs:8745`: `&["mistral"]`. `parse()` accepts `mistral_ai` (`lib.rs:2170`) but section-name resolver does not — `[providers.mistral_ai]` errors. Extend alias to `&["mistral", "mistral_ai"]`.

### MIS-15 — `MISTRAL_BASE_URL` env override has no host validation (low)

`lib.rs:8623` accepts `MISTRAL_BASE_URL` overrides. Pair with shared M5 (SSRF surface) — consider host-allowlist when M5 lands.

## Test Coverage Gaps

| Layer | Coverage | Gap |
|---|---|---|
| `compatible_tests.rs` | None (no `mistral` substring in file) | No mock test exercises the Mistral preset's wire shape, error envelope, or tool-choice forwarding |
| `compatible_tests.rs` (display name) | None (verified via grep — zero matches) | No assertion that `display_name() == "Mistral La Plateforme"` |
| `models.json` | Empty (verified via grep) | No curated models, no pricing, no vision flag, no token-window data |
| `tests/mistral_costly.rs` | Does not exist (verified via `ls tests/`) | No live API check of `tool_choice = "required"`, no 422 error envelope assertion, no `reasoning_effort` confirmation |
| `tests/mistral_mock.rs` | Does not exist | No SSE-stream replay test for Mistral's terminal-usage-chunk pattern (C1) |

## Verification Strategy

1. **`tests/mistral_costly.rs`** mirroring `deepseek_costly.rs`: run against `mistral-large-2512`; assert stream completes, usage non-zero, returned tool-call id matches `^[a-zA-Z0-9]{9}$`, error envelope parses on a deliberately-bad request.
2. **Mock-server unit test** that asserts `request_body(…)` for an `LlmRequest` with `reasoning_effort = High` and `cache_retention = Long` does NOT emit `reasoning` (MIS-2), `prompt_cache_retention` (MIS-3), or non-`high`/`none` effort strings.
3. **Schema-drift allowlist test**: in-tree set of accepted Mistral top-level body fields; `request_body` output must be a subset.
4. **Pixtral / mistral-large-2512 vision smoke test** once `models.json` is populated, to confirm MIS-10 fix.

## References

- Mistral Chat Completion endpoint (June 2026): https://docs.mistral.ai/api/#tag/chat
- Mistral models overview (June 2026): https://docs.mistral.ai/getting-started/models/models_overview/
- Mistral FIM endpoint: https://docs.mistral.ai/api/endpoint/fim
- Mistral vision capability: https://docs.mistral.ai/capabilities/vision/
- Mistral function-calling guide (stale on `tool_choice`): https://docs.mistral.ai/capabilities/function_calling/
- Mistral changelog (alias rotation context): https://docs.mistral.ai/getting-started/changelog
- Verified 422 envelope shape: https://github.com/open-webui/open-webui/issues/10167
- Verified `extra_forbidden` validator behavior: https://github.com/openclaw/openclaw/issues/47079
- Peer Mistral provider (official SDK wrapper, June 2026 vintage): `/Users/abbassabra/esqueezy/others/pi/packages/ai/src/providers/mistral.ts`
- Peer Mistral reasoning-mode test: `/Users/abbassabra/esqueezy/others/pi/packages/ai/test/mistral-reasoning-mode.test.ts`
- Peer Mistral tool-schema test: `/Users/abbassabra/esqueezy/others/pi/packages/ai/test/mistral-tool-schema.test.ts`
- Shared aggregator audit (this preset's parent): `/Users/abbassabra/esqueezy/squeezy/.audit/providers/openai-compatible.md`
