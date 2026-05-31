# Cloudflare AI Gateway Preset Audit

## Summary

**2 critical / 4 high / 9 medium / 6 low / 2 nit = 23 findings.** The shared aggregator audit at `.audit/providers/openai-compatible.md` already booked CFAG-1 (inverted dual-auth) and CFAG-2 (`/compat` deprecated); referenced by ID below, not re-listed.

Three problem clusters:

1. **Wire-shape stale**: squeezy targets the pre-2026 `/compat` URL (`crates/squeezy-core/src/lib.rs:125`). Cloudflare's May 21 2026 REST API moves the host to `api.cloudflare.com/.../ai/v1/{chat/completions,responses,messages}`, drops `gateway_id` from the path in favor of a `cf-aig-gateway-id` header, and routes any upstream via a body `model` prefix (`openai/…`, `anthropic/…`, `@cf/…`).
2. **Observability surface absent**: 1 of 18 documented `cf-aig-*` headers (`cf-aig-authorization`) is exposed as typed config. The rest can only be pasted as raw strings into `[providers.cloudflare_ai_gateway.headers]`, with no per-request override path.
3. **Placeholder hygiene**: `substitute_url_placeholders` (`crates/squeezy-llm/src/compatible.rs:701-745`) does raw `String::replace`. Opencode wraps both inputs in `encodeURIComponent` (`others/opencode/packages/llm/src/providers/cloudflare.ts:39,56`).

Top three recommendations:

1. Migrate the default `base_url` to `https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1`, emit `cf-aig-gateway-id` instead of a URL segment, keep the legacy URL as opt-in.
2. Re-shape credentials so `cf-aig-authorization` carries the Cloudflare gateway token and `Authorization: Bearer` carries the upstream provider's key (current arrangement only works for Workers AI upstreams — CFAG-1).
3. Promote the `cf-aig-*` knob surface to typed TOML config and thread per-request overrides through `LlmRequest`.

## Verified

