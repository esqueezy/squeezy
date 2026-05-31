# Azure OpenAI Preset Audit

## Summary

- **Severity tally**: 2 critical / 4 high / 5 medium / 3 low / 1 nit
- **Top 3 actionable recommendations**:
  1. **Fix the default `api-version`** (`crates/squeezy-core/src/lib.rs:35`). Today it is `"v1"`, which goes onto the wire as `?api-version=v1` against `/openai/v1/responses`. Azure's docs are explicit that the v1 GA Responses endpoint requires `?api-version=preview` until that surface graduates; `v1` returns 404 / `FeatureNotFound`. The costly test (`tests/azure_openai_costly.rs:37`) and the example in the comment at `tests/azure_openai_costly.rs:26` both perpetuate the wrong value.
  2. **Stop silently dropping content-filter signal.** `from_openai_incomplete` (`crates/squeezy-llm/src/lib.rs:512-518`) maps `"content_filter"` → `StopReason::Refusal`, but Azure additionally returns a per-prompt envelope (`prompt_filter_results.content_filter_results.*` with hate / sexual / violence / self-harm / jailbreak / protected_material scores) and may HTTP-400 the whole request *before* any SSE arrives when the prompt is blocked at the input filter. The Responses provider neither parses the body filter envelope nor distinguishes input-time block from output-time block, so the agent can't tell the user *why* a refusal happened.
  3. **Add an Entra-ID auth path.** The provider unconditionally sends `api-key: …` (`crates/squeezy-llm/src/openai.rs:342-346`). Modern Azure deployments increasingly require Microsoft Entra ID / managed-identity (`Authorization: Bearer <Entra token>`); squeezy has no way to opt in without forking the provider or going through `extra_headers`, and `AzureOpenAiConfig` (`crates/squeezy-core/src/lib.rs:2258-2275`) has no `extra_headers` field, no `auth_mode` field, and no token-provider hook.

## Verified

| Item | Observed | Verified against Azure docs |
| --- | --- | --- |
| Base URL: required, no default | `DEFAULT_AZURE_OPENAI_BASE_URL = ""` (`lib.rs:34`); empty value rejected at `openai.rs:70-74` | OK |
| Auth: `api-key: <key>` | `openai.rs:342-346` | OK for key-based; **missing** Entra/Bearer |
| Env var: `AZURE_OPENAI_API_KEY` | costly test (`azure_openai_costly.rs:13`), CLI fallback (`auth.rs:48-49`), env-driven config (`lib.rs:613-616` resolves `SQUEEZY_AZURE_OPENAI_KEY` first) | Microsoft uses `AZURE_OPENAI_API_KEY` in all official samples — squeezy treats it as a *fallback*, not the primary; minor mismatch |
| `api-version` default: `"v1"` | `lib.rs:35`, costly comment `tests/azure_openai_costly.rs:26` | **Wrong for `/responses`**: requires `preview` until GA closes the Next-Gen-APIs feature flag |

## Implementation Overview

Azure OpenAI rides the same `OpenAiProvider` struct as native OpenAI; constructor is `OpenAiProvider::from_azure_config` (`crates/squeezy-llm/src/openai.rs:69-86`). It diverges from `from_config` in four ways: (a) `base_url` is required, not defaulted; (b) `api_version` is stored and concatenated as `?api-version=…` onto every `/responses` URL (`openai.rs:315-319`); (c) `deployment_name_map: BTreeMap<String, String>` rewrites the request body's `model` field (`openai.rs:329-332`) before the POST; (d) auth flips to `api-key` header via `provider_name == "azure_openai"` at request time (`openai.rs:342-346`). Config shape lives at `crates/squeezy-core/src/lib.rs:2258-2275`; env+TOML loader at `lib.rs:611-633` (accepts `"azure"` / `"azure-openai"` aliases, merges both TOML sections).

