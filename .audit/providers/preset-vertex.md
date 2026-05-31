# Vertex AI Preset Audit

## Summary

Severity tally: 1 critical / 4 high / 5 medium / 4 low / 2 nit.

Top 3 actionable recommendations:

1. **Wire Vertex to a refreshing `ApiKeySource`** — today `from_config` snapshots `VERTEX_ACCESS_TOKEN` at construction (`crates/squeezy-llm/src/compatible.rs:84,93`, `crates/squeezy-llm/src/credentials.rs:446-458`). Vertex OAuth tokens live ~1 hour (Google: "service account access tokens last for 1 hour"); after that every request fails 401 and `send_with_auth_retry` (`crates/squeezy-llm/src/retry.rs:99-101`) re-reads the same stale snapshot. Either (a) re-read env on `current_key` for Vertex specifically, or (b) implement a real `ServiceAccountTokenSource` that consumes `GOOGLE_APPLICATION_CREDENTIALS` (or ADC) and refreshes against `oauth2.googleapis.com/token` with scope `https://www.googleapis.com/auth/cloud-platform`. This is VX-1 / H-28.
2. **Sunset `google/gemini-2.5-pro` default and add the global endpoint** — `DEFAULT_VERTEX_MODEL` (`crates/squeezy-core/src/lib.rs:96`) targets a model with a confirmed retirement window (Google: "retirement dates for Gemini 2.5 Pro, Gemini 2.5 Flash-Lite, and Gemini 2.5 Flash have been updated to October 16, 2026"). At the same time Gemini 3.x is only reachable through the `global` location (which has a different host: `aiplatform.googleapis.com` with no `{location}-` prefix). `vertex_base_url` (`crates/squeezy-core/src/lib.rs:133-137`) hard-codes the regional shape; passing `vertex_location = "global"` builds an invalid `https://global-aiplatform.googleapis.com/...` URL that DNS-fails.
3. **Stop fabricating the env var name; surface PROVIDERS.md drift** — `VERTEX_ACCESS_TOKEN` (`crates/squeezy-core/src/lib.rs:2109`) is a squeezy-local convention; Google's docs use `OPENAI_API_KEY="$(gcloud auth application-default print-access-token)"`. Worse: `crates/squeezy-skills/external-docs/PROVIDERS.md:215-227` promises `service_account_json = "..."` config and "Squeezy refreshes the token transparently" — neither exists in the code (`grep -r service_account_json crates/` returns zero matches). Either implement the doc'd behavior or rewrite the doc to match VX-1 reality.

## Implementation Overview

Vertex routes through `OpenAiCompatibleProvider` (`crates/squeezy-llm/src/compatible.rs:39-46`), same machinery as OpenRouter / Groq. The preset is `OpenAiCompatiblePreset::Vertex` (`crates/squeezy-core/src/lib.rs:2057,2200`); only the `base_url` synthesis and the env-var default are bespoke. Config build flow (`crates/squeezy-core/src/lib.rs:8627-8641`):

1. If `VERTEX_BASE_URL` env or `providers.vertex.base_url` TOML is set, use verbatim.
2. Otherwise resolve `project` from `VERTEX_PROJECT` → `GOOGLE_CLOUD_PROJECT` → TOML `vertex_project`. Hard error if missing.
3. Resolve `location` from `VERTEX_LOCATION` → TOML `vertex_location` → `DEFAULT_VERTEX_LOCATION = "us-central1"` (`lib.rs:95`).
4. `vertex_base_url(project.trim(), location.trim())` → `https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}/endpoints/openapi`.

`stream_response` (`compatible.rs:451`) appends `/chat/completions`. Final URL matches Google's `auth-and-credentials` and chat-completions REST docs. Path segment is `openapi` (one word), NOT `openai`. squeezy gets this right.