| Surface | squeezy | Reality (May–Jun 2026) | Verified |
|---|---|---|---|
| Default base URL | `…/gateway.ai.cloudflare.com/v1/{account_id}/{gateway_id}/compat` (`crates/squeezy-core/src/lib.rs:124-125`) | `/compat` deprecated; recommended path is the REST API. Legacy keeps working but is on the deprecation timeline. | **Fail** (CFAG-2) |
| `Authorization` slot | `CLOUDFLARE_API_KEY` (`crates/squeezy-llm/src/compatible.rs:474`) | On `/compat` upstream routes the Bearer must be the **upstream** provider's key; CF gateway token belongs in `cf-aig-authorization`. | **Fail** (CFAG-1) |
| `cf-aig-authorization` header | Injected from `CF_AIG_TOKEN` (`crates/squeezy-core/src/lib.rs:8700-8713`), user override wins | Required on authenticated gateways; format `Bearer <token>`. | Pass for the header path, fails when paired with CFAG-1 |
| `account_id` placeholder | Trimmed + required (`compatible.rs:715-744`) | Required on both URL shapes. | Pass on presence, no character validation (CFAG-7) |
| `gateway_id` placeholder | Defaults to `"default"` at config-build time (`crates/squeezy-core/src/lib.rs:8684-8688`) | CF auto-creates the `default` gateway on first use. | Pass for config flow, fails for `from_config` callers (CFAG-5) |
| Env var | `CLOUDFLARE_API_KEY` reused for Workers AI + AI Gateway upstream (`crates/squeezy-core/src/lib.rs:2126-2127`) | Opencode keeps them separate: `CLOUDFLARE_API_TOKEN`/`CF_AIG_TOKEN` for gateway auth, `CLOUDFLARE_API_KEY`/`CLOUDFLARE_WORKERS_AI_TOKEN` for Workers AI (`others/opencode/packages/llm/src/providers/cloudflare.ts:10-11`). | **Fail** (CFAG-6) |
| `cf-aig-gateway-id` header | Never emitted | Required by the new REST API for non-default gateways and unconditionally for Workers AI through AI Gateway. | **Fail** (CFAG-3) |
| Cache / cost / log headers | None exposed | CF documents 15+ `cf-aig-*` request/response headers. | **Fail** (CFAG-4) |
| `cf-aig-cache-status` on stream | Never read | `HIT` should zero cost (CF's pricing model). | **Fail** (CFAG-8) |
| Custom + computed cost | Neither sent nor parsed | `cf-aig-custom-cost` per-request, plus gateway-logged cost. | **Fail** (CFAG-9) |
| Custom-URL SSRF guard | None (`check_base_url_scheme` runs only for Azure, `crates/squeezy-core/src/lib.rs:8539-8542+`) | Same root as shared audit M5. | **Fail** (inherits) |

## Implementation Overview

The preset routes through `OpenAiCompatibleProvider` like the other 18 aggregator presets (`crates/squeezy-llm/src/compatible.rs:39-46`). Three preset-specific code paths:

1. `crates/squeezy-core/src/lib.rs:124-127` — default URL template (`/compat` shape), gateway-id default slug, and the OpenAI-compat model id.
2. `crates/squeezy-core/src/lib.rs:8659-8713` — config builder that requires `cloudflare_account_id`/`CLOUDFLARE_ACCOUNT_ID`, defaults `cloudflare_gateway_id` to `"default"`, and injects `cf-aig-authorization: Bearer {CF_AIG_TOKEN}` when the user hasn't already set it (user-supplied wins).
3. `crates/squeezy-llm/src/compatible.rs:701-745` — `substitute_url_placeholders` consumes `OpenAiCompatibleConfig.account_id`/`gateway_id` (`crates/squeezy-core/src/lib.rs:1944-1955`) before the URL locks in.

The `Authorization` header itself is set by `bearer_auth(api_key)` around `crates/squeezy-llm/src/compatible.rs:474`, with `api_key` resolved from `CLOUDFLARE_API_KEY`. Opencode's split-credential model (`others/opencode/packages/llm/src/providers/cloudflare.ts:42-51`) is not mirrored — squeezy has exactly one key slot.

`provider_setting_headers` (`crates/squeezy-core/src/lib.rs:8505-8510`) lifts the raw `providers.cloudflare_ai_gateway.headers` table into `extra_headers`. No typed schema, no validation, no per-request override; everything locks at `OpenAiCompatibleProvider::from_config` time.

Tests cover URL substitution + dual-auth header presence (`crates/squeezy-llm/src/compatible_tests.rs:986-1078`, `crates/squeezy-core/src/lib_tests.rs:2553-2691`). No end-to-end mock against either URL shape; no costly integration test (verified — no `cloudflare_ai_gateway_costly.rs` in `crates/squeezy-llm/tests/`).

## Findings

### Already booked in shared audit

- **CFAG-1** — Inverted dual-auth. See `.audit/providers/openai-compatible.md` §C2.
- **CFAG-2** — `/compat` deprecated. See §C3.

### CFAG-3 — Missing `cf-aig-gateway-id` header path (high → critical post-migration)

The new REST API drops `gateway_id` from the URL — gateway selection moves to a `cf-aig-gateway-id` header. Squeezy emits nothing of the sort. The moment CFAG-2 is fixed by switching to `…/ai/v1/chat/completions`, the gateway id is silently dropped and CF routes through the default gateway. Workers-AI-through-AI-Gateway requires the header unconditionally.

**Fix**: emit `cf-aig-gateway-id: {gateway_id}` (when set) and stop carrying it in the URL.

### CFAG-4 — `cf-aig-*` knob surface unexposed (high)

CF AI Gateway's value-add is per-request observability and reliability headers. Documented surface (per the [header glossary](https://developers.cloudflare.com/ai-gateway/glossary/)):

- Caching: `cf-aig-cache-key`, `cf-aig-cache-ttl` (60s–1mo), `cf-aig-skip-cache`, response `cf-aig-cache-status`.
- Retries: `cf-aig-max-attempts`, `cf-aig-retry-delay`, `cf-aig-backoff`, `cf-aig-request-timeout`.
- Observability: `cf-aig-event-id`, `cf-aig-log-id`, `cf-aig-collect-log`, `cf-aig-metadata`, `cf-aig-step` (response).
- Cost: `cf-aig-custom-cost`.
- Compliance: `cf-aig-dlp` (response).

Users today paste each into `[providers.cloudflare_ai_gateway.headers]` (`crates/squeezy-core/src/lib.rs:8505-8510`) as raw strings — no validation, no recognition that `cf-aig-metadata` is JSON-stringified, no per-call override (everything is locked at construction time, `crates/squeezy-llm/src/compatible.rs:62-98`).

**Fix**: add a typed `CloudflareAiGatewaySettings` block under `OpenAiCompatibleConfig` (shape similar to `ProviderTransportConfig`) and thread per-turn overrides through `LlmRequest`. Peer reference: opencode's `RouteDefaultsInput` (`others/opencode/packages/llm/src/providers/cloudflare.ts:23,34`).

### CFAG-5 — `gateway_id` not auto-defaulted by `from_config` (medium)

`from_config` (`crates/squeezy-llm/src/compatible.rs:62-98`) consumes `config.gateway_id` verbatim; when it's `None` and the template still has `{gateway_id}`, `substitute_url_placeholders` returns `ProviderNotConfigured` (`compatible.rs:715-740`). The config builder at `crates/squeezy-core/src/lib.rs:8684-8688` does default to `"default"`, but any programmatic caller constructing `OpenAiCompatibleConfig` directly gets a hard error.

**Fix**: have `substitute_url_placeholders` default `{gateway_id}` to `"default"` when the preset is `CloudflareAiGateway`. Better: drop the placeholder from the URL entirely per CFAG-3.

### CFAG-6 — One env var, two semantically distinct credentials (medium)

`CLOUDFLARE_API_KEY` is the Workers AI account token (`crates/squeezy-core/src/lib.rs:2126`) AND the AI Gateway Bearer slot (`:2127`). Running both presets makes them collide: fixing one breaks the other. Combined with CFAG-1, the env value also needs to swap with each upstream change (Anthropic vs OpenAI vs Groq).

**Fix**: rename the AI Gateway preset's `default_api_key_env` (e.g. `CLOUDFLARE_AI_GATEWAY_UPSTREAM_KEY`) and treat `CF_AIG_TOKEN` as the gateway-side credential. Opencode separates them (`others/opencode/packages/llm/src/providers/cloudflare.ts:10-11`).

### CFAG-7 — `account_id` / `gateway_id` accept path-poisoning characters (medium)

`substitute_url_placeholders` does raw `String::replace` with no rejection of `/`, `?`, `#`, whitespace, or non-ASCII (`crates/squeezy-llm/src/compatible.rs:730-743`). Examples that pass through today:

- `cloudflare_account_id = "acct/extra"` → `…/v1/acct/extra/default/compat` (extra path segment).
- `cloudflare_gateway_id = "g?x=1"` → `…/g?x=1/compat` (query string opens; `/compat` becomes garbage).
- `cloudflare_gateway_id = "g\ny"` (newline) → broken URL, no early failure.

Booked as M8/M-34 in the shared audit; the AI Gateway preset is the only one taking *two* placeholders.

**Fix**: validate against `^[A-Za-z0-9_-]{1,64}$` or percent-encode. Opencode pattern at `others/opencode/packages/llm/src/providers/cloudflare.ts:39,56`.

### CFAG-8 — Cached responses billed as fresh streams (medium)

CF's caching layer returns the upstream's original SSE body verbatim with `cf-aig-cache-status: HIT`. Squeezy never reads response headers in the stream parser (`crates/squeezy-llm/src/compatible.rs:990-1170` only references the body). A cache hit:

- Is reported to the cost broker as a normal request (input/output tokens are whatever the upstream's `usage` chunk reported), even though CF's pricing says cache hits are billed at zero.
- Has no transcript breadcrumb, so the user can't tell which sessions benefited from caching they paid CF for.

**Fix**: hoist `cf-aig-cache-status` out of response headers and emit `LlmEvent::ServerNote("cache hit")` (or extend `CostSnapshot` with a `cached: bool`); zero the cost contribution on `HIT`.

### CFAG-9 — Custom / gateway-computed cost not surfaced (medium)

Two gaps:

- No way to send `cf-aig-custom-cost: {"per_token_in": …, "per_token_out": …}` per request (same root as CFAG-4).
- `parse_chat_usage` (`crates/squeezy-llm/src/compatible.rs:1138-1164`) only reads `usage.{prompt,completion}_tokens[_details]`. CF AI Gateway dashboards expose a computed cost per call; squeezy's billing diverges.

**Fix**: bundle with CFAG-4 (request-side) and extend `parse_chat_usage` to consume any `cost` field, similar to OpenRouter (OR-2 in shared audit).

### CFAG-10 — `/compat` model-prefix requirement not enforced (medium)

On `/compat` the model id must carry an upstream prefix (`openai/…`, `anthropic/…`, `groq/…`, `@cf/…`). Bare `gpt-5.5` 400s. Default at `crates/squeezy-core/src/lib.rs:128` is correct (`@cf/meta/llama-3.3-70b-instruct-fp8-fast`), but user overrides aren't validated. Same shape as Vercel VL-1 in the shared audit.

**Fix**: when preset is `CloudflareAiGateway`, reject a `model` lacking an upstream prefix at config-build time.

### CFAG-11 — No load-balancing / fallback surface (medium)

Per [fallbacks docs](https://developers.cloudflare.com/ai-gateway/configuration/fallbacks/), AI Gateway accepts a body-level array shape unique to the Universal Endpoint (itself deprecated per https://developers.cloudflare.com/ai-gateway/usage/universal/). Squeezy has no path to emit it. The `Custom` preset escape hatch doesn't work either — the body builder always wraps requests in chat-completions shape (`crates/squeezy-llm/src/compatible.rs:134-297`).

**Fix**: out of scope for the OpenAI-compat preset; document as a known limitation.

### CFAG-12 — squeezy retries on top of CF retries (low → medium)

`crates/squeezy-llm/src/retry.rs` is provider-agnostic. If a user sets `cf-aig-max-attempts: 3`, CF retries upstream and squeezy *also* retries on top, doubling wall-clock cost on a flapping upstream.

**Fix**: bundle with CFAG-4. When the typed `cache_headers.max_attempts > 0`, lower squeezy's own `retries` for this provider so the budget lives in one layer.

### CFAG-13 — Streaming + cache fast-path semantics untested (low)

When `cf-aig-skip-cache: false` and a HIT occurs with `stream: true`, CF replays cached SSE chunks. squeezy's parser handles arbitrary SSE so it works in practice, but: (a) no test (verified — no `cache_status` references in `crates/squeezy-llm/src/compatible_tests.rs`), and (b) `parse_chat_event`'s `state.completed_emitted` interaction with C1 (shared audit) may differ when CF concatenates cached chunks. Worth a regression test once C1 is fixed.

### CFAG-14 — CF error envelope mismatch (low)

CF returns its own error shape on DLP blocks (`cf-aig-dlp` response header) and pre-upstream rate limits (`{"error": "AI Gateway: rate limit exceeded"}` — top-level string, not `{error: {message}}`). `format_chat_error` (`crates/squeezy-llm/src/compatible.rs:976-998`) probes the OpenAI shape first; CF's flat string falls through to `default_message`. Booked as H6 in the shared audit; the gateway is the most-likely producer.

### CFAG-15 — Duplicate `Authorization` headers possible (low)

If a user puts `Authorization` in `providers.cloudflare_ai_gateway.headers`, the `bearer_auth(api_key)` call (`crates/squeezy-llm/src/compatible.rs:464-485`) still emits its own value. `reqwest` permits duplicate headers; CF's edge takes the first; the user's manual override silently loses. Tied to shared audit M1 (BTreeMap-case issue).

**Fix**: normalize header keys to canonical case at config-build time and let user-supplied `Authorization` override the bearer call.

### CFAG-16 — No costly integration test (low)

`crates/squeezy-llm/tests/` has no `cloudflare_ai_gateway_costly.rs`. Minimum viable: route `openai/gpt-4o-mini` through AI Gateway with both `OPENAI_API_KEY` and `CF_AIG_TOKEN` set, assert `Authorization: Bearer ${OPENAI_API_KEY}` + `cf-aig-authorization: Bearer ${CF_AIG_TOKEN}`. Would have caught CFAG-1 immediately.

### CFAG-17 — No `models.json` entries (low)

`grep cloudflare crates/squeezy-llm/src/models.json` is empty. `is_full_tier()` correctly returns `false` for `CloudflareAiGateway` (`crates/squeezy-core/src/lib.rs:2048-2059`), but users picking this preset to standardize multi-vendor routing get only generic context-window estimates.

### CFAG-18 — `display_name` doesn't disambiguate auth posture (nit)

`crates/squeezy-core/src/lib.rs:2040` → `"Cloudflare AI Gateway"`. After CFAG-2 is fixed, the picker should distinguish `(legacy /compat)` vs `(REST API)` so debuggers can tell which wire shape a TOML config drives.

### CFAG-19 — Comment cements the wrong mental model (nit)

`crates/squeezy-core/src/lib.rs:2121-2125`: *"Cloudflare uses one API token (CLOUDFLARE_API_KEY) for the direct Workers AI endpoint, and the same token for the upstream bearer when routing through AI Gateway."* This is exactly CFAG-1's inverted understanding — and probably how the bug landed.

**Fix**: tighten the comment to describe the actual dual-auth split.

## Catalog (`cf-aig-*` coverage)

| Header | Purpose | Typed config? |
|---|---|---|
| `cf-aig-authorization` | CF gateway token Bearer | Yes (`CF_AIG_TOKEN` env, `crates/squeezy-core/src/lib.rs:8700-8713`) |
| `cf-aig-gateway-id` | New REST API gateway selector | **No** (CFAG-3) |
| `cf-aig-cache-key` | Custom cache key | No |
| `cf-aig-cache-ttl` | Cache duration (60s–1mo) | No |
| `cf-aig-skip-cache` | Bypass cache | No |
| `cf-aig-cache-status` (resp) | HIT / MISS | No — never read (CFAG-8) |
| `cf-aig-metadata` | JSON-stringified custom metadata | No |
| `cf-aig-custom-cost` | Per-token cost override | No (CFAG-9) |
| `cf-aig-event-id` | Trace id across related requests | No |
| `cf-aig-log-id` | Target a log entry for feedback APIs | No |
| `cf-aig-collect-log` | Bypass default log setting | No |
| `cf-aig-skip-log` | Disable logging | No |
| `cf-aig-max-attempts` | Gateway-managed retry budget | No (CFAG-12) |
| `cf-aig-retry-delay` | Inter-attempt delay | No |
| `cf-aig-backoff` | Backoff type | No |
| `cf-aig-request-timeout` | Per-request timeout (ms) | No |
| `cf-aig-step` (resp) | Which fallback step succeeded | No |
| `cf-aig-dlp` (resp) | DLP policy match | No (CFAG-14) |

**Score**: 1 of 18 documented headers has typed config. The remaining 17 are raw-string pass-through only, and none vary per request.

## Test Coverage

Covered:
- `{account_id}`/`{gateway_id}` substitution against the deprecated `/compat` URL (`crates/squeezy-llm/src/compatible_tests.rs:986-1029`).
- `ProviderNotConfigured` when `account_id` is missing (`compatible_tests.rs:1031-1078`).
- `cf-aig-authorization` injection from `CF_AIG_TOKEN` (`crates/squeezy-core/src/lib_tests.rs:2553-2607`) — asserts presence but not the `Authorization` slot (CFAG-1 invisible).
- `gateway_id` default to `"default"` at config-build time (`lib_tests.rs:2609-2654`); not on `from_config` path (CFAG-5).
- User-supplied `cf-aig-authorization` wins over env shortcut (`lib_tests.rs:2655-2691`).

Gaps: no end-to-end mock against either URL shape; no costly integration test (CFAG-16); no `cf-aig-gateway-id` test (CFAG-3); no `cf-aig-cache-status: HIT` test (CFAG-8); no placeholder-poisoning test (CFAG-7); no model-prefix validator test (CFAG-10).

## Verification Strategy

1. **REST API contract test**: bind a mock to a random port with `base_url` = new REST shape; fire one request; assert host `api.cloudflare.com`, path `/client/v4/accounts/{account_id}/ai/v1/chat/completions`, `cf-aig-gateway-id` present when `gateway_id` non-default, `Authorization: Bearer ${CLOUDFLARE_API_TOKEN}`, body `model` carrying upstream prefix.
2. **Legacy `/compat` test**: same mock against the legacy URL; assert `Authorization: Bearer ${UPSTREAM_KEY}` and `cf-aig-authorization: Bearer ${CF_AIG_TOKEN}`. Today's code FAILS this (CFAG-1).
3. **Placeholder hardening**: parameterized bad inputs (`acct/extra`, `g?x`, `g\ny`, `g#frag`, ` g `); each must produce `ProviderNotConfigured` (CFAG-7).
4. **Cached-stream test**: SSE script with `cf-aig-cache-status: HIT`; assert breadcrumb event + zero cost (CFAG-8).
5. **Per-request header override**: dispatch two requests through one provider with different `cf-aig-metadata` payloads; assert each wire request carries the right value (catches CFAG-4's "headers locked at construction time").
6. **Model-prefix validator**: `model = "gpt-5.5"` on `CloudflareAiGateway` must reject at config-build time with a hint to use `openai/gpt-5.5` (CFAG-10).

## References

- `/compat` endpoint (deprecated): https://developers.cloudflare.com/ai-gateway/usage/chat-completion/
- New REST API: https://developers.cloudflare.com/ai-gateway/usage/rest-api/
- REST API changelog (May 21 2026): https://developers.cloudflare.com/changelog/post/2026-05-21-rest-api/
- Authenticated Gateway: https://developers.cloudflare.com/ai-gateway/configuration/authentication/
- Header glossary: https://developers.cloudflare.com/ai-gateway/glossary/
- Caching: https://developers.cloudflare.com/ai-gateway/features/caching/
- Custom costs: https://developers.cloudflare.com/ai-gateway/configuration/custom-costs/
- Custom metadata: https://developers.cloudflare.com/ai-gateway/observability/custom-metadata/
- Fallbacks (Universal Endpoint): https://developers.cloudflare.com/ai-gateway/configuration/fallbacks/
- Universal Endpoint (deprecated): https://developers.cloudflare.com/ai-gateway/usage/universal/
- Troubleshooting: https://developers.cloudflare.com/ai-gateway/reference/troubleshooting/
- Default gateway changelog (2026-03-02): https://developers.cloudflare.com/changelog/post/2026-03-02-default-gateway/
- Workers AI OpenAI-compat: https://developers.cloudflare.com/workers-ai/configuration/open-ai-compatibility/
- Opencode peer: `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/cloudflare.ts`
- Shared aggregator audit: `/Users/abbassabra/esqueezy/squeezy/.audit/providers/openai-compatible.md`
- Preset URL template: `/Users/abbassabra/esqueezy/squeezy/crates/squeezy-core/src/lib.rs:124-127`
- Placeholder substitution: `/Users/abbassabra/esqueezy/squeezy/crates/squeezy-llm/src/compatible.rs:701-745`
- Preset config builder: `/Users/abbassabra/esqueezy/squeezy/crates/squeezy-core/src/lib.rs:8659-8725`
- Auth header injection: `/Users/abbassabra/esqueezy/squeezy/crates/squeezy-core/src/lib.rs:8693-8713`
- Substitution tests: `/Users/abbassabra/esqueezy/squeezy/crates/squeezy-llm/src/compatible_tests.rs:986-1078`
- Dual-auth tests: `/Users/abbassabra/esqueezy/squeezy/crates/squeezy-core/src/lib_tests.rs:2553-2691`