**Not supported**: Entra ID bearer auth, classic `/openai/deployments/{deployment}/responses` URL shape, `extra_headers` slot, prompt/output `content_filter_results` parsing, input-vs-output filter discrimination, `Apim-Subscription-Key`, `x-ms-*` operational headers, regional-availability gating, `cognitiveservices.azure.com` host detection, Azure-specific `store: true` default (codex sets this at `others/codex/codex-rs/core/src/client.rs:761`).

## Findings

Order: critical → nit. Shared OpenAI Responses findings (C-01..L-06 in `.audit/providers/openai.md`) — SSE heartbeats, refusal events, mid-stream retry, `response.failed` parsing, function-call argument deltas, overflow signal — **all apply to Azure too** because `from_azure_config` reuses the same `parse_openai_event` path. Not re-listed here. Aggregator findings AZ-1, AZ-2, AZ-3 from `.audit/providers/openai-compatible.md` cover the v1-vs-classic URL shape, the missing Entra-ID path, and the deployment-name-map body rewrite; expanded below where Azure-specific.

### [CRITICAL] AZ-C1 — Default `api-version=v1` 404s the `/responses` endpoint

- **Location**: `crates/squeezy-core/src/lib.rs:35` (`DEFAULT_AZURE_OPENAI_API_VERSION = "v1"`); concatenation at `crates/squeezy-llm/src/openai.rs:316-319`; costly test at `tests/azure_openai_costly.rs:26, 37`.
- **Observed**: Defaults compose `https://{resource}.openai.azure.com/openai/v1/responses?api-version=v1`.
- **Issue**: Verified May 2026: Azure's v1 GA Responses surface is gated behind the "Next-Generation APIs (v1 preview)" resource feature flag and requires `?api-version=preview`. `v1` returns 404 `FeatureNotFound`. The `openai` Python SDK's `AzureOpenAI` helper hardcodes `preview` for Responses. opencode (`others/opencode/packages/llm/src/providers/azure.ts:33,42`) and pi (`others/pi/packages/ai/src/providers/azure-openai-responses.ts:20`) default to `"v1"` — but their default URL targets chat-completions, not Responses.
- **Impact**: Out-of-the-box config fails. Every fresh Azure user hits this. The costly test passes only because the human knows to set `AZURE_OPENAI_API_VERSION=preview`.
- **Fix sketch**: Change `DEFAULT_AZURE_OPENAI_API_VERSION` to `"preview"`. Update costly-test comment. Optionally warn at `from_azure_config` when `api_version == "v1"` against `/responses`.
- **Reference**: https://learn.microsoft.com/en-us/azure/foundry/openai/how-to/responses.

### [CRITICAL] AZ-C2 — Content-filter envelope is dropped; input-time blocks surface as bare 400s

- **Location**: HTTP error at `crates/squeezy-llm/src/openai.rs:359-368`; incomplete-reason mapper at `crates/squeezy-llm/src/lib.rs:512-518`.
- **Observed**: Prompt-blocked: HTTP 400 `{"error":{"code":"content_filter","innererror":{"code":"ResponsibleAIPolicyViolation","content_filter_result":{"hate":{"filtered":true,"severity":"high"}, ...}}}}` → stringified into `ProviderRequest("400: …")`. Mid-stream block: SSE `response.incomplete` with `incomplete_details.reason == "content_filter"` → `StopReason::Refusal`, but per-category severity in `prompt_filter_results` / `content_filter_results` is discarded.
- **Issue**: Two distinct refusal shapes (prompt-blocked vs output-blocked) collapse to a single signal. Category metadata (hate / sexual / jailbreak / protected_material_code / protected_material_text) never reaches the agent. Agent loop can't tell user "prompt blocked at hate filter, severity high"; sees `provider error: 400` and retries.
- **Impact**: Blocked prompts look like flaky 400s. Agent retries until quota exhaustion.
- **Fix sketch**: When `name == "azure_openai"` and error JSON has `error.code == "content_filter"`, surface a structured refusal carrying category/severity. In `parse_openai_event` (`openai.rs:470+`) inspect `response.incomplete_details.content_filter_result` / `response.content_filter_results`.
- **Reference**: https://learn.microsoft.com/en-us/azure/foundry/openai/concepts/content-filter.

