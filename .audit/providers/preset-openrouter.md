# OpenRouter Preset Audit

## Summary

OpenRouter is the canonical aggregator preset in the `OpenAiCompatibleProvider` family. squeezy's wiring is conservative and correct on the basics — base URL, Bearer auth, env-var resolution, attribution headers, Anthropic-via-aggregator `cache_control` markers, and OpenAI-via-aggregator `prompt_cache_key` forwarding all work. The deficits cluster in three areas: (1) **observability of OpenRouter's premium response fields** (`usage.cost`, `cost_details.upstream_inference_cost`, `provider` echo, mid-stream provider switch), (2) **complete absence of OpenRouter-specific routing knobs** (`provider.order`, `transforms`, `route`, `:online`/`:nitro`/`:floor`/`:free` suffixes), and (3) **legacy attribution header name** (`X-Title` rather than `X-OpenRouter-Title`, per the current docs). No OAuth/PKCE flow despite OpenRouter publishing one explicitly for CLI use. Severity tally: **0 critical / 2 high / 6 medium / 5 low / 2 nit** = **15 findings** (10 net-new, 5 referenced from shared catalog).

Top three actionable recommendations:
1. **Surface `usage.cost` and `cost_details.upstream_inference_cost`** from the OpenRouter response. The values arrive in every streaming response (no opt-in needed per current docs); `parse_chat_usage` at `crates/squeezy-llm/src/compatible.rs:1138-1164` drops them on the floor (OR-2, shared catalog). User sees squeezy's estimated cost instead of the actual billed cost.
2. **Re-emit `ServerModel` on every provider change**, not just the first chunk. OpenRouter's provider routing fallback (default-on per `allow_fallbacks: true`) silently moves a request to a backup upstream mid-stream; squeezy's `ServerModelEcho` (`crates/squeezy-llm/src/lib.rs:629-657`) seals after the first observation, so the user never learns their `anthropic/*` call landed on a Bedrock proxy (M-10, shared catalog).
3. **Add an OpenRouter-specific body extension for `provider` and `transforms`**. Even the most basic knobs (`provider.order`, `provider.allow_fallbacks: false`, `transforms: ["middle-out"]`, `route: "fallback"`) cannot be expressed today — `LlmRequest` has no shape for them and the chat-completions body builder hard-codes the field list (`compatible.rs:206-296`).

## Verified Configuration

