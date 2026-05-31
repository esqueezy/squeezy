# DeepInfra Preset Audit

## Summary

- Severity tally: **0 critical / 4 high / 5 medium / 3 low / 2 nit** = **14 findings** (preset-specific; cross-cutting shared-core findings inherited from `.audit/providers/openai-compatible.md` referenced by ID — **DI-1** already captured there).
- Top 3 actionable recommendations:
  1. **Refresh the default model** at `crates/squeezy-core/src/lib.rs:107`. `meta-llama/Meta-Llama-3.1-70B-Instruct` (released 2024-07) is no longer on DeepInfra's featured-models table; the June 2026 pricing page promotes DeepSeek-V4-Flash ($0.10/$0.20 per MTok, 1M context, reasoning), Llama 4-Scout-17B ($0.08/$0.30, vision), Qwen 3.7-Max, GPT-OSS-120B. Llama 3.1-70B costs ~3.5× more, has 128k context, and no `reasoning_content`. See **DI-2**.
  2. **Add `deepinfra` to `PROVIDERS` and add `models.json` rows**. Today (a) `crates/squeezy-llm/src/registry.rs:212-237` does not list `"deepinfra"` (nor `"baseten"`), and (b) `grep -c '"provider": "deepinfra"' models.json` returns 0. Both gaps compound: vision-capable Llama 4-Scout / Qwen3.6 vision are blocked by `ensure_vision_support` (`crates/squeezy-llm/src/lib.rs:344-357`), cost telemetry stays `None`, context window falls back to 272k, and reasoning gate is decorative. See **DI-3**.
  3. **Wire DeepInfra into the CLI auth resolver + accept `DEEPINFRA_TOKEN`**. `crates/squeezy-cli/src/auth.rs:28-151` has no `section: "deepinfra"` row, and `lib.rs:2114` hard-codes only `DEEPINFRA_API_KEY` — but DeepInfra's own docs use `DEEPINFRA_TOKEN` in every code sample (LangChain uses `DEEPINFRA_API_TOKEN`; only Vercel AI SDK uses `DEEPINFRA_API_KEY`). Copy-paste from DeepInfra docs → "missing API key" with no nudge. See **DI-4**.

## Verified

- Base URL: `https://api.deepinfra.com/v1/openai` (`crates/squeezy-core/src/lib.rs:106`) — ✓ (DeepInfra docs; matches opencode `others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:9`).
- Auth: `Authorization: Bearer <key>` via `bearer_auth(key)` (`crates/squeezy-llm/src/compatible.rs:474`) — ✓.
- Env var: `DEEPINFRA_API_KEY` (`crates/squeezy-core/src/lib.rs:2114`) — ⚠ canonical is `DEEPINFRA_TOKEN` (DI-4).
- Default model: `meta-llama/Meta-Llama-3.1-70B-Instruct` (`crates/squeezy-core/src/lib.rs:107`) — shape ✓, flagship stale (DI-2).
- `reasoning_content` parsing: shared core at `crates/squeezy-llm/src/compatible.rs:1053-1054` reads `delta.reasoning_content` for DeepInfra-hosted DeepSeek-R1 / DeepSeek-V4-Pro / GPT-OSS streams. Cross-cutting **DS-1** noise misfire bites DeepInfra too (DI-7).
- 401-ping: **none.** No `verify_credentials` / `GET /v1/openai/models` probe. opencode probes per `others/opencode/packages/llm/script/setup-recording-env.ts:197-204`; squeezy does not (DI-9).
- Tool calling wire shape: shared core; DeepInfra accepts the standard `tools: [...]` body but only `tool_choice ∈ {"auto", "none"}` per docs (DI-6).

## Implementation Overview

Single-row metadata pin on `OpenAiCompatibleProvider`: enum variant `OpenAiCompatiblePreset::DeepInfra` (`crates/squeezy-core/src/lib.rs:1977`), `as_str/display_name/default_base_url/default_api_key_env/default_model/parse/section` at `:2009,:2034,:2078,:2114,:2145,:2174,:8749`; telemetry at `crates/squeezy-telemetry/src/lib.rs:1025,:1070`. `is_full_tier` is **false** (`lib.rs:2048-2059`).