Auth: `from_config` resolves `VERTEX_ACCESS_TOKEN` through the standard chain (inline → `credentials.json` → env → `SQUEEZY_VERTEX_KEY` fallback per `crates/squeezy-cli/src/auth.rs:87-92` → `SQUEEZY_CREDENTIALS_JSON`), wraps in `StaticApiKey` (`crates/squeezy-llm/src/credentials.rs:427-444`), and ships as `Authorization: Bearer <token>` via `bearer_auth` (`compatible.rs:474`). No service-account JSON, no ADC, no token-refresh hook; docstring at `lib.rs:2105-2108` admits "Users either set this env var to a token they refresh themselves."

Models in `crates/squeezy-llm/src/models.json:833-892`: two rows, `google/gemini-2.5-pro` and `google/gemini-2.5-flash`, both with `prompt_caching: false`, `reasoning_tokens: false`, `reasoning_effort: false`. Costly test at `crates/squeezy-llm/tests/vertex_costly.rs` requires `VERTEX_PROJECT` + token env. `vertex` is in `is_full_tier()` (`lib.rs:2057`).

## Findings

### VX-1 [CRITICAL] OAuth access token snapshot expires ~1h; session dies (= H-28)

- **Location**: `crates/squeezy-llm/src/compatible.rs:83-94`, `crates/squeezy-llm/src/credentials.rs:446-458`, `crates/squeezy-llm/src/retry.rs:79-102`.
- **Observed**: `from_config` calls `resolve_api_key_with_inline(...).value` once at construction and wraps in `static_api_key_source(api_key, "vertex")`. `StaticApiKey::current_key` returns the cloned snapshot forever; `StaticApiKey::invalidate` is a no-op. On 401, `send_with_auth_retry` calls `source.invalidate()` then `source.current_key()` — still the stale token.
- **Issue**: Google: "service account access tokens last for 1 hour" and "after expiration, it must be refreshed." The auth-retry layer was designed for OAuth (`credentials.rs:393-397`) but Vertex is wired to the static path.
- **Impact**: Sessions exceeding the TTL hard-fail. User must kill, run `gcloud auth print-access-token`, re-export, restart. Multi-hour agent runs unusable.
- **Fix**:
  - Minimum: a `VertexTokenSource` whose `current_key()` re-reads `VERTEX_ACCESS_TOKEN` each request (cheap; captures externally-rotated `gcloud` tokens) and whose `invalidate()` clears any cache.
  - Proper: service-account JSON via `GOOGLE_APPLICATION_CREDENTIALS` → JWT → `POST oauth2.googleapis.com/token` with scope `https://www.googleapis.com/auth/cloud-platform`, cache until `expires_at - 60s`. Use `yup-oauth2` or `gcp-auth` crate.
  - Best: also probe the GKE/Cloud Run metadata server (VX-I) and Workload Identity Federation. opencode references `google-auth-library`'s `getApplicationDefault()` (`others/opencode/packages/core/src/plugin/provider/google-vertex.ts:41-55`).
- **Reference**: ticket H-28; `auth-and-credentials` docs.

### VX-A [HIGH] Default model `google/gemini-2.5-pro` is on a confirmed retirement runway

- **Location**: `crates/squeezy-core/src/lib.rs:96`, `crates/squeezy-llm/src/models.json:833-862`.
- **Observed**: `DEFAULT_VERTEX_MODEL = "google/gemini-2.5-pro"`; `models.json` row says `lifecycle: active`.
- **Issue**: Google's lifecycle docs (May 2026): "retirement dates for Gemini 2.5 Pro, Gemini 2.5 Flash-Lite, and Gemini 2.5 Flash have been updated to October 16, 2026." Gemini 3 is GA via the global endpoint (`gemini-3-pro`, `gemini-3-flash`, `gemini-3-flash-lite`).
- **Impact**: Default 404s within ~4 months. `lifecycle` field is decorative; never consulted.
- **Fix**: bump default to `google/gemini-3-pro` once added to `models.json`; add a CI lint flagging `lifecycle != "active"` rows.
- **Reference**: https://docs.cloud.google.com/vertex-ai/generative-ai/docs/learn/model-versions.

