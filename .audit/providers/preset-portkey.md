# PortKey Preset Audit

## Summary

- Severity tally: **0 critical / 3 high / 6 medium / 5 low / 3 nit** = **17 findings**.
- The shared OpenAI-compatible code path delivers a *working* PortKey request (the user-verified baseline), but every preset-specific affordance — auth header, default model, routing-header allow-list, error hint, `models.json`, multi-key fallback — is partially or completely wrong against PortKey's 2026 docs.
- Top 3 actionable recommendations:
  1. **Send `x-portkey-api-key`, not `Authorization: Bearer`, for the PortKey key.** PortKey reserves `Authorization` for the *upstream provider's* credential — squeezy squats it. Today's scheme works only because PortKey accepts Bearer as a legacy alias; PortKey's own Claude Code integration uses `x-portkey-api-key` (P1 / PK-1).
  2. **Fix the routing-header allow-list and the error hint.** `portkey_routing_header_present` (`compatible.rs:747-760`) recognises 3 of the ~12 PortKey-prefixed headers shipped through May 2026 (P4). The hint at `compatible.rs:515-519` invents an `@open-ai/` slug literal that doesn't match PortKey's actual user-defined slug convention (P5 / PK-2).
  3. **Populate `models.json` or drop PortKey from `is_full_tier`.** `is_full_tier()` returns `true` (`lib.rs:2053`) yet `grep "portkey" models.json` is 0 (P8 / PK-3).

## Implementation Overview

PortKey routes entirely through `OpenAiCompatibleProvider`. There is no dedicated PortKey file.

| Concern | Location | Value |
|---|---|---|
| Enum variant | `crates/squeezy-core/src/lib.rs:1968` | `OpenAiCompatiblePreset::PortKey` |
| Display name | `crates/squeezy-core/src/lib.rs:2025` | `"PortKey"` |
| `is_full_tier()` | `crates/squeezy-core/src/lib.rs:2053` | `true` |
| Default base URL | `crates/squeezy-core/src/lib.rs:83, 2065` | `https://api.portkey.ai/v1` |
| Default api-key env | `crates/squeezy-core/src/lib.rs:2101` | `PORTKEY_API_KEY` |
| Default model | `crates/squeezy-core/src/lib.rs:84, 2136` | `anthropic/claude-opus-4-7` |
| Aliases | `crates/squeezy-core/src/lib.rs:2165` | `portkey`, `port_key` |
| Provider settings key | `crates/squeezy-core/src/lib.rs:8740` | `[providers.portkey]` |
| Small-fast model | `crates/squeezy-core/src/lib.rs:57, 73` | `anthropic/claude-haiku-4-5` |
| CLI auth row | `crates/squeezy-cli/src/auth.rs:63-68` | `SQUEEZY_PORTKEY_KEY` → `PORTKEY_API_KEY` |
| Routing-header guard | `crates/squeezy-llm/src/compatible.rs:747-760` | 3 headers |
| 400 `x-portkey-` hint | `crates/squeezy-llm/src/compatible.rs:505-524` | preset+status guarded |
| Costly test | `crates/squeezy-llm/tests/portkey_costly.rs:11-56` | virtual-key path only |
| Unit test | `crates/squeezy-llm/src/compatible_tests.rs:553-564` | guard helper only |
| `models.json` entries | `crates/squeezy-llm/src/models.json` | **0** |

Wire shape per request: `POST https://api.portkey.ai/v1/chat/completions`, `Authorization: Bearer <PORTKEY_API_KEY>` (`compatible.rs:474`), plus user-supplied `providers.portkey.headers` merged at `compatible.rs:85-90`. No PortKey-specific body shaping. The historical `x-portkey-provider` auto-injection was deliberately removed (comment, `compatible.rs:454-463`) because guessing the upstream from a `vendor/model` prefix mis-routed Model-Catalog accounts. PortKey is the only preset that gets a custom error-message hint (`compatible.rs:505-524`), keyed on `status == 400 && message.contains("x-portkey")`, with a `portkey_routing_configured` branch that suppresses half the hint when the user has set a routing header.