- **Base URL**: `https://openrouter.ai/api/v1` (`crates/squeezy-core/src/lib.rs:79`) — Verified ✓ (matches `https://openrouter.ai/docs/api/reference/overview` and opencode's profile at `others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:13`).
- **Auth header**: `Authorization: Bearer <key>` (`crates/squeezy-llm/src/compatible.rs:474`, applied to every Bearer preset) — Verified ✓.
- **Env var**: `OPENROUTER_API_KEY` (`crates/squeezy-core/src/lib.rs:2099`) — Verified ✓ (matches opencode at `others/opencode/packages/llm/src/providers/openrouter.ts:84`).
- **Default attribution headers**: `HTTP-Referer: https://github.com/esqueezy/squeezy` + `X-Title: Squeezy` (`crates/squeezy-llm/src/compatible.rs:762-775`). Both are required for OpenRouter ranking entries. `HTTP-Referer` is the canonical/required header; `X-Title` is now legacy — current docs prefer `X-OpenRouter-Title` (see OR-1 below).
- **Default model**: `anthropic/claude-opus-4-7` (`crates/squeezy-core/src/lib.rs:80`). Active. The `:online`, `:nitro`, `:floor`, `:free` suffixes are accepted verbatim by squeezy since the model id is a plain string — but no help text or validation surfaces them.
- **Small-fast model**: `anthropic/claude-haiku-4-5` (`crates/squeezy-core/src/lib.rs:55`). Active.
- **is_full_tier**: `true` (`crates/squeezy-core/src/lib.rs:2048-2059`). Backed by 4 entries in `models.json`.
- **Cache markers**: Anthropic-via-OpenRouter (`anthropic/*` namespace) gets ephemeral `cache_control` on system/last-user/last-stable-tool blocks via `COMPAT_TABLE` (`compatible.rs:374-381`). OpenAI-via-OpenRouter (`openai/*`) gets `prompt_cache_key` + `prompt_cache_retention` forwarding via the same body builder (`compatible.rs:225-246`). Both paths are unit-tested at `compatible_tests.rs:727-815`.
- **Reasoning**: Both legacy (`reasoning_effort: "high"`) and unified (`reasoning: { effort: "high" }`) shapes are emitted unconditionally when `reasoning_effort` is set (`compatible.rs:215-224`). OpenRouter accepts both, so no per-route branching needed.
- **Streaming**: Always `stream: true` + `stream_options: { include_usage: true }` (`compatible.rs:209-210`). The `stream_options.include_usage` flag is **a no-op as of May 2026** — usage is always included by OpenRouter — but is harmless to emit.

## Implementation

The OpenRouter preset is a single enum variant (`OpenAiCompatiblePreset::OpenRouter`, `crates/squeezy-core/src/lib.rs:1966`) routed through the shared `OpenAiCompatibleProvider`. Preset metadata is per-variant:

- `as_str` → `"openrouter"` (`lib.rs:1998`)
- `display_name` → `"OpenRouter"` (`lib.rs:2023`)
- `default_base_url` → `DEFAULT_OPENROUTER_BASE_URL` (`lib.rs:2063`)
- `default_api_key_env` → `"OPENROUTER_API_KEY"` (`lib.rs:2099`)
- `default_model` → `DEFAULT_OPENROUTER_MODEL` (`lib.rs:2134`)
- `parse` aliases → `"openrouter" | "open_router"` (`lib.rs:2163`)
- TOML section aliases → `["openrouter"]` (`lib.rs:8738`)

The only OpenRouter-specific runtime branch in the entire codebase is `preset_default_headers` at `crates/squeezy-llm/src/compatible.rs:762-775`, which injects the two attribution headers when the preset matches. User-supplied `extra_headers` from `providers.openrouter.headers` TOML override the defaults via the merge at `compatible.rs:88-90`. Everything else (body shape, SSE parsing, error classification) is shared with the other 17 chat-completions presets.

The Anthropic-via-aggregator path (the common configuration for OpenRouter users on Claude models) is the most interesting interaction:
1. `request_body` (`compatible.rs:134-297`) calls `supports_anthropic_caching` (`compatible.rs:435-437`), which consults `COMPAT_TABLE` and matches the `anthropic/` prefix.
2. The shared `cache_policy` module decides marker placement (system tail / last user block / last stable tool).
3. The body is shaped with `cache_control: { type: "ephemeral" }` on system/last-user content arrays and `tools[idx].cache_control` on the last stable tool.
4. OpenRouter forwards these markers verbatim to Anthropic, and per OpenRouter's docs, uses "sticky routing" to pin subsequent requests to the same upstream provider so the cache stays warm — but only when the provider's cache-read price is cheaper than the regular prompt price.
5. `parse_chat_usage` (`compatible.rs:1138-1164`) reads `prompt_tokens_details.cached_tokens` (the OpenAI-shape field), which OpenRouter populates for cached Anthropic requests. So cache **read** accounting works; cache **write** accounting (`cache_write_tokens`) is dropped (see OR-7 below).

The 4 curated models in `crates/squeezy-llm/src/models.json:418-517` cover `anthropic/claude-opus-4-7`, `anthropic/claude-haiku-4-5`, `openai/gpt-5.5`, `google/gemini-2.5-pro`. All declare `prompt_caching: false` — incorrect for the Anthropic + OpenAI namespaces, which DO support caching via OpenRouter (OR-9 below).

## Findings

### OR-1 — `X-Title` is now legacy; current header is `X-OpenRouter-Title` (medium)

`compatible.rs:772` emits `X-Title: Squeezy`. OpenRouter's current attribution docs (verified `https://openrouter.ai/docs/app-attribution`, May 2026) state: "`X-Title` is backwards-compatible but superseded; `X-OpenRouter-Title` is the current standard." Still functional today (no breakage), but on a deprecation timeline. Tracked as OR-1 in the shared catalog.

**Fix**: emit `X-OpenRouter-Title: Squeezy` (the new canonical) and keep `X-Title` as a duplicate for a transition window, or drop `X-Title` entirely if OpenRouter's deprecation schedule allows. opencode currently still emits the legacy form (`others/opencode/packages/core/src/plugin/provider/openrouter.ts:15`), so squeezy is not uniquely lagging.

### OR-2 — `usage.cost` and `cost_details.upstream_inference_cost` are dropped on the floor (high)

OpenRouter's `usage` object includes:
- `cost` (USD, billed to user's OpenRouter account) — always present as of May 2026
- `cost_details.upstream_inference_cost` (USD, charged by the upstream provider; BYOK only)
- `prompt_tokens_details.cached_tokens`, `prompt_tokens_details.cache_write_tokens`, `prompt_tokens_details.audio_tokens`
- `completion_tokens_details.reasoning_tokens`

`parse_chat_usage` (`compatible.rs:1138-1164`) reads only `prompt_tokens`, `completion_tokens`, `prompt_tokens_details.cached_tokens`, and `completion_tokens_details.reasoning_tokens`. `cost`, `cost_details`, and `cache_write_tokens` are silently ignored. The `CostSnapshot` type (consumed by the agent loop's cost-tracking layer) has slots for `estimated_usd_micros` and `cache_write_input_tokens`, both currently set to `None` by this path. Tracked as OR-2 in the shared catalog.

**Fix**: extend `parse_chat_usage` to read `cost` (multiply by 1e6 for `estimated_usd_micros` and rename the field to make clear it can be provider-authoritative when the route surfaces it), and `prompt_tokens_details.cache_write_tokens` for `cache_write_input_tokens`. This eliminates squeezy's reliance on the local `models.json` pricing table for OpenRouter — squeezy ships zero `pricing` entries for OpenRouter models (verified at `models.json:433, 458, 483, 508`), so today every OpenRouter call reports `0 / $0.00` for actual cost.

### OR-3 — Provider routing knobs are unreachable (medium)

The `provider` body field is OpenRouter's most-distinctive feature: `provider.order: ["anthropic", "bedrock"]`, `provider.allow_fallbacks: false`, `provider.only: ["anthropic"]`, `provider.ignore: ["openai"]`, `provider.data_collection: "deny"`, `provider.quantizations: ["fp8", "fp16"]`, `provider.zdr: true`, `provider.sort: "throughput"`, `provider.max_price: { prompt: 5, completion: 15 }`. None of these are reachable. `LlmRequest` (`crates/squeezy-llm/src/lib.rs`) has no field for `provider` and `compatible.rs:206-296` hard-codes the body schema. Tracked as OR-3 in the shared catalog; subsumed under shared H3.

**Fix**: the cleanest path is a typed `provider_routing: Option<ProviderRoutingSpec>` on `LlmRequest` that the chat-completions body builder forwards verbatim when the preset is OpenRouter. A lighter path is a `Custom`-style escape-hatch body merge for any preset, controlled by a TOML `[providers.openrouter.body_extension]` table.

### OR-4 — Mid-stream provider fallback is invisible (medium)

OpenRouter's `allow_fallbacks: true` (default) routes around upstream 5xx/rate-limit by retrying through a different provider mid-request. Each SSE chunk carries an updated top-level `model` field and an OpenRouter-specific `provider` field. `parse_chat_event` (`compatible.rs:1033-1042`) captures `model` only on the first chunk (the `is_none()` guard) and never reads `provider`. The outer loop's `ServerModelEcho::observe` (`crates/squeezy-llm/src/lib.rs:629-656`) seals after the first echo, so any later `model` change is dropped. Tracked as OR-4 / M-10 in the shared catalog.

**Fix**: re-emit `LlmEvent::ServerModel` whenever the echo differs from the previously observed value (track last-emitted, not first-emitted). Plus: read the `provider` field at `parse_chat_event` and expose it as a new event variant (e.g. `LlmEvent::UpstreamProvider`) so the user sees not just "anthropic/claude-opus-4-7 → anthropic/claude-opus-4-7" but "request now flowing through Bedrock instead of direct Anthropic."

### OR-5 — No OAuth PKCE flow despite OpenRouter publishing one (medium)

OpenRouter ships a documented PKCE OAuth flow (`https://openrouter.ai/auth?callback_url=...&code_challenge=...&code_challenge_method=S256`) intended for CLI use: the user runs `squeezy login --openrouter`, browser opens, user authorizes, callback delivers a `code`, squeezy POSTs to `https://openrouter.ai/api/v1/auth/keys` with the code + `code_verifier` to get a user-controlled API key. squeezy implements no such flow (verified: zero references to `/auth/keys`, `pkce`, or `openrouter.ai/auth` in `crates/`). The OpenAI Codex preset has the analogous `~/.squeezy/auth/openai-codex.json` token store (`crates/squeezy-core/src/lib.rs:1918-1919`); GitHub Copilot uses `with_api_key_source` for rotating tokens (`compatible.rs:106`); the infrastructure to add OAuth-source-backed OpenRouter is present.

**Fix**: implement an `auth openrouter` subcommand that runs the PKCE flow, persists the obtained API key to `~/.squeezy/auth/openrouter.json`, and wires a credential source through `with_api_key_source`. Users still have the `OPENROUTER_API_KEY` env-var path as the unauthenticated alternative.

### OR-6 — `:online` / `:nitro` / `:floor` / `:free` suffixes work but are undocumented (low)

OpenRouter accepts model-id suffixes `:online` (built-in web search via Exa.ai; note OpenRouter docs say this is deprecated in favor of the `openrouter:web_search` server tool), `:nitro` (highest-throughput provider), `:floor` (cheapest provider), `:free` (free tier where available). squeezy passes the model id through verbatim so these technically work, but no documentation, CLI hint, or `--list-models` surfaces them. Users who don't know to type `anthropic/claude-opus-4-7:nitro` won't discover the routing capability.

**Fix**: add a doc page or `--help` blurb under the OpenRouter preset describing the four suffixes and their semantics. Optional: gate `:online` behind a warning since OpenRouter has deprecated it.

### OR-7 — `cache_write_tokens` is dropped, masking cache-write billing (low → medium)

`parse_chat_usage` reads `prompt_tokens_details.cached_tokens` but never reads `prompt_tokens_details.cache_write_tokens` (the field OpenRouter populates for Anthropic models with explicit cache writes, per the current usage-accounting docs). `CostSnapshot.cache_write_input_tokens` stays `None`. For multi-turn coding sessions on `anthropic/*` via OpenRouter, the first turn's cache-write billing is invisible while subsequent cache-read savings ARE accounted (because `cached_tokens` IS read). Net result: cost dashboard underestimates the first turn and the second-turn savings appear larger than they are in net.

**Fix**: also read `prompt_tokens_details.cache_write_tokens` and assign to `cache_write_input_tokens`. Mirror the native Anthropic adapter's behavior. Pairs naturally with the OR-2 cost-field fix.

### OR-8 — `usage`/`reasoning` request body knobs are unreachable (low)

OpenRouter accepts request-body fields `usage: { include: true }` (deprecated but accepted), `reasoning: { effort, max_tokens, exclude }` (with `exclude: true` to hide the reasoning text from the response while still using it internally), and `route: "fallback"` (for cross-model fallback at the OpenRouter level, e.g. `model: "anthropic/claude-opus-4-7"`, `models: ["openai/gpt-5.5"]`, `route: "fallback"`). `LlmRequest` exposes only `reasoning_effort`; the rest cannot be set. opencode exposes them via `providerOptions.openrouter.{usage,reasoning,promptCacheKey}` (`others/opencode/packages/llm/src/providers/openrouter.ts:56-67`).

**Fix**: same as OR-3 — a per-preset body extension would absorb these together.

### OR-9 — `models.json` lies about `prompt_caching: false` for cache-capable namespaces (low)

`crates/squeezy-llm/src/models.json` entries for `anthropic/claude-opus-4-7` (line 431), `anthropic/claude-haiku-4-5` (line 456), and `openai/gpt-5.5` (line 481) all set `prompt_caching: false`. But the code DOES cache for all three:
- `supports_anthropic_caching` returns `true` for the two Anthropic entries via `COMPAT_TABLE` matching the `anthropic/` prefix (verified at `compatible.rs:374-381, 435-437`).
- `prompt_cache_key` + `prompt_cache_retention` is emitted for `openai/gpt-5.5` (verified by the test at `compatible_tests.rs:727-738`).

The capability flag is consulted by the model registry to decide whether to expose caching knobs to the agent. Setting `false` may suppress UI/feature paths that would otherwise be active. Net effect today: harmless (the body builder doesn't consult the registry — it consults `COMPAT_TABLE` directly), but the JSON misrepresents the wire behavior and will mislead future contributors.

**Fix**: set `prompt_caching: true` for the three cache-capable entries. The `google/gemini-2.5-pro` entry is correct as `false` (no `google/` cache marker emission).

### OR-10 — `pricing: null` for every OpenRouter model entry (low)

All 4 OpenRouter `models.json` entries set `pricing: null` (`models.json:433, 458, 483, 508`). This is partly justified by OR-2: OpenRouter ships actual cost in `usage.cost`, so the local pricing table is redundant *if* the response field is surfaced. Today it isn't (OR-2), so the user gets zero cost data on every OpenRouter call.

**Fix**: either populate `pricing` with snapshot values that match OpenRouter's listed model prices (with the staleness risk that comes with hard-coded pricing), or block-list the entries from cost estimation entirely with a `pricing: "provider_authoritative"` sentinel and rely exclusively on the OR-2 fix to surface real-time billed cost.

### OR-11 — `stream_options.include_usage: true` is a no-op as of May 2026 (nit)

The shared body builder unconditionally emits `stream_options: { include_usage: true }` (`compatible.rs:210`). Per OpenRouter's current docs ("Full usage details are now always included automatically in every response. The `usage: { include: true }` and `stream_options: { include_usage: true }` parameters are deprecated and have no effect."), the field is harmless but obsolete on the OpenRouter route. Other presets in the family (Groq, OpenAI direct) still REQUIRE it to surface `usage`, so this is correct as a default — just noting the cost on the OpenRouter route is 8 bytes of payload, not a behavior issue.

**Fix**: none required; document.

### OR-12 — Stream-loss after `finish_reason` swallows OpenRouter's final usage chunk (high, inherited from C1)

The shared-catalog finding C1 hits OpenRouter directly. OpenRouter sends a final `usage`-only chunk after the chunk carrying `finish_reason: "stop"`. Because `parse_chat_event` flips `state.completed_emitted = true` inline on `finish_reason` and the outer loop short-circuits before the next chunk parses, the final usage chunk is dropped. Net result: every OpenRouter call's token + cost accounting is whatever the previous chunk happened to carry (often zero), regardless of the OR-2 fix above. C1 is the prerequisite for OR-2 to actually work.

**Fix**: same as C1 — only seal `completed_emitted` on `[DONE]`, continue parsing usage chunks after `finish_reason`.

### OR-13 — `account_id` / `gateway_id` placeholder substitution runs on the OpenRouter URL too (nit)

`substitute_url_placeholders` (`compatible.rs:701-745`) runs for every preset including OpenRouter (`compatible.rs:77-82`). Today the OpenRouter URL has no `{account_id}` / `{gateway_id}` tokens, so the function is a no-op. But a user who pastes `https://openrouter.ai/api/v1/{gateway_id}` as a debugging mistake won't get a clean error — they'll get a 404 against a literal `{gateway_id}` URL or the substitution will reject it. Defensive code coverage is fine; no action needed.

### OR-14 — OpenRouter `: OPENROUTER PROCESSING` SSE keepalives are silently absorbed (correct) (nit)

OpenRouter emits SSE comment lines (`: OPENROUTER PROCESSING`) as keepalives to prevent connection timeouts (per `https://openrouter.ai/docs/api/reference/streaming`). `decode_sse_event` (`sse.rs:49-63`) extracts only `data:` lines, so comment lines are silently dropped — which is the correct SSE-spec behavior. Verified by inspection. No action needed.

### OR-15 — OpenRouter's `provider` echo (per-chunk) is read nowhere (medium)

In addition to `model`, OpenRouter ships a top-level `provider` field on each SSE chunk identifying the upstream that served the chunk (`anthropic`, `bedrock`, `vertex`, `openai`, etc.). This is the canonical signal for "which provider actually answered" and is exactly what users opting into `provider.order` care about. `parse_chat_event` reads `id`, `model`, `usage`, `choices` — not `provider`. There's no `LlmEvent` variant for it. Pair with OR-4 fix.

**Fix**: surface a new event (or extend `LlmEvent::ServerModel` to carry the upstream provider slug) so the TUI can render "via anthropic" / "via bedrock" badges.

## Catalog (Models)

`crates/squeezy-llm/src/models.json` ships 4 OpenRouter entries:

| id | profile | context | max_out | tokenizer | lifecycle | caching | pricing |
|---|---|---|---|---|---|---|---|
| `anthropic/claude-opus-4-7` | strong | 200K | 64K | anthropic | active | false (wrong) | null |
| `anthropic/claude-haiku-4-5` | cheap | 200K | 64K | anthropic | active | false (wrong) | null |
| `openai/gpt-5.5` | strong | 400K | 128K | openai_compatible | active | false (wrong) | null |
| `google/gemini-2.5-pro` | strong | 1.05M | 65K | google | active | false (correct) | null |

Notes: zero `pricing` entries (OR-10), three wrong `prompt_caching` flags (OR-9), no `xai/grok-*` entries despite `xai/` being a recognized COMPAT_TABLE namespace, no `:nitro`/`:floor`/`:free` variants surfaced.

## Test Coverage

- **Costly**: `crates/squeezy-llm/tests/openrouter_costly.rs` — single test, runs an echo smoke check against `anthropic/claude-haiku-4-5` and asserts the response contains `squeezy-ok`. No cost assertion (would catch OR-2). No mid-stream provider switch (would catch OR-4). No suffix test (would catch OR-6).
- **Unit**: `crates/squeezy-llm/src/compatible_tests.rs:540-550` verifies the attribution headers default. `compatible_tests.rs:727-738` verifies `prompt_cache_key` forwarding through OpenAI-via-OpenRouter. `compatible_tests.rs:741-754` verifies the Anthropic cache_control + prompt_cache_key coexistence. `compatible_tests.rs:784-802` verifies the `Long` retention TTL upgrade. `compatible_tests.rs:925-944` verifies OpenRouter is in `is_full_tier`. Nothing tests the legacy-vs-current attribution header name (OR-1) or `usage.cost` parsing (OR-2).
- **Gaps**:
  - No mock-server test for the post-`finish_reason` usage chunk pattern (OR-12).
  - No test for OpenRouter's `provider` field at the chunk level (OR-15).
  - No test for the `X-OpenRouter-Title` header (the new canonical name) (OR-1).
  - No `models.json` invariant test that asserts the `prompt_caching` flag matches `COMPAT_TABLE.supports_cache_control` (OR-9).

## Verification Strategy

1. **Mock SSE replay**: capture an actual OpenRouter response (a free-tier call against `deepseek/deepseek-chat:free` is sufficient) to a fixture file. Replay it through a `wiremock` server and assert `parse_chat_usage` surfaces `cost`, `cache_write_tokens`. This exercises OR-2, OR-7, OR-12 together.
2. **Provider-switch fixture**: synthesize a multi-chunk fixture where chunk 1 has `model: "anthropic/claude-opus-4-7", provider: "anthropic"` and chunk 5 (after a synthetic 5xx) has `model: "anthropic/claude-opus-4-7", provider: "bedrock"`. Assert the parser emits TWO `ServerModel` events (or a single `UpstreamProvider` event change). This exercises OR-4 + OR-15.
3. **Attribution-header roundtrip**: capture both `X-Title` and `X-OpenRouter-Title` from the outbound request via a mock-server header assertion. Confirm the desired final state (whether one or both end up on the wire) (OR-1).
4. **Costly-cost-real**: extend `openrouter_costly.rs` to run a 100-token completion against `anthropic/claude-haiku-4-5`, assert the parsed `CostSnapshot.estimated_usd_micros > 0`. Catches OR-2 + OR-7 + OR-12 in one shot.
5. **`models.json` invariant unit test**: walk every entry where `provider == "openrouter"`, look up `compat_entry(id)`, assert `entry.capabilities.prompt_caching == compat_entry(id).supports_cache_control` (OR-9).

## References

- OpenRouter API reference: https://openrouter.ai/docs/api/reference/overview
- OpenRouter app attribution headers: https://openrouter.ai/docs/app-attribution
- OpenRouter usage accounting (`cost`, `cost_details`, `*_tokens_details`): https://openrouter.ai/docs/cookbook/administration/usage-accounting
- OpenRouter provider routing parameters: https://openrouter.ai/docs/guides/routing/provider-selection
- OpenRouter model fallback (`models: [...]`, `route: "fallback"`): https://openrouter.ai/docs/guides/routing/model-fallbacks
- OpenRouter streaming (SSE shape, mid-stream errors, `: OPENROUTER PROCESSING` keepalives): https://openrouter.ai/docs/api/reference/streaming
- OpenRouter prompt caching (Anthropic `cache_control` + sticky routing): https://openrouter.ai/docs/guides/best-practices/prompt-caching
- OpenRouter OAuth PKCE: https://openrouter.ai/docs/guides/overview/auth/oauth
- OpenRouter reasoning tokens (`reasoning: {effort, max_tokens, exclude}`): https://openrouter.ai/docs/guides/best-practices/reasoning-tokens
- OpenRouter `:online` web variant: https://openrouter.ai/docs/guides/routing/model-variants/online
- OpenRouter free variant `:free`: https://openrouter.ai/docs/guides/routing/model-variants/free
- OpenRouter changelog (for monitoring breaking changes): https://openrouter.ai/docs/changelog
- opencode OpenRouter route: `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/openrouter.ts`
- opencode OpenRouter plugin (attribution headers): `/Users/abbassabra/esqueezy/others/opencode/packages/core/src/plugin/provider/openrouter.ts`
- opencode shared profile (base URLs): `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/openai-compatible-profile.ts`
- Shared squeezy OpenAI-compatible audit: `/Users/abbassabra/esqueezy/squeezy/.audit/providers/openai-compatible.md` (OR-1..OR-4, M-10, C1)
- squeezy attribution headers: `crates/squeezy-llm/src/compatible.rs:762-775`
- squeezy preset metadata: `crates/squeezy-core/src/lib.rs:1966-2214`
- squeezy `COMPAT_TABLE`: `crates/squeezy-llm/src/compatible.rs:367-403`
- squeezy `parse_chat_usage`: `crates/squeezy-llm/src/compatible.rs:1138-1164`
- squeezy `ServerModelEcho`: `crates/squeezy-llm/src/lib.rs:629-656`
- squeezy OpenRouter models: `crates/squeezy-llm/src/models.json:418-517`
- squeezy OpenRouter costly test: `crates/squeezy-llm/tests/openrouter_costly.rs`