### VX-B [HIGH] No `global` location support; Gemini 3 unreachable

- **Location**: `crates/squeezy-core/src/lib.rs:133-137`.
- **Observed**: `vertex_base_url` unconditionally emits `https://{location}-aiplatform.googleapis.com/...`.
- **Issue**: Google's `global` location lives at bare `aiplatform.googleapis.com` (no `{location}-` prefix): `https://aiplatform.googleapis.com/v1/projects/{project}/locations/global/endpoints/openapi/chat/completions`. Gemini 3 models route here exclusively. `vertex_location = "global"` builds `https://global-aiplatform.googleapis.com/...` which fails DNS.
- **Impact**: Gemini 3 requires manual `base_url` override (undocumented).
- **Fix**: special-case `global` (and continental `us`/`eu` → `aiplatform.{location}.rep.googleapis.com`, see VX-J) inside `vertex_base_url`. opencode's `vertexEndpoint` helper (`google-vertex.ts:27-30,147-153`) is the reference shape.
- **Reference**: https://discuss.google.dev/t/using-vertex-ai-s-openai-compatible-endpoint-with-a-simple-agent-runtime/338152.

### VX-2 [HIGH] No Anthropic-on-Vertex routing

- **Location**: `crates/squeezy-core/src/lib.rs:133-137`.
- **Observed**: URL synthesis hard-codes `/endpoints/openapi`; no alternate path for Anthropic-on-Vertex.
- **Issue**: Claude on Vertex uses `https://{location}-aiplatform.googleapis.com/v1/projects/{p}/locations/{l}/publishers/anthropic/models/{model}:streamRawPredict`, wire shape is Anthropic Messages + `anthropic_version: vertex-2023-10-16` (NOT chat completions). Continental `us`/`eu` use `.rep.googleapis.com`. Default Vertex Claude is `claude-opus-4-7` with 1M-context (May 2026).
- **Impact**: Enterprises that chose Vertex for Anthropic governance (BAA, residency, IAM-gated billing) can't use squeezy on it.
- **Fix**: separate `AnthropicVertexProvider` reusing `AnthropicProvider` machinery with Vertex URL + ADC auth, mirroring Anthropic-on-Bedrock. opencode separates this cleanly: `GoogleVertexAnthropicPlugin` (`google-vertex.ts:108-162`).
- **Reference**: https://platform.claude.com/docs/en/build-with-claude/claude-on-vertex-ai.

### VX-C [HIGH] Gemini 2.5 reasoning passthrough silently lost; cache flags zero

- **Location**: `crates/squeezy-llm/src/models.json:837-846,867-876`, `crates/squeezy-llm/src/compatible.rs:215-224`.
- **Observed**: Both rows: `reasoning_tokens: false, reasoning_effort: false, prompt_caching: false`. `OpenAiCompatibleProvider::request_body` always emits OpenAI-style `reasoning_effort` + `reasoning.effort` regardless.
- **Issue**: Verified May 2026 — Vertex's OpenAI-compat layer forwards Gemini 2.5 thinking via `extra_body.google.thinking_config.thinking_budget` (Vertex-specific extension; LiteLLM translates this). Vertex context-caching at 10% input price covers Gemini 2.5 Pro/Flash via `cachedContent`. OpenAI's `reasoning_effort` is NOT translated to `thinkingBudget`.
- **Impact**: `--reasoning high` is a silent no-op; cached-input billing visibility lost.
- **Fix**: (a) flip registry flags to match Google's caps; (b) Vertex-specific request-body branch that translates `reasoning_effort` → `extra_body.google.thinking_config` (table from `others/pi/.../google-vertex.ts:538-568`); (c) extend `parse_chat_usage` to read Gemini's cached/thinking token shapes.
- **Reference**: https://cloud.google.com/blog/products/ai-machine-learning/vertex-ai-context-caching ; https://docs.litellm.ai/docs/providers/vertex.

### VX-D [MEDIUM] Service-account JSON / ADC flow promised in docs but absent

