# xAI Grok Provider Audit

## Summary

- **Severity tally:** 2 critical / 4 high / 6 medium / 4 low / 3 nit
- **Top 3 actionable recommendations**
  1. **Refresh the xAI model registry and `is_responses_capable` predicate** — `models.json` ships only `grok-4`, `grok-4-fast-reasoning`, `grok-code-fast-1`, all of which xAI **silently retired on 2026-05-15** and rerouted to `grok-4.3` at *new* pricing. Every cost estimate squeezy quotes for an xAI session today is wrong, and the routing predicate has no entries for the current generation (`grok-4.3`, `grok-4.20-*`, `grok-build-0.1`, `grok-imagine-*`). Refresh `models.json`, extend the prefix match to cover `grok-build`, `grok-imagine`, and dotted minor versions, and add tests for `grok-4.3` / `grok-4.20-multi-agent-0309`.
  2. **Stop bypassing `XaiProvider` in tests and add a Responses-route mock** — the only integration test (`tests/xai_costly.rs`) constructs `OpenAiCompatibleProvider` directly, never invoking `is_responses_capable`. `XaiProvider::stream_response` therefore has **zero** test coverage and the costly test cannot detect a regression in the dual-route dispatcher. Add a wiremock-based unit suite that asserts `grok-4` lands on `/v1/responses` while `grok-2` lands on `/v1/chat/completions`.
  3. **Expose xAI Live Search and `reasoning_effort` defaults** — `LlmRequest` has no slot for Live Search (`search_parameters` on Chat, `web_search` hosted tool on Responses). Reasoning capability lookup at `registry.rs:capabilities_for("xai", …)` returns `reasoning_effort: false` for every shipping xAI model, which means the OpenAI Responses request body **never** sets `reasoning.summary = "auto"` and `reasoning.effort` even for `grok-4.3` (the one model that actually honors it). The user can opt into hosted web search on the underlying API but squeezy strips the tool when it comes through the agent loop.

## Implementation Overview

The xAI Grok integration lives in `crates/squeezy-llm/src/xai.rs` (83 lines). It defines a tiny `XaiProvider` struct that owns two heterogenous sub-providers — `OpenAiProvider` (Responses route) and `OpenAiCompatibleProvider` (Chat Completions route) — and dispatches each request based on the model id via the free function `is_responses_capable`. Provider construction at `xai.rs:30-37` calls `OpenAiProvider::from_xai_config` and `OpenAiCompatibleProvider::from_config` back-to-back, so the API key is resolved twice and two `Arc<dyn ApiKeySource>` are minted, but they share one `reqwest::Client` (via the process-wide `shared_client` cache in `transport.rs`).

The routing predicate at `xai.rs:59-78` lowercases the model id, strips an optional `xai/` aggregator namespace, and routes Responses for `grok-code*` (special-cased) plus any `grok-{3..9}*` model; everything else (Grok 2, grok-beta, unknown) falls back to Chat Completions. The corresponding tests at `xai_tests.rs:4-64` cover the documented Grok 2/3/4 SKUs and the namespace-prefix case, but stop short of exercising the actual HTTP dispatch.

Per-request behaviour delegates entirely to the parent OpenAI / OpenAI-compat providers. xAI itself is therefore inheriting every OpenAI-Responses request-body decision (`prompt_cache_key`, `prompt_cache_retention`, reasoning summary include) and every chat-completions decision (`reasoning_effort`, `prompt_cache_key`, `cache_control` markers). Two design choices are bespoke: (a) `OpenAiProvider::from_xai_config` at `openai.rs:88-112` intentionally ignores `OpenAiCompatibleConfig::extra_headers`, sending user-supplied headers only on the chat route; (b) the model registry pricing/capabilities at `models.json:684-771` is xAI-specific and consulted whenever the OpenAI Responses path runs against an xAI request (`openai.rs:205-207`).

## Findings