## Findings

### [HIGH] P1 — `Authorization: Bearer <PORTKEY_KEY>` squats on PortKey's upstream slot (refs PK-1)

- **Location**: `crates/squeezy-llm/src/compatible.rs:474` (`bearer_auth(key)`); env `lib.rs:2101`.
- **Observed**: squeezy sends `Authorization: Bearer <PORTKEY_API_KEY>`. PortKey 2026 docs make `x-portkey-api-key` canonical for PortKey's own auth; the `Authorization: Bearer` slot is reserved for the **upstream** provider's credential, which PortKey rewrites into the upstream-native auth header. PortKey's own Claude Code integration sets `ANTHROPIC_CUSTOM_HEADERS: x-portkey-api-key: <PK>\nx-portkey-provider: @anthropic-prod`. opencode's PortKey example (`others/pi/.../models.md:170-185`) does the same via a `custom-proxy`.
- **Issue**: today's scheme works because PortKey accepts Bearer as a legacy alias. It blocks the BYO-upstream-key mode (`Authorization: Bearer <OPENAI_KEY>` + `x-portkey-api-key: <PK>` + `x-portkey-provider: openai`); users wanting that mode must override `Authorization` via `extra_headers`, which BTreeMap merges into a duplicated header (P12).
- **Fix sketch**: inject `x-portkey-api-key: <resolved_key>` for the PortKey arm at `compatible.rs:85-90`, drop the `bearer_auth` call for PortKey (gate `compatible.rs:474`), and document a `providers.portkey.upstream_api_key_env` knob. Mirrors the Cloudflare AI Gateway dual-auth fix (openai-compatible C2).

### [HIGH] P2 — Default model `anthropic/claude-opus-4-7` may 400 on Model-Catalog accounts

- **Location**: `crates/squeezy-core/src/lib.rs:84` (`DEFAULT_PORTKEY_MODEL`), `:57` (small-fast).
- **Observed**: PortKey 2026 routes by (a) `@<integration-slug>/<model>` model id (e.g. `@anthropic-prod/claude-sonnet-4-5-20250929`), (b) `x-portkey-virtual-key` + bare id, (c) `x-portkey-config: pc-***` + config-pinned model, or (d) `x-portkey-provider: <slug>` + bare id. The bare `anthropic/claude-opus-4-7` matches none — PortKey returns the 400 that the hint at `compatible.rs:515-519` was written for.
- **Issue**: first-touch UX with no other config produces an immediate 400. Fallback `PORTKEY_SMALL_FAST_MODEL` has the same defect.
- **Fix sketch**: set the default to `@<your-integration-slug>/claude-opus-4-7` (placeholder that the validator rejects until substituted), OR detect "default model + no routing header + no `@` prefix" pre-flight and emit a setup error. opencode sidesteps this by not shipping PortKey as a built-in profile (`others/opencode/.../openai-compatible-profile.ts:6-16`).

### [HIGH] P3 — Shared chat-completions body omits `seed`, `temperature`, `output_schema`

- **Location**: `crates/squeezy-llm/src/compatible.rs:206-297`.
- **Inherits**: openai-compatible H3 / M2 / M3.
- **PortKey-specific impact**: PortKey forwards unknown body fields verbatim to upstream; users routing OpenAI/Anthropic through PortKey lose JSON-mode contracts and determinism knobs they could otherwise set. Fix lives in the shared body builder.

### [MEDIUM] P4 — Routing-header allow-list is incomplete (refs PK-2)