- **Location**: `crates/squeezy-skills/external-docs/PROVIDERS.md:212-227`.
- **Observed**: PROVIDERS.md promises `service_account_json = "/path/to/key.json"` and "Squeezy refreshes the token transparently." `grep -rn 'service_account_json\|GOOGLE_APPLICATION_CREDENTIALS\|cloud-platform' crates/` returns zero matches.
- **Issue**: Docs claim a feature that doesn't exist. Users following the doc hit VX-1's 1-hour death loop.
- **Fix**: implement service-account JSON loading (see VX-1) OR rewrite the doc to explicitly say "Squeezy does NOT refresh tokens automatically — set up a cron that refreshes via `gcloud auth print-access-token`."
- **Reference**: `PROVIDERS.md:212-227`.

### VX-3 [MEDIUM] No model-namespace validation

- **Location**: `crates/squeezy-llm/src/compatible.rs:206-208`.
- **Observed**: `model` forwarded verbatim.
- **Issue**: Vertex's OpenAI-compat demands `{publisher}/{model-id}`: `google/gemini-2.5-pro`, `meta/llama-4-maverick-17b-128e-instruct-maas`, `deepseek-ai/deepseek-v3.1-maas`, `mistralai/codestral-2405`. A user copy-pasting `gemini-2.5-pro` (no prefix) gets opaque 400. `claude-*` ids are entirely wrong for this path (see VX-2).
- **Fix**: validate `model.contains('/')` and prefix is in the known publisher set for the Vertex preset; reject `anthropic/*` with a hint pointing at VX-2.
- **Reference**: https://docs.cloud.google.com/vertex-ai/generative-ai/docs/maas/call-open-model-apis.

### VX-4 [LOW] `vertex_base_url` doesn't validate project / location grammar

- **Location**: `crates/squeezy-core/src/lib.rs:133-137`, `8640`.
- **Observed**: `format!` with no validation; config build calls `.trim()` but `vertex_base_url` is `pub` and external callers (e.g. `tests/vertex_costly.rs:33`) bypass that.
- **Issue**: Accepts `project = "foo/bar"`, `"foo?x=1"`, whitespace, non-ASCII. `format!` interpolates verbatim → broken URLs.
- **Fix**: enforce GCP project grammar (`[a-z]([-a-z0-9]{4,28}[a-z0-9])?`) and location grammar (region pattern or literal `global`/`us`/`eu`). Return `Result`.
- **Reference**: https://cloud.google.com/resource-manager/docs/creating-managing-projects.

### VX-E [MEDIUM] `VERTEX_ACCESS_TOKEN` is squeezy-invented; not Google's de-facto convention

- **Location**: `crates/squeezy-core/src/lib.rs:2109`, `crates/squeezy-cli/src/auth.rs:87-92`.
- **Observed**: `Self::Vertex => "VERTEX_ACCESS_TOKEN"`.
- **Issue**: Google's canonical recipe (verified May 2026): `export OPENAI_API_KEY="$(gcloud auth application-default print-access-token)"`. `VERTEX_ACCESS_TOKEN` matches no Google docs and no other tool. Users copying Google's quickstart end up with token in `OPENAI_API_KEY` while squeezy looks for `VERTEX_ACCESS_TOKEN`.
- **Fix**: keep `VERTEX_ACCESS_TOKEN` as primary, add fallback probes for `GOOGLE_VERTEX_ACCESS_TOKEN` and `GOOGLE_CLOUD_ACCESS_TOKEN`. Do NOT alias to `OPENAI_API_KEY` (already claimed by the OpenAI preset).
- **Reference**: https://docs.cloud.google.com/vertex-ai/generative-ai/docs/migrate/openai/auth-and-credentials.

### VX-F [MEDIUM] `vertex_location` default has no doctor check; region/model GA mismatches 404 opaquely