**Zero** DeepInfra-specific branches in `crates/squeezy-llm/src/compatible.rs` — no entry in `preset_default_headers` (`:762-775`), no body-field branch in `request_body` (`:134-297`). DeepInfra HF org prefixes (`meta-llama/`, `deepseek-ai/`, `Qwen/`, etc.) miss `COMPAT_TABLE` (`compatible.rs:374-403`); `openai/`+`google/` prefixes match OpenRouter-shaped rows — wrong for DeepInfra (DI-11).

DeepInfra ships three surfaces on the same host — `/v1/openai/chat/completions` (squeezy's path), `/v1/openai/{embeddings,images/generations}` (Flux 1.x/2), `/v1/inference/{model_id}` (DeepInfra-native rerank/speech/classification). Squeezy reaches only the first.

## Findings

### DI-2 — Default model no-longer-flagship + no reasoning + no vision (high)

- **Location**: `crates/squeezy-core/src/lib.rs:107` (`DEFAULT_DEEPINFRA_MODEL`); also `crates/squeezy-skills/external-docs/PROVIDERS.md:34`.
- **Observed**: `meta-llama/Meta-Llama-3.1-70B-Instruct` (released 2024-07). Still listed but absent from June 2026 "Featured Models" on https://deepinfra.com/pricing. Featured lineup: DeepSeek-V4-Pro ($1.30/$2.60 + $0.10 cached), DeepSeek-V4-Flash ($0.10/$0.20 + $0.02 cached), Qwen 3.7-Max ($2.50/$7.50), Llama 4-Scout-17B ($0.08/$0.30), Gemini 3.5-Flash ($1.50/$9.00).
- **Issue**: Default lacks `reasoning_content` (squeezy's `reasoning_effort` silently dropped; spurious `reasoning_only_stop` notice can fire); lacks vision; ~3.5× pricier than V4-Flash; 128k context vs V4's 1M.
- **Fix sketch**: Rotate to `deepseek-ai/DeepSeek-V4-Flash`. Update `PROVIDERS.md:34`.

### DI-3 — Missing from `PROVIDERS` AND zero `models.json` rows (high)

- **Location**: `crates/squeezy-llm/src/registry.rs:212-237` (`PROVIDERS` constant — `deepinfra` absent, also `baseten`); `crates/squeezy-llm/src/models.json` (zero `"provider": "deepinfra"` rows).
- **Observed**: `model_info_for("deepinfra", ...)` falls through to `fallback_model_info` (`registry.rs:161-182, :249-253`): `TEXT_TOOLS`, `pricing: None`, 272k context, `vision: false`. Compound impact: (1) cost reporting → `None`; cached-token counts reach `CostSnapshot.cached_input_tokens` but produce no dollar values; (2) `ensure_vision_support` (`lib.rs:344-357`) **blocks** Llama 4-Scout / Qwen3.6 vision SKUs from images though DeepInfra docs confirm support; (3) `reasoning_effort` decorative; (4) `PROVIDERS`-iterating doctor/validators silently skip DeepInfra config sections.
- **Fix sketch**: (a) Add `"deepinfra"` and `"baseten"` to `PROVIDERS`. (b) Add ≥6 `models.json` rows (DeepSeek-V4-Flash/Pro, Llama 4-Scout-17B, Qwen3.6-35B-A3B, Qwen3.7-Max, gpt-oss-120B) with pricing from https://deepinfra.com/pricing, `vision: true` where applicable, `reasoning_tokens: true` on V4+gpt-oss, `prompt_caching: true`. (c) Promote `is_full_tier` once costly test ships.
- **Reference**: `.audit/providers/openai-compatible.md` **DI-1**.

### DI-4 — CLI auth has no `deepinfra` row; `DEEPINFRA_TOKEN` unrecognized (high)

- **Location**: `crates/squeezy-cli/src/auth.rs:28-151` (no `section: "deepinfra"` among 21 rows; baseten also missing); `crates/squeezy-core/src/lib.rs:2114` hard-codes `"DEEPINFRA_API_KEY"`.
- **Observed**: (a) `squeezy auth login deepinfra` has no resolver row. (b) DeepInfra's canonical env per https://docs.deepinfra.com/chat/overview is `DEEPINFRA_TOKEN`; LangChain uses `DEEPINFRA_API_TOKEN`; only Vercel AI SDK / Mastra use `DEEPINFRA_API_KEY`. User who copies curl from DeepInfra docs gets "DEEPINFRA_API_KEY not set" with no hint.
- **Fix sketch**: Add `KnownProvider { section: "deepinfra", cli: "deepinfra", env: "SQUEEZY_DEEPINFRA_KEY", fallback_env: Some("DEEPINFRA_API_KEY") }` (also baseten). Also check `DEEPINFRA_TOKEN` / `DEEPINFRA_API_TOKEN` as fallbacks.

### DI-5 — Tunable body fields unreachable; DeepInfra-hosted open models hit hardest (high)

- **Location**: `crates/squeezy-llm/src/compatible.rs:134-297` (`request_body`); `crates/squeezy-llm/src/lib.rs:130-176` (`LlmRequest`).
- **Observed**: DeepInfra accepts `seed`, `top_p`, `temperature`, `stop`, `frequency/presence_penalty`, `response_format` (json_object + json_schema per https://docs.deepinfra.com/chat/structured-outputs), `logprobs`. None reachable.
- **Issue**: DeepInfra-hosted open models have wildly different default sampling (Llama: `temp=0.6/top_p=0.9`; Qwen3: `0.7`; DeepSeek-V4 differs by thinking mode). Without `seed`/`temperature` reproducibility is impossible; `output_schema` set on DeepInfra silently returns markdown.
- **Fix sketch**: Cross-cutting **H3** + **M3**; no DeepInfra overlay needed.

### DI-6 — `tool_choice` only accepts `"auto"`/`"none"` per docs; squeezy forwards `"required"` blindly (medium)

- **Location**: `crates/squeezy-llm/src/compatible.rs:283-294`; DeepInfra docs at https://docs.deepinfra.com/chat/tool-calling explicitly enumerate only `"auto"` and `"none"`.
- **Observed**: Squeezy forwards `request.tool_choice` verbatim. Squeezy's own docs recommend `tool_choice = "required"` for tool-shy models.
- **Issue**: DeepInfra route with `tool_choice = "required"` may 4xx, or silently coerce to `"auto"` (lost intent). Function-pin object form also unreachable.
- **Fix sketch**: Per-preset `tool_choice` normalizer — when `preset == DeepInfra` and `tool_choice == Some("required")`, log warning + omit field. Or test against live API and update docs if DeepInfra actually accepts `"required"`.

### DI-7 — Reasoning-only-stop notice fires on legitimate DeepSeek-V4 thinking completions (medium)

- **Location**: `crates/squeezy-llm/src/compatible.rs:1094-1109`.
- **Observed**: DeepInfra-hosted DeepSeek-R1 / DeepSeek-V4-Pro / gpt-oss legitimately terminate with `finish_reason: "stop"` after streaming `reasoning_content` chunks. Shared notice misfires identically to native DeepSeek (cross-cutting **DS-1** / **H-31**). Worse on DeepInfra because the recommended `tool_choice = "required"` remediation may itself 4xx (DI-6).
- **Fix sketch**: Same as **DS-1** — suppress notice when `state.reasoning_buf` is non-empty.

### DI-8 — `prompt_cache_key` is dead weight; cache is auto-prefix (medium)

- **Location**: `crates/squeezy-llm/src/compatible.rs:225-237`.
- **Observed**: Squeezy always forwards `prompt_cache_key` when set. DeepInfra docs don't document this field; pricing implies auto-prefix-cache on V4 SKUs (V4-Pro $0.10 cached vs $1.30 miss; V4-Flash $0.02 vs $0.10; Qwen3.7-Max $0.50 vs $2.50).
- **Issue**: Harmless today; shared **H8** truncate-collision lights up if DeepInfra ever ships a cache header. Cost-panel under-reports savings until DI-3 rows include `cache_read_usd_micros_per_mtok`.
- **Fix sketch**: Lands with DI-3.

### DI-9 — No `GET /v1/openai/models` credential probe (medium)

- **Location**: `crates/squeezy-llm/src/compatible.rs:439-528` (no probe path); opencode validates per `others/opencode/packages/llm/script/setup-recording-env.ts:197-204`.
- **Observed**: First credential check is the first paid chat call. Bad keys surface as `DeepInfra 401: <body>` at `compatible.rs:525-528`.
- **Issue**: `squeezy doctor` can't validate a DeepInfra key for free; mid-rotation rollouts have no fast probe. Universal across the aggregator group but DeepInfra publishes a free `/v1/openai/models` endpoint that opencode already uses.
- **Fix sketch**: Generic `verify_credentials` method on `OpenAiCompatibleProvider` that GETs `{base_url}/models`; wire `squeezy doctor` to call it.

### DI-10 — Embeddings, image generation, DeepInfra-native routes unreachable (low)

DeepInfra exposes `/v1/openai/embeddings`, `/v1/openai/images/generations` (Flux 1.x/2), `/v1/inference/{model_id}` (rerank, speech, classification). Squeezy reaches only chat-completions. Forward-looking gap — no embeddings API in squeezy yet; image-output not modeled. Track under future multimodal / rerank-as-tool work.

### DI-11 — `COMPAT_TABLE` misroutes `openai/`+`google/` ids on DeepInfra (low)

`compatible.rs:374-403`: DeepInfra-hosted `openai/gpt-oss-120B` and `google/gemma-3-27b-it` match OpenRouter-shaped `COMPAT_TABLE` rows (`CompatFlavor::OpenAi`/`GoogleCompat`); `compat_entry` is preset-blind. Latent today (only drives `supports_cache_control`), but cross-cutting **M6** (`reasoning_effort` gate) would route via wrong flavor. Fix: make `compat_entry` preset-aware — only consult `COMPAT_TABLE` for OpenRouter/Vercel/PortKey/CloudflareAiGateway. **X-09** lookalike.

### DI-12 — Org-prefix case sensitivity has no friendly hint (low)

`compatible.rs:483-528`: DeepInfra is case-sensitive (lowercase id → 404). Fix: when 400/404 mentions "model not found", hint at https://deepinfra.com/models + case. Same shape as **FW-11**.

### DI-13 — `is_full_tier = false` is correct today, but track promotion (nit)

- **Location**: `crates/squeezy-core/src/lib.rs:2048-2059`.
- **Observed**: Correct given DI-3, but flip to `true` after registry rows + costly test ship.

### DI-14 — `PROVIDERS.md` Llama-only description is stale (nit)

- **Location**: `crates/squeezy-skills/external-docs/PROVIDERS.md:34` — current text groups DeepInfra under "Llama, …". Post-2025 lineup is DeepSeek-V4 + Llama 4 + Qwen3.6/3.7 + GPT-OSS; mention cached-input rates.

## Wire-Shape Verification (June 2026)

| Aspect | Squeezy | DeepInfra | Status |
|---|---|---|---|
| Base URL | `https://api.deepinfra.com/v1/openai` (`lib.rs:106`) | same | ✓ |
| Auth | `Bearer <key>` (`compatible.rs:474`) | same | ✓ |
| Env var | `DEEPINFRA_API_KEY` only (`lib.rs:2114`) | canonical `DEEPINFRA_TOKEN` | ⚠ DI-4 |
| Default model | `meta-llama/Meta-Llama-3.1-70B-Instruct` (`lib.rs:107`) | featured = DeepSeek-V4 / Llama 4 / Qwen3.7 | ✗ DI-2 |
| `reasoning_content` parse | yes (`compatible.rs:1053-1054`) | DeepSeek-R1/V4, gpt-oss | ✓ |
| `tool_choice` | forwarded verbatim (`compatible.rs:292-294`) | docs enum `"auto"`/`"none"` only | ⚠ DI-6 |
| `response_format: json_object` | not emitted | supported | ✗ M3 |
| `prompt_cache_key` | always emitted | not documented | ⚠ DI-8 |
| `cached_tokens` parse | yes (`compatible.rs:1147-1151`) | shipped on cache-enabled SKUs | ✓ |
| Vision | fallback `vision: false` | Llama 4-Scout / Qwen3.6 vision supported | ✗ DI-3 |
| Embeddings + image routes | not wired | both exposed | — DI-10 |
| 401 ping | not wired | `GET /v1/openai/models` works | ⚠ DI-9 |
| `is_full_tier` | `false` | promote after rows + test ship | ⚠ DI-13 |

## Test Coverage

No `deepinfra_costly.rs`, no mock, zero `models.json` rows, missing from `PROVIDERS` and `auth.rs`. Preset-specific tests once **DI-1** lands cross-cuttingly:

- **DI-T1**: Mock SSE with `delta.reasoning_content` against DeepSeek-V4-Pro → assert `ReasoningDelta { Summary }`, no spurious notice (covers DI-7 once **DS-1** fixed).
- **DI-T2**: Mock with `usage.prompt_tokens_details.cached_tokens` → assert `CostSnapshot.cached_input_tokens` round-trip + dollars (post DI-3).
- **DI-T3**: Mock 401 with DeepInfra's `{"detail": "..."}` envelope → assert `detail` surfaced (probes shared **H6**).
- **DI-T4**: `tool_choice = "auto"`/`"none"` round-trip; `"required"` omits-or-warns (DI-6).
- **DI-T5**: Costly (`SQUEEZY_RUN_COSTLY_TESTS=1`) — turn against `deepseek-ai/DeepSeek-V4-Flash`, assert `ReasoningDelta` fires and `cached_input_tokens` grows turn-2 with prefix re-use.

Endpoint snapshot (**T-55**): `("deepinfra", "https://api.deepinfra.com/v1/openai", "<refreshed-default>")`.

## References

- DeepInfra chat completions overview (June 2026): https://docs.deepinfra.com/chat/overview
- DeepInfra tool calling: https://docs.deepinfra.com/chat/tool-calling
- DeepInfra structured outputs: https://docs.deepinfra.com/chat/structured-outputs
- DeepInfra native API: https://docs.deepinfra.com/apis/deepinfra-native
- DeepInfra pricing + featured models (June 2026): https://deepinfra.com/pricing
- DeepInfra DeepSeek lineup: https://deepinfra.com/deepseek
- DeepInfra Flux image models: https://deepinfra.com/flux
- DeepInfra OpenAI SDK migration: https://deepinfra.com/blog/openai-api
- Llama 3.1 70B Instruct model card: https://deepinfra.com/meta-llama/Meta-Llama-3.1-70B-Instruct
- opencode DeepInfra profile: `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:9`
- opencode DeepInfra credential probe: `/Users/abbassabra/esqueezy/others/opencode/packages/llm/script/setup-recording-env.ts:197-204`
- opencode `@ai-sdk/deepinfra` plugin: `/Users/abbassabra/esqueezy/others/opencode/packages/core/src/plugin/provider/deepinfra.ts`
- Cross-cutting findings inherited from `.audit/providers/openai-compatible.md`: **DI-1** (no test + no models.json — captured), **H3** (sampling knobs), **H6** (error envelope flatten), **H8** (`prompt_cache_key` clamp collision), **M3** (`output_schema` → `response_format`), **M6** (gate `reasoning_effort` emission), **DS-1 / H-31** (reasoning-only-stop noise), **X-09** (preset-aware flavor gates), **T-53 / T-55** (test gaps).