- **Location**: `crates/squeezy-llm/src/compatible.rs:747-760`.
- **Observed**: recognises `x-portkey-provider`, `x-portkey-virtual-key`, `x-portkey-config`. PortKey 2026 also ships: `x-portkey-custom-host` (overrides upstream URL — a routing signal); behaviour modifiers `x-portkey-cache-namespace`, `x-portkey-cache-force-refresh`, `x-portkey-request-timeout`, `x-portkey-metadata`, `x-portkey-trace-id`, `x-portkey-span-id`, `x-portkey-parent-span-id`, `x-portkey-span-name`, `x-portkey-forward-headers`; plus v2.9.0 (2026-05-21) `x-portkey-sensitive-headers` and v2.8.0 (2026-05-14) `x-portkey-azure-entra-scope`. (The audit's earlier reference to `x-portkey-router` is not borne out by the changelog — it does not exist.)
- **Issue**: a user who configures only `x-portkey-custom-host` (a valid routing scheme for self-hosted upstreams) still gets the hint that recommends setting `x-portkey-config / x-portkey-virtual-key / x-portkey-provider`. The allow-list also doesn't recognise the `@<slug>/<model>` id form, which is itself a routing signal.
- **Fix sketch**: add `x-portkey-custom-host` to the const array; add a separate check `request.model.starts_with('@') ⇒ routing-configured`; drop the "headers" half of the hint when the model id already carries `@<slug>/`.

### [MEDIUM] P5 — Error-hint slug example is fabricated (refs PK-2)

- **Location**: `crates/squeezy-llm/src/compatible.rs:515-519`.
- **Observed**: hint text recommends `@open-ai/gpt-4o-mini` and `@openrouter/<vendor>/<model>`. PortKey slugs are *user-chosen* at integration-creation; community examples are `@openai-prod`, `@anthropic-prod`, `@bedrock-prod`. `@open-ai/` is not what PortKey returns from the integrations console, and `@openrouter/` only works if the user named their OpenRouter integration that.
- **Issue**: a user typing `@open-ai/gpt-4o-mini` literally gets a second 400 about the slug not existing, sending them in circles.
- **Fix sketch**: rewrite to `@<your-integration-slug>/<model-id>` and link to `portkey.ai/integrations` console rather than the (different-purpose) `GET /v1/models` endpoint also suggested by the hint.

### [MEDIUM] P6 — Hint suggests `GET /v1/models` to enumerate slugs; wrong endpoint

- **Location**: `crates/squeezy-llm/src/compatible.rs:516-518`.
- **Observed**: hint says `GET https://api.portkey.ai/v1/models` "to see what's available". That endpoint returns the global model catalog PortKey knows, not the user's per-workspace integration slugs. The correct surface for slugs is the `portkey.ai/integrations` console or the Model Catalog API.
- **Fix sketch**: swap the URL in the hint, or remove the suggestion and link the Model Catalog docs.

### [MEDIUM] P7 — Multi-key fallback / config-as-JSON path is undocumented (refs PK-4)

- **Location**: `crates/squeezy-llm/src/compatible.rs:75-90` (header merge); no docs.
- **Observed**: `x-portkey-config` accepts either `pc-***` ids or inline JSON `{"strategy": {"mode": "fallback"}, "targets": [...]}`. Squeezy supports this only by accident — users dump the JSON into `providers.portkey.headers["x-portkey-config"]`. No validation, no typed knob, no test (`portkey_costly.rs` covers virtual-key only).
- **Fix sketch**: add typed `providers.portkey.config_id` and `providers.portkey.fallback_targets` knobs that synthesise `x-portkey-config`; keep raw-header injection as the escape hatch.

### [MEDIUM] P8 — `is_full_tier()` returns true but `models.json` has 0 PortKey entries (refs PK-3, openai-compatible N4)

- **Location**: `lib.rs:2048-2059`; `crates/squeezy-llm/src/models.json` (`grep -c "portkey"` → 0).
- **Observed**: `is_full_tier`'s docstring promises "curated models exist in the registry and a dedicated costly integration test ships". The test ships (`portkey_costly.rs`); the curated models do not.
- **Impact**: model picker special-cases PortKey to show a placeholder (`main.rs:2061-2073`). `capabilities_for("portkey", model)` always falls through to `fallback_model_info` (`registry.rs:250-258`) — so vision/reasoning/pricing are never authoritative (cascades into P9).
- **Fix sketch**: seed entries using `@<slug>/<model>` shape for the top 5–10 common slugs (Anthropic, OpenAI, Bedrock, Vertex, OpenRouter via PortKey), OR drop PortKey from `is_full_tier` and accept the "user supplies own slugs" reality.

### [MEDIUM] P9 — Vision capability lookup always false on PortKey (refs openai-compatible M11)

- **Location**: `crates/squeezy-llm/src/compatible.rs:445-447`; `registry.rs:256-258`.
- **Observed**: `ensure_vision_support` keys off `("portkey", model_id)`; with 0 entries, falls back to `vision = false`. Verified May 2026 that PortKey relays both URL and base64 data-URL `image_url` parts to upstream verbatim.
- **Impact**: a user attaching an image to a PortKey-routed Anthropic/OpenAI/Gemini model hits a squeezy-side `provider does not support vision` error before the call leaves the process.
- **Fix sketch**: when preset is PortKey, also consult `compat_entry(model).flavor` so an `@anthropic-prod/claude-opus-4-7` id inherits Anthropic's vision flag.

### [LOW] P10 — `reasoning_effort` + `reasoning.effort` may 422 on PortKey-fronted strict providers

- **Location**: `compatible.rs:215-224`.
- **Inherits**: openai-compatible M6. PortKey forwards unknown body fields; Mistral and certain Cerebras SKUs 422 on them. Gate emission on `compat_entry(model).supports_reasoning`.

### [LOW] P11 — Inline mid-stream PortKey error envelope not retried

- **Location**: `compatible.rs:1022-1027`; `retry.rs:46-55`.
- **Observed**: PortKey is documented to stream upstream errors mid-flight as inline SSE events (notably Anthropic's overloaded errors when "Catch Overloaded Error on Stream" is *not* enabled on the integration). Squeezy classifies as non-retryable `ProviderStream`.
- **Inherits**: openai-compatible H7 fix.

### [LOW] P12 — User-supplied `Authorization` header duplicates rather than replaces the Bearer

- **Location**: `crates/squeezy-llm/src/compatible.rs:474-479`.
- **Observed**: `bearer_auth(key)` then a loop appending `extra_headers` via `RequestBuilder::header`. Reqwest's `header` appends — so `Authorization` shows up twice on the wire when the user tries the BYO-key escape from P1.
- **Fix sketch**: normalise via `HeaderMap::insert` before sending. Same as openai-compatible M1.

### [LOW] P13 — Costly test only exercises virtual-key routing

- **Location**: `crates/squeezy-llm/tests/portkey_costly.rs:11-56`.
- **Observed**: requires `PORTKEY_VIRTUAL_KEY` (`:14, 22`), skips otherwise. Doesn't exercise `x-portkey-config`, `x-portkey-provider`, or `@<slug>/<model>` id routing.

### [LOW] P14 — Response `x-portkey-trace-id` is never surfaced

- **Location**: `crates/squeezy-llm/src/compatible.rs:483-487` (raw body read), `1138-1164` (`parse_chat_usage`).
- **Observed**: PortKey echoes a trace id on every response that cross-references back to its console. Squeezy reads neither it nor `x-portkey-cost-cents`. Users can't link a turn back to the PortKey log without hand-correlating timestamps.
- **Fix sketch**: read response headers in `stream_response` and stash on `StreamState` for inclusion in the terminal `Completed` event.

### [NIT] P15 — `parse(value)` misses `pk` and `portkey_ai` aliases

- **Location**: `crates/squeezy-core/src/lib.rs:2165`. Currently accepts `portkey` and `port_key` only.

### [NIT] P16 — Display name capitalisation mismatches vendor branding

- **Location**: `crates/squeezy-core/src/lib.rs:2025` (`"PortKey"`). Vendor uses "Portkey" everywhere (`docs.portkey.ai`, `Portkey-AI/cli`, the v2.9.0 changelog). Cosmetic.

### [NIT] P17 — Comment references "integration-style PortKey accounts" — undefined term

- **Location**: `crates/squeezy-llm/src/compatible.rs:497-504`. PortKey's current term-of-art is "Model Catalog". Renaming to "Model-Catalog-style workspaces" matches the docs.

## Test Coverage Gaps

| Coverage | Present | Gap |
|---|---|---|
| Costly | virtual-key path (`portkey_costly.rs`) | no `x-portkey-config` / `x-portkey-provider` / `@<slug>/` variants |
| Mock | none | no PortKey SSE replay harness |
| `models.json` | none | 0 entries (P8) |
| Routing-guard unit | `compatible_tests.rs:553-564` | `x-portkey-custom-host`, `@<slug>/` id not tested |
| Hint emission | none | neither branch unit-tested |

Specific gaps to close: (1) mock-server 400 with `x-portkey-virtual-key required` body, assert the "set one of those" half of the hint; (2) same mock with the header already set, assert the "routing header is set ... but PortKey still rejected" half fires; (3) end-to-end vision call with `@anthropic-prod/claude-opus-4-7`, assert no preflight vision-support 400 (P9); (4) costly variant pinning routing via `x-portkey-config: pc-***`.

## Cross-Reference: opencode

opencode does not ship a built-in PortKey provider. Users wire PortKey through opencode's `custom-proxy` (`others/pi/packages/coding-agent/docs/models.md:170-185`) with `x-portkey-api-key` in `headers` — NOT `Authorization: Bearer`. This is the PortKey-canonical scheme and corroborates the fix recommended in P1. `others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:6-16` lists 9 built-in profiles (baseten, cerebras, deepinfra, deepseek, fireworks, groq, openrouter, togetherai, xai) — PortKey deliberately absent. Squeezy is one of the few coding-agent harnesses shipping PortKey as a first-class preset; the design opportunity (typed `config_id` / `fallback_targets` knobs, P7) is not yet leveraged.

## References

- [PortKey API headers reference](https://portkey.ai/docs/api-reference/inference-api/headers) — `x-portkey-api-key` canonical; `Authorization: Bearer` is for upstream provider credential
- [PortKey virtual keys / Model Catalog](https://portkey.ai/docs/product/ai-gateway/virtual-keys) — superseded by `@provider-slug/model-name`
- [PortKey gateway configs](https://portkey.ai/docs/product/ai-gateway/configs) — `pc-***` ids or inline JSON with `strategy.mode = fallback`
- [PortKey Claude Code integration](https://portkey.ai/docs/integrations/libraries/claude-code) — uses `x-portkey-api-key` + `x-portkey-provider`
- [PortKey Anthropic integration](https://portkey.ai/docs/integrations/llms/anthropic) — confirms `@<integration-slug>/<model-id>` shape
- [PortKey enterprise changelog](https://portkey.ai/docs/changelog/enterprise) — v2.8.0 `x-portkey-azure-entra-scope` (2026-05-14), v2.9.0 `x-portkey-sensitive-headers` + per-model `custom_host` (2026-05-21); no `x-portkey-router` exists
- [Squeezy shared OpenAI-compat audit](./openai-compatible.md) — PK-1..PK-4, plus M1 (dup headers), M6 (reasoning), M11 (vision), H3 (body fields), H7 (inline error retry)
- opencode peer reference: `others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:6-16` (no PortKey profile)
- opencode custom-proxy PortKey example: `others/pi/packages/coding-agent/docs/models.md:170-185`