- **Location**: `crates/squeezy-core/src/lib.rs:95,8637-8639`.
- **Observed**: `DEFAULT_VERTEX_LOCATION = "us-central1"`; no region-model GA validation.
- **Issue**: Model availability matrix shifts (Google: "some models are only available in the global region"). Residency picks (e.g. `europe-west4`) may not GA the chosen model → opaque 404.
- **Fix**: `doctor` step hitting `GET .../publishers/google/models`, hint on mismatch.
- **Reference**: https://docs.cloud.google.com/vertex-ai/generative-ai/docs/learn/locations.

### VX-G [LOW] No mocked OAuth-refresh test; VX-1 can silently regress

- **Location**: `crates/squeezy-llm/tests/vertex_costly.rs:17-54`.
- **Observed**: one `tokio::test`, single round-trip, no 401-then-200 fixture.
- **Fix**: add `vertex_mock.rs` per the shared audit's Verification Strategy — small mock server returns 401 first, asserts `invalidate()` was called, then 200. Pair with a `RefreshableToken` test source that bumps a version counter to prove refresh ran.

### VX-H [LOW] No usage parsing for Vertex's cached/thinking token shapes

- **Location**: `crates/squeezy-llm/src/compatible.rs:1138-1164`.
- **Observed**: `parse_chat_usage` handles standard OpenAI usage fields only.
- **Issue**: Vertex's OpenAI-compat block forwards Gemini's `cachedContentTokenCount` and `thoughtsTokenCount` under non-canonical keys. Cost undercounted on thinking, overcounted on cached.
- **Fix**: probe alternate paths (`prompt_tokens_details.cached_content_token_count`, `completion_tokens_details.thinking_tokens`). pi peer: `others/pi/.../google-vertex.ts:229-247`.

### VX-I [LOW] No Workload Identity Federation / GKE metadata server hook

- **Location**: provider construction (no metadata probe).
- **Observed**: env-var only.
- **Issue**: GKE / Cloud Run users get free OAuth2 from `http://169.254.169.254/computeMetadata/v1/instance/service-accounts/default/token` with `Metadata-Flavor: Google`. Squeezy ignores it; users run sidecars to refresh `VERTEX_ACCESS_TOKEN`.
- **Fix**: probe metadata server at startup (200ms timeout) as part of the VX-1 token source. Standard ADC order: `GOOGLE_APPLICATION_CREDENTIALS` → metadata server → `gcloud` cached creds.
- **Reference**: https://docs.cloud.google.com/kubernetes-engine/docs/how-to/workload-identity.

### VX-J [LOW] No continental multi-region endpoint support (`us`, `eu`)

- **Location**: `crates/squeezy-core/src/lib.rs:133-137`.
- **Observed**: only `{region}-aiplatform.googleapis.com`.
- **Issue**: `us`/`eu` continental regions need `https://aiplatform.{location}.rep.googleapis.com/v1/projects/...` (Regional Endpoint Platform). EU-residency users get DNS-divergent host.
- **Fix**: branch on `location in {"us","eu"}` inside `vertex_base_url`. opencode reference: `google-vertex.ts:147-153`.

### VX-K [NIT] `default_base_url` comment for Vertex doesn't link to the synthesis helper

- **Location**: `crates/squeezy-core/src/lib.rs:2069-2073`.
- **Fix**: cross-reference `vertex_base_url` and `build_openai_compatible_config`'s Vertex arm at `lib.rs:8627-8641`.

### VX-L [NIT] Brand drift: "Vertex AI" vs Google's recent "Gemini Enterprise Agent Platform" rebrand

- **Issue**: Google began migrating "Vertex AI" docs to "Gemini Enterprise Agent Platform" in 2026 (URL paths like `gemini-enterprise-agent-platform/` now redirect). Either follow or document the decision to stay on the "Vertex AI" name.

## Doc-vs-Code Discrepancies