### [HIGH] AZ-H1 — `extra_headers` slot missing on `AzureOpenAiConfig`

- **Location**: struct at `crates/squeezy-core/src/lib.rs:2258-2275`; constructor at `crates/squeezy-llm/src/openai.rs:69-86`.
- **Observed**: `AzureOpenAiConfig` has only `api_key_env`, `api_key`, `base_url`, `api_version`, `deployment_name_map`, `transport`. `OpenAiCompatibleConfig` carries `extra_headers` (`lib.rs:1942`); Azure does not.
- **Issue**: Operational headers users commonly need cannot be set: `Apim-Subscription-Key` (APIM front-end), `x-ms-correlation-request-id`, `x-ms-client-request-id`, custom `User-Agent`, and — critically — `Authorization: Bearer <Entra token>`. opencode (`others/opencode/packages/llm/src/providers/azure.ts:60-72`) and pi (`others/pi/packages/ai/src/providers/azure-openai-responses.ts:236-240`) both expose pass-throughs.
- **Impact**: Deployments behind Azure API Management (Microsoft's recommended production fronting) are non-functional. No escape hatch except forking to `openai_compatible`, which then loses `deployment_name_map`.
- **Fix sketch**: Add `#[serde(default)] pub extra_headers: BTreeMap<String, String>` to `AzureOpenAiConfig`; apply before the `api-key` header so users can override `Authorization` (AZ-H2).
- **Reference**: AZ-2 in `.audit/providers/openai-compatible.md:420`.

### [HIGH] AZ-H2 — No Entra ID / managed-identity Bearer path

- **Location**: `crates/squeezy-llm/src/openai.rs:342-346`.
- **Observed**: `if provider_name == "azure_openai" { builder.header("api-key", key) } else { builder.bearer_auth(key) }`. No `auth_mode` field.
- **Issue**: Microsoft's recommended modern auth is Entra ID — managed identity or workload identity federation issues a short-lived JWT, sent as `Authorization: Bearer <jwt>` with resource `https://cognitiveservices.azure.com/.default`. Verified May 2026: official Microsoft samples now show Entra as primary. squeezy is hard-bound to the API-key path, blocked for tenants with Conditional Access policies forbidding keys.
- **Impact**: Enterprise tenants where policy forbids keys (rotation overhead, audit gaps) cannot use squeezy.
- **Fix sketch**: Add `auth_mode: Option<AzureAuthMode>` (`ApiKey` default, `EntraBearer`). For `EntraBearer`, set `Authorization: Bearer <key>`, skip `api-key`. Allow `ApiKeySource` to wrap a refreshable JWT (azidentity / IMDS hook). Interim: let `extra_headers` populate `Authorization` and skip `api-key` when key is empty.
- **Reference**: https://learn.microsoft.com/en-us/azure/ai-foundry/openai/how-to/managed-identity.

### [HIGH] AZ-H3 — Classic `/openai/deployments/{deployment}/responses` URL shape silently breaks

- **Location**: `crates/squeezy-llm/src/openai.rs:315-332`.
- **Observed**: `stream_response` always appends `/responses` to `base_url`, no fallback for the classic shape that embeds deployment in the path.
- **Issue**: Older / Azure Government / Mooncake resources still take `base_url = "https://res.openai.azure.com/openai/deployments/my-deploy"`. The provider produces `…/my-deploy/responses` but the body still carries `model: "gpt-5"` and the URL still expects dated `?api-version=2024-10-21`. Codex detects Azure hosts (`others/codex/codex-rs/codex-api/src/provider.rs:106-127`); pi normalises URLs via `normalizeAzureBaseUrl` (`others/pi/packages/ai/src/providers/azure-openai-responses.ts:168-189`). squeezy does neither.
- **Impact**: Users on Azure Government / non-public clouds following older quickstarts get 404s with no diagnostic.
- **Fix sketch**: Detect `/openai/deployments/` in `from_azure_config`, either refuse with an actionable error pointing at the v1 URL or normalise to `/openai/v1` and warn.
- **Reference**: AZ-1 in `.audit/providers/openai-compatible.md:419`.

### [HIGH] AZ-H4 — `store: true` Azure default not applied; `previous_response_id` replay breaks

- **Location**: `crates/squeezy-llm/src/openai.rs:165` and `crates/squeezy-core/src/lib.rs:807-811`.
- **Observed**: Forwards `request.store` verbatim; `store_responses` defaults to `false`. Codex sets `store: provider.is_azure_responses_endpoint()` (`others/codex/codex-rs/core/src/client.rs:761`).
- **Issue**: Azure's Responses requires `store: true` for the multi-turn `previous_response_id` flow. squeezy forwards `previous_response_id` unconditionally (`openai.rs:167-169`); with `store: false` the prior response was never retained and Azure returns `400: response not found`.
- **Impact**: Multi-turn sessions intermittently fail with "response_id not found" after retries.
- **Fix sketch**: When `provider_name == "azure_openai"` in `request_body`, default `store: true` unless the user explicitly opted out via `[model].store_responses = false`. Document ZDR caveat.
- **Reference**: `others/codex/codex-rs/core/src/client.rs:761`.

### [MEDIUM] AZ-M1 — Vision capability gate fails closed for custom deployment ids

- **Location**: `crates/squeezy-llm/src/openai.rs:308-310` → `crates/squeezy-llm/src/lib.rs:344-357`; fallback at `crates/squeezy-llm/src/registry.rs:245-258` returns `ModelCapabilities::TEXT_TOOLS` (`vision: false`, line 32).
- **Observed**: `ensure_vision_support` runs against `request.model` BEFORE deployment substitution. With `deployment_name_map` the logical id (e.g. `gpt-4o`) is checked → OK. Without the map, the user typically puts the literal deployment id in `[model].name` (the historical contract, still documented at `lib.rs:2271`) → registry miss → fallback `vision: false` → request errors out even when the underlying deployment IS gpt-4o.
- **Issue**: `models.json` carries only three Azure entries; every custom deployment id misses.
- **Impact**: Vision queries on user-named Azure deployments fail even when the underlying model supports vision.
- **Fix sketch**: When the lookup misses and `provider == "azure_openai"`, default `vision: true` in `fallback_model_info` (`registry.rs:161-168`).

### [MEDIUM] AZ-M2 — Azure prompt-caching cost claim is region-unverified

- **Location**: `crates/squeezy-llm/src/models.json:272-360` sets `prompt_caching: true` and a `cache_read_usd_micros_per_mtok` 10× cheaper than input on all three entries.
- **Observed**: squeezy emits `prompt_cache_key` + `prompt_cache_retention: "24h"` exactly as for native OpenAI (`openai.rs:170-181`).
- **Issue**: Verified May 2026: Azure prompt caching is region-gated (disabled in several EU sovereign / Mooncake / US Gov regions). The static pricing under-bills 10× when caching is silently unavailable.
- **Impact**: Cost-accounting drift in non-caching regions; eval pricing looks better than reality.
- **Fix sketch**: Document at the `AzureOpenAiConfig` docstring. When `cached_tokens == 0` for N consecutive turns despite a steady `prompt_cache_key`, `tracing::warn!` once.

### [MEDIUM] AZ-M3 — `models.json` lacks `o3`, `o4-mini`, `gpt-4o`, `gpt-4.1` entries

- **Location**: `crates/squeezy-llm/src/models.json:272-360`.
- **Observed**: Only `gpt-5.5`, `gpt-5.4-mini`, `gpt-5.4-nano`. Missing: `o3`, `o4-mini`, `gpt-4o`, `gpt-4o-mini`, `gpt-4.1`, `gpt-4.1-mini`, `gpt-4.1-nano`, `gpt-5.4-pro`.
- **Issue**: Fallback `ModelCapabilities::TEXT_TOOLS` lies about vision/reasoning; cost reads `$0`; `small_fast_model_for_provider("azure_openai")` (`lib.rs:69`) returns `gpt-5.4-nano` even when the resource doesn't have that deployment.
- **Impact**: Cost dashboards under-report; capability gating misfires (cf. AZ-M1).
- **Fix sketch**: Add entries for `o3`, `o4-mini`, `gpt-4o`, `gpt-4o-mini`, `gpt-4.1`, `gpt-5.4-pro`. Mirror native OpenAI caps, flag `effective_context_window_percent: 90` (Azure per-deployment quotas bite earlier).

### [MEDIUM] AZ-M4 — `?api-version=` concatenation does not URL-encode

- **Location**: `crates/squeezy-llm/src/openai.rs:316-319`.
- **Observed**: Direct `push_str`, no encoding, no validation; assumes `base_url` has no `?`.
- **Issue**: Typo like `api_version = "preview "` corrupts the URL; a base_url containing `?` produces double query strings.
- **Fix sketch**: Use `reqwest::Url::query_pairs_mut`; refuse `api_version` not matching `^[A-Za-z0-9._-]+$`.

### [MEDIUM] AZ-M5 — Doctor probes a `/models` endpoint Azure doesn't serve at `/openai/v1`

- **Location**: `crates/squeezy-cli/src/doctor.rs:679-710`.
- **Observed**: GETs `{base}/models?api-version=...`. Against `…/openai/v1` Azure exposes deployments under `/openai/v1/deployments`, not `/models`.
- **Impact**: False-negative health probes.
- **Fix sketch**: Probe `GET {base}/deployments?api-version=preview` (or classic `GET {base}/openai/deployments?api-version=2024-10-21`).
- **Reference**: https://learn.microsoft.com/en-us/azure/foundry/openai/reference-preview-latest.

### [LOW] AZ-L1 — Costly-test rides the wrong default `api-version`

- **Location**: `crates/squeezy-llm/tests/azure_openai_costly.rs:26, 36-37`. Falls out once AZ-C1 lands; meanwhile add an `AZURE_OPENAI_API_VERSION=preview` note in the comment.

### [LOW] AZ-L2 — `deployment_name_map` not exposed via env var

- **Location**: `crates/squeezy-core/src/lib.rs:627-630`. Loaded only from TOML. pi exposes `AZURE_OPENAI_DEPLOYMENT_NAME_MAP=k=v,k=v` (`others/pi/packages/ai/src/providers/azure-openai-responses.ts:23-42`); add an equivalent fallback.

### [LOW] AZ-L3 — Base-URL validation accepts non-Azure hostnames

- **Location**: `crates/squeezy-core/src/lib.rs:8544`. Only checks HTTP-vs-HTTPS scheme. Misconfigured `base_url = "https://attacker.example.com"` happily exfiltrates the `api-key` plus prompt. Codex enforces a suffix list (`others/codex/codex-rs/codex-api/src/provider.rs:118-126`). Warn (don't error — APIM uses custom hosts) when the suffix is none of `.openai.azure.com`, `.cognitiveservices.azure.com`, `.aoai.azure.com`, `.openai.azure.us`, `.openai.azure.cn`.

### [NIT] AZ-N1 — Doc comment on `deployment_name_map` case-sensitivity

- **Location**: `crates/squeezy-llm/src/openai.rs:142-147`. Comment overstates Azure's case-sensitivity (the SDK is case-insensitive in practice). Tighten to "match the deployment id verbatim from the resource portal".

## Catalog

Azure entries in `crates/squeezy-llm/src/models.json:272-360`: `gpt-5.5` (strong, 400k ctx), `gpt-5.4-mini` (balanced, 400k), `gpt-5.4-nano` (cheap, 400k). All three flag `vision: true`, `reasoning_tokens: true`, `prompt_caching: true`.

- **Missing**: `o3`, `o4-mini`, `gpt-4o`, `gpt-4o-mini`, `gpt-4.1`, `gpt-4.1-mini`, `gpt-4.1-nano`, `gpt-5.4-pro`. All ship on Azure per `learn.microsoft.com/en-us/azure/ai-foundry/foundry-models/concepts/models-sold-directly-by-azure`.
- **Stale**: entries don't reflect regional caching / vision limitations.
- **`metadata_source`**: points at `developers.openai.com`; for Azure entries should be `learn.microsoft.com/en-us/azure/foundry/openai/concepts/models`.

## Test Coverage Gaps

- `tests/azure_openai_costly.rs` covers: provider construction, happy-path `/responses` streaming with deployment-name passthrough — and uses the wrong default `api_version`.
- Unit tests at `openai_tests.rs:985-1043` cover only `resolve_deployment_name` substitution; nothing exercises URL composition.
- **Missing**: classic-URL handling (AZ-H3); content_filter HTTP-400 envelope + mid-stream `response.incomplete` (AZ-C2); multi-turn `previous_response_id` with `store: true` (AZ-H4); Entra Bearer path (AZ-H2); `extra_headers` pass-through (AZ-H1); vision gating against `deployment_name_map` resolved vs unmapped (AZ-M1); `?api-version=` composition with reserved characters / pre-existing query string (AZ-M4); doctor probe against a `/models`-404 host (AZ-M5).

## Verification Strategy

- **401 ping**: `curl -sS -X POST -H "api-key: BOGUS" "https://${RESOURCE}.openai.azure.com/openai/v1/responses?api-version=preview" -d '{"model":"gpt-5.5","input":[]}'` → 401 `Access denied due to invalid subscription key`. With `?api-version=v1` (squeezy default) → 404 `FeatureNotFound`, confirming AZ-C1.
- **Mock with `?api-version=` assertion**: `wiremock` listener on `/openai/v1/responses`, assert query contains `api-version=preview`. Build via `from_azure_config` with `api_version: "preview".into()`. Mirror `tests/lmstudio_mock.rs`.
- **Content-filter mock**: Feed `data: {"type":"response.incomplete","response":{"incomplete_details":{"reason":"content_filter","content_filter_result":{"hate":{"filtered":true,"severity":"high"}}}}}\n\n`; assert `Completed { stop_reason: Refusal, ... }` carries category metadata once AZ-C2 lands.
- **Classic URL**: `base_url = ".../openai/deployments/my-deploy"`; assert `from_azure_config` rejects or normalises once AZ-H3 lands.

## References

- Microsoft Learn — Azure OpenAI v1 API: https://learn.microsoft.com/en-us/azure/foundry/openai/latest
- Microsoft Learn — Responses API on Azure: https://learn.microsoft.com/en-us/azure/foundry/openai/how-to/responses
- Microsoft Learn — API version lifecycle: https://learn.microsoft.com/en-us/azure/foundry/openai/api-version-lifecycle
- Microsoft Learn — Content filtering: https://learn.microsoft.com/en-us/azure/foundry/openai/concepts/content-filter
- Microsoft Learn — Content streaming: https://learn.microsoft.com/en-us/azure/foundry/openai/concepts/content-streaming
- Microsoft Learn — Managed identity / Entra ID: https://learn.microsoft.com/en-us/azure/ai-foundry/openai/how-to/managed-identity
- Microsoft Learn — Models sold by Azure: https://learn.microsoft.com/en-us/azure/ai-foundry/foundry-models/concepts/models-sold-directly-by-azure
- Microsoft Learn — Model retirements: https://learn.microsoft.com/en-us/azure/foundry/openai/concepts/model-retirements
- codex Azure host detection: `others/codex/codex-rs/codex-api/src/provider.rs:106-127`
- codex `store: true` for Azure: `others/codex/codex-rs/core/src/client.rs:761`
- opencode Azure route config: `others/opencode/packages/llm/src/providers/azure.ts:26-46, 65-72`
- pi Azure provider (URL normalize + deployment map env): `others/pi/packages/ai/src/providers/azure-openai-responses.ts:20-42, 168-189, 195-224`