### [CRITICAL] Stale model catalog: every shipped xAI model in `models.json` was retired by xAI on 2026-05-15

- **Location**: `crates/squeezy-llm/src/models.json:683-772`
- **Observed**:
  ```
  "id": "grok-4",                "input_usd_micros_per_mtok": 3000000, "output_usd_micros_per_mtok": 15000000
  "id": "grok-4-fast-reasoning", "input_usd_micros_per_mtok": 200000,  "output_usd_micros_per_mtok": 500000
  "id": "grok-code-fast-1",      "input_usd_micros_per_mtok": 200000,  "output_usd_micros_per_mtok": 1500000
  ```
- **Issue**: per the [May 15 retirement notice](https://docs.x.ai/developers/migration/may-15-retirement) **all three** of these slugs were retired and now silently redirect to `grok-4.3` at $1.25/$2.50 per Mtok. The registry's `pricing` block therefore misreports every active xAI billing line. Worse, retired *reasoning* slugs come back from xAI mapped onto `grok-4.3` with `reasoning_effort: low`; retired *non-reasoning* slugs come back with `reasoning_effort: none`. Squeezy stores neither the redirect nor the effort, so cost estimates and reasoning telemetry diverge from the wire.
- **Impact**: every existing user pinned to `grok-4` (the registry default — `squeezy-core/src/lib.rs:89`) pays grok-4.3 pricing while the TUI cost meter shows them grok-4 pricing. `grok-4-fast-reasoning` (cited in `xai_costly.rs:14` as default model `grok-4-fast-non-reasoning`!) charges 6× the registry estimate.
- **Fix sketch**: bump the curated list to `{grok-4.3, grok-4.20-0309-reasoning, grok-4.20-0309-non-reasoning, grok-4.20-multi-agent-0309, grok-build-0.1}`. Set default model to `grok-4.3`. Add a registry-driven warning when a retired slug is requested.
- **Reference**: https://docs.x.ai/developers/migration/may-15-retirement, https://docs.x.ai/developers/models

### [CRITICAL] `is_responses_capable` will silently route the next generation of Grok IDs to the wrong route

- **Location**: `crates/squeezy-llm/src/xai.rs:59-78`
- **Observed**:
  ```rust
  let Some(rest) = id.strip_prefix("grok-") else { return false; };
  let Some(generation_char) = rest.chars().next() else { return false; };
  matches!(generation_char, '3'..='9')
  ```
- **Issue**: the matcher inspects the *first character* after `grok-`. Live xAI models today include `grok-4.3`, `grok-4.20-0309-reasoning`, `grok-4.20-multi-agent-0309`, `grok-build-0.1`, `grok-imagine-image`. `grok-4.20-…` and `grok-4.3` happen to start with `'4'` so they accidentally route correctly, but `grok-build-0.1` and `grok-imagine-image` (which xAI documents as Responses-only) start with `'b'`/`'i'` and therefore hit Chat Completions — and `/v1/chat/completions` either errors or silently downgrades for the image variants (image endpoint is `/v1/images/generations`, neither route the dispatcher considers). A future `grok-A1` or any non-numeric generation cap (e.g. `grok-omega-…`) is broken too.
- **Impact**: any user who picks `grok-build-0.1` (the current cost-optimised 256k model) gets 404'd by the chat completions stream parser; the failure surfaces as an opaque `ProviderRequest` error and the agent loop has no recovery branch.
- **Fix sketch**: replace the digit-range gate with a small allow-list of base families (`grok-4`, `grok-4.3`, `grok-4.20`, `grok-build`, `grok-code`, `grok-imagine-*` → reject from dispatcher entirely, route via dedicated image endpoint) plus an "unknown grok → Responses" default (xAI's docs treat Responses as the canonical surface as of May 2026). Add a parsing test for `grok-4.3`, `grok-4.20-multi-agent-0309`, `grok-build-0.1`.
- **Reference**: https://docs.x.ai/developers/models; opencode unconditionally routes xAI through Responses (`others/opencode/packages/core/src/plugin/provider/xai.ts:16` — `evt.language = evt.sdk.responses(...)`).

### [HIGH] Live Search (`search_parameters` / hosted `web_search` tool) is unreachable from squeezy

- **Location**: `crates/squeezy-llm/src/lib.rs:459-464`, `crates/squeezy-llm/src/compatible.rs:247-296`, `crates/squeezy-llm/src/openai.rs:218-240`
- **Observed**: `LlmToolSpec` carries only `name/description/parameters/strict`; both providers serialize every tool as `{"type":"function", …}`. There is no field for a hosted xAI tool, no place to attach `search_parameters: { mode: "auto", max_search_results, from_date, to_date, sources }`, no citation surfacing.
- **Issue**: xAI's standout feature is Live Search — it's the only OpenAI-compatible provider that ships a server-side web tool inline. Both wire forms accept it:
  - Chat Completions: top-level `search_parameters` field next to `messages`.
  - Responses: hosted `{"type":"web_search", "filters": {...}}` tool entry.
  Squeezy strips both, and any `citations` array returned by xAI ends up in the response body but is parsed neither by `parse_openai_event` nor by `parse_chat_event`.
- **Impact**: users on xAI cannot ask Grok for fresh data; teams comparing squeezy to other agents lose the xAI-native real-time browsing capability. Telemetry that should attribute cost to search lookups is missing.
- **Fix sketch**: (a) extend `LlmToolSpec` (or add a sibling `LlmHostedTool` enum) to carry `{ kind: WebSearch { filters }, … }`; (b) wire chat path to merge `search_parameters` into the body when any web-search hosted tool is present; (c) wire responses path to append the hosted tool entry; (d) extend `parse_openai_event` to surface `response.citations[]` deltas and add an `LlmEvent::Citation` event (or fold into `TextDelta` markdown).
- **Reference**: https://docs.x.ai/developers/tools/web-search, https://docs.x.ai/developers/tools/citations

### [HIGH] xAI reasoning models never get `reasoning.summary = "auto"` because the registry says `reasoning_effort: false`

- **Location**: `crates/squeezy-llm/src/openai.rs:205-217`, `crates/squeezy-llm/src/models.json:693, 723, 753`
- **Observed**:
  ```rust
  let reasoning_capable = crate::capabilities_for(provider_name, &request.model)
      .is_some_and(|caps| caps.reasoning_effort);
  if reasoning_capable || request.reasoning_effort.is_some() {
      // emits reasoning.summary = "auto" and (optionally) reasoning.effort
  }
  ```
  `models.json` sets `"reasoning_effort": false` for `grok-4`, `grok-4-fast-reasoning`, `grok-code-fast-1`.
- **Issue**: per xAI's [reasoning docs](https://docs.x.ai/docs/guides/reasoning), grok-4.3 (and the now-retired -fast-reasoning slugs that redirect into it) accept `reasoning_effort ∈ {none, low, medium, high}`. Because the registry flag is false, the Responses request body never asks for a reasoning summary; that means even when the user picks a reasoning-capable Grok variant, the `response.reasoning_summary_text.delta` stream never carries content and `ReasoningPayload` is empty. Reasoning replay across turns is broken for xAI.
- **Impact**: TUI shows blank "thinking" panel for grok-4-fast-reasoning / grok-4.3. `LlmEvent::ReasoningDone` is never emitted on the Responses path. Reasoning tokens still bill server-side but squeezy has no visibility.
- **Fix sketch**: set `reasoning_effort: true` for every xAI reasoning model in the updated catalog (`grok-4.3`, `grok-4.20-0309-reasoning`). For chat-completions route on grok-3-mini-class historical models, the chat path already emits both `reasoning_effort` shapes — that path is fine.
- **Reference**: https://docs.x.ai/docs/guides/reasoning

### [HIGH] xAI Responses route silently swallows user-supplied headers, so xAI Gateway / proxy auth breaks

- **Location**: `crates/squeezy-llm/src/openai.rs:88-112`, comment: "The `OpenAiCompatibleConfig::extra_headers` map is intentionally ignored here — those headers (HTTP-Referer, X-Title, x-portkey-*) are chat-completions aggregator concerns".
- **Issue**: the comment justifies dropping aggregator headers, but the same map is the only knob a user has to forward custom routing headers (`x-organization-id`, `x-team-id`, internal-proxy `Authorization-Bearer-Forward`, observability vendor tags such as `helicone-property-*`). Dropping them on the Responses route but honoring them on the Chat route gives an asymmetric and surprising contract — `grok-4` requests skip them, `grok-2` requests include them, and the user has no signal which is happening.
- **Impact**: enterprise deployments fronting xAI behind PortKey-Helicone-style proxies lose telemetry & access-control headers for any Grok 3+ session. Symptom: silent 401s or missing per-org cost rollup, no error trail.
- **Fix sketch**: drop the asymmetric "intentionally ignored" guard. Either honor `extra_headers` on both routes or, if some headers truly are chat-only (HTTP-Referer / X-Title for OpenRouter), partition the map by an explicit allow-list defined per preset. Same fix applies to `openai_codex` and `azure_openai` constructors that share `with_api_key_source`.

### [HIGH] Costly integration test bypasses `XaiProvider`, so the dual-route dispatcher has zero end-to-end coverage

- **Location**: `crates/squeezy-llm/tests/xai_costly.rs:18-43`
- **Observed**:
  ```rust
  let provider = OpenAiCompatibleProvider::from_config(&OpenAiCompatibleConfig {
      preset: PRESET,
      ...
  })?;
  ```
  Default model is `grok-4-fast-non-reasoning`, which `is_responses_capable` would route to Responses — but the test constructs the chat-completions provider directly, so the Responses code path is never exercised even when `XAI_API_KEY` is set.
- **Issue**: this masks routing regressions completely. A bug that 404s every Grok 4 request against `/v1/chat/completions` would pass the live integration test because the chat path *also* accepts grok-4 (xAI's chat endpoint is generation-agnostic). The test claims to validate "xai chat completions streaming" but the production code path it should validate is `XaiProvider::stream_response`.
- **Impact**: maintenance landmine — a future refactor of `is_responses_capable` that drops grok-4 from the Responses path silently passes CI, and only fails when an end user hits the agent with reasoning enabled.
- **Fix sketch**: rebuild the test on `XaiProvider::from_config` so the dispatcher is on the hot path. Add a second `#[tokio::test]` exercising a known-Responses model id (e.g. `grok-4.3` once registry is refreshed) and assert via a wiremock route which endpoint received the body.

### [MEDIUM] No mock-based unit tests for chat-completions / Responses streaming under the xAI preset

- **Location**: `crates/squeezy-llm/src/xai_tests.rs` (entire file)
- **Issue**: only the routing predicate is unit-tested. There is no equivalent of `openai_tests.rs` / `compatible_tests.rs` exercising:
  - `XaiProvider::stream_response` selecting the correct sub-provider for the request model.
  - Live Search citations in a recorded SSE replay.
  - `usage.completion_tokens_details.reasoning_tokens` surfacing into `CostSnapshot`.
  - The "Responses route drops extra_headers" behavior is silent (no test).
- **Impact**: regressions in the chat or responses path against xAI's quirks (citations, reasoning, etc.) ship without notice.
- **Fix sketch**: pick a stable SSE fixture per route — wiremock-based — and assert `LlmEvent` order, cost snapshot, and that `extra_headers` reach `/v1/chat/completions` only.
- **Reference**: `crates/squeezy-llm/src/openai_tests.rs` pattern.

### [MEDIUM] xAI's `usage.prompt_tokens_details.cached_tokens` is parsed on chat but ignored on Responses for non-OpenAI shape

- **Location**: `crates/squeezy-llm/src/openai.rs:781-784`, `crates/squeezy-llm/src/compatible.rs:1147-1151`
- **Observed**:
  - Responses path looks at `usage.input_tokens_details.cached_tokens`.
  - Chat path looks at `usage.prompt_tokens_details.cached_tokens` *or* `usage.prompt_cache_hit_tokens`.
- **Issue**: xAI's Chat Completions endpoint reports prompt cache hits at top-level `usage.cached_tokens` in some response shapes (per [chat docs](https://docs.x.ai/developers/guides/chat) examples). Neither `prompt_tokens_details` nor `prompt_cache_hit_tokens` always appears. The current parser misses it.
- **Impact**: when xAI eventually flips the wire to top-level `cached_tokens`, squeezy's cache-hit telemetry zeros out silently. Cost meter overestimates.
- **Fix sketch**: in `parse_chat_usage`, also try `usage.get("cached_tokens").and_then(Value::as_u64)` after the two existing paths. Same defensive fallback wins for DeepSeek / Groq.

### [MEDIUM] No User-Agent identifier on xAI requests — analytics + rate-limit attribution is anonymous

- **Location**: `crates/squeezy-llm/src/transport.rs:96-110`, all xAI request builders
- **Observed**: `reqwest::Client::builder()` is built with no `.user_agent(...)`; reqwest defaults to `"reqwest/<version>"`.
- **Issue**: xAI tags rate-limit buckets per API key, but vendor analytics (and partner dashboards in workspaces) use the User-Agent string to attribute traffic. Opencode/codex/grok-cli all stamp a custom UA (see opencode's `User-Agent: opencode/${InstallationVersion}` at `xai.ts:658`). Squeezy ships with no identifier. Same gap likely affects other providers but is most acute for xAI because xAI's enterprise tooling segments by integration tag.
- **Impact**: users on shared xAI keys cannot see "squeezy" line items in xAI's usage dashboard, and xAI's abuse mitigation lumps squeezy traffic into the generic-reqwest bucket. If reqwest gets rate-limited globally, squeezy gets caught.
- **Fix sketch**: add `.user_agent(format!("squeezy/{}", env!("CARGO_PKG_VERSION")))` to `build_client`. Make it a preset-aware override if xAI ever publishes a partner program (similar to `OPENAI_OPENAI` integrations).

### [MEDIUM] `OpenAiCompatiblePreset::XAi` is duplicated as `CompatFlavor::XaiCompat` with an unreachable `xai/` prefix branch

- **Location**: `crates/squeezy-llm/src/compatible.rs:397-403`
- **Observed**:
  ```rust
  CompatEntry { model_prefix: "xai/", flavor: CompatFlavor::XaiCompat, ... }
  ```
- **Issue**: a request with model id `xai/grok-4` and the XAi preset never reaches `compat_entry` because `XaiProvider::stream_response` already redirects Grok 3+ to the Responses path; the chat path is taken only for `grok-2*` style ids (no `xai/` prefix). Conversely, requests with `xai/grok-4` on a *different* preset (OpenRouter, Vercel) reach the `XaiCompat` flavor — but that flavor is descriptive only (no fields are wired). Net effect: the `XaiCompat` row exists, but no production code branches on it.
- **Impact**: dead code. Adding xAI-via-OpenRouter quirks (e.g. their `:online` suffix for routing through OpenRouter's web search) has nowhere to attach.
- **Fix sketch**: either delete the `XaiCompat` row or actually use it: when reasoning is requested and the upstream is xAI-via-OpenRouter, OpenRouter forwards reasoning summary tokens only if the body sets `transforms = ["reasoning"]`. Add that under `XaiCompat`.

### [MEDIUM] Redundant API-key resolution: `from_config` resolves the credential twice (once per sub-provider)

- **Location**: `crates/squeezy-llm/src/xai.rs:30-36`
- **Observed**:
  ```rust
  responses: OpenAiProvider::from_xai_config(config)?,
  chat: OpenAiCompatibleProvider::from_config(config)?,
  ```
- **Issue**: both constructors call `resolve_api_key_with_inline(config.api_key.as_deref(), &config.api_key_env)`, each of which re-reads credentials.json and env vars. For static API keys this is just extra I/O on provider startup; if a future xAI integration adopts OAuth (see opencode's `XaiAuthPlugin` for SuperGrok), the two clients would maintain independent refresh state and could race.
- **Impact**: small startup cost today. Pre-existing landmine for any future OAuth implementation.
- **Fix sketch**: resolve the credential once at the top of `XaiProvider::from_config`, then call `OpenAiProvider::with_api_key_source` / `OpenAiCompatibleProvider::with_api_key_source` to inject the same `Arc<dyn ApiKeySource>` into both.

### [MEDIUM] No image-generation route — `grok-imagine-*` requests go to nothing useful

- **Location**: not implemented anywhere; gap surfaced by `models.json` not listing the imagine models and `xai.rs:68` accidentally claiming `grok-code-*` ⇒ Responses but saying nothing about image.
- **Issue**: xAI's image generation lives at `/v1/images/generations` and is not addressable through either of squeezy's sub-providers. A user who configures `model = "grok-imagine-image"` hits Chat Completions (because the dispatcher's allow-list misses `i*`), gets a 404, and the error surfaces only as a generic provider request error.
- **Impact**: image-only Grok models are entirely unavailable in squeezy; no graceful error explains why.
- **Fix sketch**: in `is_responses_capable`, recognize `grok-imagine-` as a separate family and reject (return a structured `ProviderNotConfigured` error explaining squeezy doesn't yet route the image endpoint). Track image generation in a follow-up — even if not implemented, the explicit error is friendlier than a 404 from the chat parser.

### [LOW] `is_responses_capable` strips at most one aggregator prefix segment, so `openrouter/xai/grok-4` is misclassified

- **Location**: `crates/squeezy-llm/src/xai.rs:64`
- **Observed**: `let id = lower.split_once('/').map(|(_, id)| id).unwrap_or(&lower);`
- **Issue**: only one slash is consumed. Vercel AI Gateway and PortKey-with-integration namespaces sometimes layer prefixes (`vercel/xai/grok-4`, `@openrouter/xai/grok-4`). After one split, `id` becomes `xai/grok-4` which doesn't start with `grok-` so the function returns false and the chat path is taken.
- **Impact**: niche, but cleanly fixable. Users routing xAI through a multi-layer aggregator into the squeezy xAI preset (override `base_url`) get the chat fallback instead of Responses.
- **Fix sketch**: replace `split_once('/')` with `rsplit_once('/')` or with `id.rsplit('/').next().unwrap_or(&lower)` so the trailing segment is always picked, regardless of how many layers prefix it.

### [LOW] `debug_assert_eq!(config.preset, OpenAiCompatiblePreset::XAi)` is a no-op in release builds

- **Location**: `crates/squeezy-llm/src/xai.rs:31`, `crates/squeezy-llm/src/openai.rs:97`
- **Observed**: relies on `debug_assert_eq!` for the preset-shape invariant.
- **Issue**: release builds (which is what users run) skip the assertion entirely. If `provider_from_config` is ever extended to dispatch a non-xAI preset through `XaiProvider::from_config` (refactor risk), the bug surfaces as silent misrouting to the xAI base URL.
- **Impact**: nil today; brittle to future routing-table edits.
- **Fix sketch**: replace with a hard `assert_eq!` (negligible cost on a one-time constructor) or upgrade to a structured `SqueezyError::ProviderNotConfigured`.

### [LOW] Cancellation between routes: `XaiProvider` clones the `CancellationToken` straight through, no extra plumbing

- **Location**: `crates/squeezy-llm/src/xai.rs:44-50`
- **Observed**: `cancel` is forwarded directly to the sub-provider.
- **Verified ✓**: The `CancellationToken` is `Clone`/`Arc`-shaped under the hood; the dispatcher only ever picks one sub-provider per call, so cancellation cleanly cascades into either branch's `tokio::select!` arm. **No bug here, but worth noting** that there is no shared idle-timeout instrumentation between routes — the "OpenAI stream idle timeout" error string surfaces verbatim for xAI Responses, while chat shows "xAI stream idle timeout" (correct, per `compatible.rs:546-549`). Asymmetric error labels are mildly confusing.
- **Fix sketch**: thread an explicit `provider_label` through `OpenAiProvider::stream_response` so the timeout message identifies xAI (cf. how `compatible.rs` already does it via `provider_label`).

### [LOW] Documentation comment claims Grok 2 / grok-beta still answer only Chat Completions — out of date

- **Location**: `crates/squeezy-llm/src/xai.rs:6-9` and `xai_tests.rs:27-44`
- **Observed**: comments state "Grok 2 / grok-beta / grok-1 still only answer Chat Completions".
- **Issue**: per the May 15, 2026 retirement notice and current docs at https://docs.x.ai/developers/models, Grok 2 and grok-beta still appear available but the registry doesn't include them and no costly test exercises them. The doc comment continues to assume a fact pattern that's no longer testable in CI.
- **Impact**: nit. Just keep the doc honest with what's still hot.
- **Fix sketch**: drop the implicit assertion that "Grok 2 only does Chat" — note instead that the Chat fallback is *defensive* (everything xAI ships supports Chat Completions). Re-read the predicate as "default to Chat for unknown ids, opt into Responses for known generations".

### [NIT] xAI `Cargo.toml` test config lists costly test under name `xai_costly` but tests against `compatible` provider

- **Location**: `crates/squeezy-llm/Cargo.toml` (`[[test]] name = "xai_costly"`) and `crates/squeezy-llm/tests/xai_costly.rs:1`
- **Observed**: the binary is named `xai_costly` yet the body constructs `OpenAiCompatibleProvider` directly. CI artifacts and `cargo test` output label this as the xAI test even though it never touches the xAI module.
- **Fix sketch**: rename or refactor the body. Cross-cutting fix with the [HIGH] coverage gap above.

### [NIT] Comment about "providers.xai.base_url is required" error is misleading

- **Location**: `crates/squeezy-llm/src/openai.rs:98-102`
- **Observed**: the error message is "providers.xai.base_url is required for the xAI Responses route". The chat path silently accepts an empty base URL (it errors in `OpenAiCompatibleProvider::from_config` at `compatible.rs:62-69` with a different message: "providers.xai.base_url is required for the xAI preset").
- **Fix sketch**: align both error strings — only one of the two sub-providers reports first, and the difference confuses users debugging config errors. Pick one canonical phrase.

### [NIT] `xai.rs` does not list xAI in any `unused_must_use` or `dead_code` allow comment, which is fine, but file lacks an example of construction in docs

- **Location**: `crates/squeezy-llm/src/xai.rs` module docs
- **Verified ✓**: the module doc is clear about the dual-route rationale and even cites the per-startup-vs-per-request tradeoff. **No fix needed**; flagged for completeness only.

## Test Coverage Gaps

- **[HIGH][easy]** `XaiProvider::stream_response` has **no test** verifying that `request.model = "grok-4"` dispatches `POST /v1/responses` and `request.model = "grok-2"` dispatches `POST /v1/chat/completions`. Add wiremock-based unit test pair; both can mock the exact one-event SSE the code expects to terminate cleanly. **Mockable: yes**, using the `wiremock` patterns visible in `compatible_tests.rs`.
- **[HIGH][easy]** No test asserts `reasoning_effort` reaches the Responses body for a reasoning-capable Grok model. Add an integration test that asserts the body contains `reasoning.summary = "auto"` and `reasoning.effort = "high"` once the registry flag is fixed. **Mockable: yes**.
- **[MEDIUM][easy]** No test for `extra_headers` asymmetry. Add a test that captures the request to `/v1/responses` and asserts a user-supplied `helicone-property-foo` header is absent (or present, after the fix). **Mockable: yes**.
- **[MEDIUM][medium]** No test for chat-completions SSE that includes xAI-style `citations`. Capture a real xAI Live Search SSE fixture (one-shot, no key needed if recorded from the docs), replay through `parse_chat_event`, assert citation surfacing. **Mockable: yes**, but requires the citation event type to exist first.
- **[MEDIUM][easy]** No test for `usage.cached_tokens` top-level fallback in `parse_chat_usage`. Add a fixture event with `{"usage": {"cached_tokens": 42}}` and assert `cost.cached_input_tokens = Some(42)`. **Mockable: yes**.
- **[LOW][easy]** No test for the multi-segment aggregator prefix in `is_responses_capable` (`openrouter/xai/grok-4`). Trivial to add. **Mockable: yes**.
- **[LOW][easy]** No test asserts `grok-imagine-*` returns a structured error rather than 404 surfacing from `/v1/chat/completions`. Add after the routing fix. **Mockable: yes**.
- **[LOW][medium]** No test asserts `LlmEvent::ServerModel` fires when xAI redirects a retired slug to grok-4.3. Real-world this fires on every Grok 4 / Grok 3 request as of May 15, 2026. Capture a recorded SSE; assert the echo. **Mockable: yes**.

## Verification Strategy (no xAI key required)

1. **Routing predicate**: `cargo test -p squeezy-llm --lib xai_tests` — already runs without network. Extend with the missing cases above (multi-segment prefix, `grok-build-0.1`, `grok-imagine-*`).
2. **Body shape**: run `cargo test -p squeezy-llm --lib openai_tests` and `compatible_tests`. These already use synthetic `LlmRequest` to validate body JSON — add an xAI-specific case that calls `OpenAiProvider::request_body` with `provider_name = "xai"` and the model id `grok-4.3` (post-registry-fix) and asserts the `reasoning` block is present.
3. **Dispatcher end-to-end**: stand up two wiremock endpoints (`/v1/responses` and `/v1/chat/completions`), construct `XaiProvider::from_config` pointing at the mock host, fire two requests, assert which endpoint received which body. Pattern available in `crates/squeezy-llm/src/compatible_tests.rs` mock setup.
4. **Retirement / redirect telemetry**: add a synthetic SSE fixture where `response.model = "grok-4.3"` but the request asked for `grok-4`. Assert `LlmEvent::ServerModel("grok-4.3")` fires.
5. **Live Search**: once the hosted tool slot exists, replay a recorded fixture from xAI docs showing `citations` and assert they surface. No xAI key required — recorded fixtures only.
6. **Pricing drift**: assert that the registry entries match a snapshot file (e.g. `models.json` test against a generated `xai_known_models.json` lifted from xAI docs at audit time).

## References

- [xAI Models docs](https://docs.x.ai/developers/models)
- [xAI May 15, 2026 retirement notice](https://docs.x.ai/developers/migration/may-15-retirement)
- [xAI Reasoning guide](https://docs.x.ai/docs/guides/reasoning)
- [xAI Web Search / Live Search](https://docs.x.ai/developers/tools/web-search)
- [xAI Citations](https://docs.x.ai/developers/tools/citations)
- [xAI Generate Text](https://docs.x.ai/docs/guides/chat)
- [xAI Image Generation](https://docs.x.ai/docs/guides/image-generations)
- [xAI Tools overview](https://docs.x.ai/docs/guides/tools/overview)
- opencode reference: `others/opencode/packages/llm/src/providers/xai.ts`
- opencode auth (OAuth + device flow): `others/opencode/packages/opencode/src/plugin/xai.ts`
- opencode model layer (defaults to Responses): `others/opencode/packages/core/src/plugin/provider/xai.ts:16`
- opencode profile (base URL): `others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:15`