| Where | Doc claims | Code does |
|---|---|---|
| `PROVIDERS.md:215-216` | "Squeezy refreshes the token transparently" with service-account JSON | nothing reads service-account JSON; static snapshot only (VX-1, VX-D) |
| `PROVIDERS.md:223` | `# service_account_json = "/path/to/key.json"` is a valid TOML key | not parsed; `ProviderSettings` has no such field |
| `lib.rs:2105-2108` | docstring admits users self-refresh | accurate; but the 1h-death UX isn't surfaced anywhere user-visible |

## Verified Endpoint Shape (May 2026)

Confirmed against Google's `auth-and-credentials` docs and `projects.locations.endpoints.chat.completions` REST reference:

- **Regional**: `https://{location}-aiplatform.googleapis.com/v1/projects/{p}/locations/{l}/endpoints/openapi/chat/completions` — squeezy ✓
- **Global**: `https://aiplatform.googleapis.com/v1/projects/{p}/locations/global/endpoints/openapi/chat/completions` — squeezy ✗ (VX-B)
- **Continental** (`us`, `eu`): `https://aiplatform.{l}.rep.googleapis.com/v1/projects/{p}/locations/{l}/endpoints/openapi/chat/completions` — squeezy ✗ (VX-J)
- **v1 vs v1beta1**: both work; **v1 is GA** for chat completions (squeezy's choice is correct). `v1beta1` carries early-release MaaS partner models.
- **Path segment is `openapi` (one word)**, NOT `openai`. squeezy is correct.
- **Auth**: `Authorization: Bearer <oauth-token>` with scope `https://www.googleapis.com/auth/cloud-platform`, ~1h TTL.
- **Model IDs**: `{publisher}/{model-id}`. Publishers: `google`, `meta` (Llama 4 family), `deepseek-ai`, `mistralai`, `qwen`. Anthropic NOT addressable via this path (VX-2).
- **Streaming SSE**: standard OpenAI shape, `stream_options.include_usage: true` supported. Known LiteLLM-reported quirk (https://github.com/BerriAI/litellm/issues/2195): Vertex/Gemini sometimes ships `finish_reason='stop'` on every chunk — interacts badly with shared-core C1 (early `completed_emitted` loses the usage tail chunk).

## References

- Vertex AI OpenAI-compat auth: https://docs.cloud.google.com/vertex-ai/generative-ai/docs/migrate/openai/auth-and-credentials
- Vertex chat completions REST v1: https://docs.cloud.google.com/vertex-ai/generative-ai/docs/reference/rest/v1/projects.locations.endpoints.chat/completions
- Vertex chat completions REST v1beta1: https://docs.cloud.google.com/vertex-ai/docs/reference/rest/v1beta1/projects.locations.endpoints.chat/completions
- Vertex locations / global endpoint: https://docs.cloud.google.com/vertex-ai/generative-ai/docs/learn/locations
- Vertex model lifecycle (Gemini 2.5 retirement Oct 2026): https://docs.cloud.google.com/vertex-ai/generative-ai/docs/learn/model-versions
- MaaS open models on Vertex: https://docs.cloud.google.com/vertex-ai/generative-ai/docs/maas/call-open-model-apis
- Vertex context caching (cachedContent): https://cloud.google.com/blog/products/ai-machine-learning/vertex-ai-context-caching
- Claude on Vertex AI: https://platform.claude.com/docs/en/build-with-claude/claude-on-vertex-ai
- Workload Identity Federation: https://cloud.google.com/iam/docs/workload-identity-federation
- GKE Workload Identity: https://docs.cloud.google.com/kubernetes-engine/docs/how-to/workload-identity
- opencode peer (`google-vertex.ts`): `/Users/abbassabra/esqueezy/others/opencode/packages/core/src/plugin/provider/google-vertex.ts`
- pi peer (`google-vertex.ts`): `/Users/abbassabra/esqueezy/others/pi/packages/ai/src/providers/google-vertex.ts`
- Cross-references: shared OpenAI-compat audit `crates/.audit/providers/openai-compatible.md` (VX-1..VX-4 in §Per-Preset / Vertex AI). Ticket H-28 in `.audit/TICKETS.md:442-443`.
