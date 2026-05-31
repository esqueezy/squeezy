# vLLM Preset Audit

## Summary

- Severity tally: **0 critical / 2 high / 5 medium / 4 low / 1 nit** = **12 findings** (preset-specific; shared-core findings referenced by ID).
- Top 3 actionable recommendations:
  1. **Drop the empty-key requirement (X-17 / VL_VLLM-1).** `resolve_api_key_with_inline` (`crates/squeezy-llm/src/compatible.rs:84`) errors `ProviderNotConfigured` on empty key, but vLLM defaults to **no auth** unless the operator passes `--api-key` / sets `VLLM_API_KEY` on the **server**. A fresh `squeezy --provider vllm` against an out-of-the-box `vllm serve` install fails at config load even though the wire request would succeed.
  2. **Surface vLLM reasoning models cleanly (VL_VLLM-2).** `--enable-reasoning --reasoning-parser deepseek_r1` (also `qwen3`, `gpt_oss`) returns `delta.reasoning_content`, correctly accumulated by the shared parser (`compatible.rs:1053-1054`). But the spurious "model finished without emitting any content" notice (`compatible.rs:1106-1108`, ticket H-31) fires on every reasoning-only completion since vLLM legitimately ends with `finish_reason=stop` and empty `content`. Shipped today, every R1/Qwen3 turn through vLLM lands a noisy notice in the transcript.
  3. **Probe `/v1/models` for the default checkpoint (VL_VLLM-6).** `DEFAULT_VLLM_MODEL = ""` (`lib.rs:2150`) is honest — vLLM serves whatever the operator loaded — but `model = ""` 400s with no actionable hint. Auto-probe `GET /v1/models` on first turn to seed the model field.

## Verified

- **Base URL**: `http://127.0.0.1:8000/v1` (`crates/squeezy-core/src/lib.rs:113`). Verified ✓ — vLLM binds `:8000` by default; `/v1` is the OpenAI-compat prefix.
- **Auth header**: `Authorization: Bearer <key>` when set (`compatible.rs:474`). Verified ✓ — vLLM matches against the configured `--api-key`; **auth is off by default**.
- **Env var**: `VLLM_API_KEY` (`crates/squeezy-core/src/lib.rs:2119`). Verified ✓ — **this is the official vendor env var**, not a squeezy invention (corrects shared-audit N3 at `.audit/providers/openai-compatible.md:211-215`). Per vLLM security docs, the server reads `VLLM_API_KEY` and accepts the same value as `--api-key`; client-side reuse is conventional.
- **Default model**: empty string (`crates/squeezy-core/src/lib.rs:2150`). Verified ✓ — a vLLM server only serves the single model the operator passed to `vllm serve`, so no static default is correct. But see VL_VLLM-6 about empty-model 400 UX.

## Implementation Overview

vLLM is a thin metadata pin on the shared `OpenAiCompatibleProvider` (`compatible.rs:39-46`): default base URL, env var, display name, CLI/TOML alias (`lib.rs:113, 1984-1985, 2012, 2037, 2081, 2119, 2150, 2177, 2208, 8752`). CLI auth at `auth.rs:126-131`. Namespace registered in `config_schema.rs:372`, `registry.rs:232`.

No preset-specific branches anywhere — no `preset_default_headers` row, no `request_body` body-field branches, no SSE quirks. Generic `parse_chat_usage` (`compatible.rs:1138-1164`) reads OpenAI-shape `prompt_tokens` / `completion_tokens` / `prompt_tokens_details.cached_tokens` / `completion_tokens_details.reasoning_tokens`. Modern vLLM emits all four when `stream_options.include_usage = true` (set unconditionally at `compatible.rs:210`).

vLLM uniquely exposes server-side extension fields via the OpenAI SDK's `extra_body=`: `top_k`, `min_p`, `repetition_penalty`, `length_penalty`, `min_tokens`, `prompt_logprobs`, `echo`, `add_generation_prompt`, `continue_final_message`, `chat_template_kwargs` (canonical opt-in for DeepSeek/Qwen3/Kimi thinking), `guided_json` / `guided_choice` / `guided_regex` / `guided_grammar` / `guided_decoding_backend`, `stop_token_ids`, `bad_words`, `priority`, `skip_special_tokens`, `cache_salt`, `structured_outputs`, `kv_transfer_params`, `vllm_xargs`. **None** are reachable through `LlmRequest` (shared H3 / H-26) — vLLM suffers most from H3 because its differentiation vs. plain OpenAI lives almost entirely in these fields.

`is_full_tier` is false (`lib.rs:2048-2059`); registry returns fabricated `ModelInfo` — context-window guess, no pricing, no capability flags.

## Findings

### VL_VLLM-1 (high) — Empty `VLLM_API_KEY` blocks startup against no-auth vLLM

- **Location**: `compatible.rs:84`; `lib.rs:2119`.
- **Observed**: `resolve_api_key_with_inline(...)` errors `ProviderNotConfigured` when both inline and env-var lookup yield empty.
- **Issue**: vLLM ships with auth disabled. Canonical `vllm serve <model>` accepts unauthenticated requests on `/v1/*`. squeezy refuses to start unless the user invents a value (any string works since the server doesn't check it when auth is off).
- **Fix sketch**: Skip Bearer injection when key empty for `OpenAiCompatiblePreset::{LMStudio, VLlm, LlamaCpp}`. Treat empty-string keys as "no auth". Shared X-17 (`TICKETS.md:175-180`) covers this.
- **Reference**: vLLM security docs — `/v1` endpoints unauthenticated unless `--api-key` / `VLLM_API_KEY` is set server-side.

### VL_VLLM-2 (high) — Reasoning-only completions emit spurious notice every turn

- **Location**: `compatible.rs:1094-1109`.
- **Observed**: On `finish_reason="stop"` with no `content` and populated `reasoning_buf`, `parse_chat_event` injects a 250-char `[squeezy] model finished without emitting any content or tool call...` notice.
- **Issue**: vLLM's reasoning-parser path (`--reasoning-parser deepseek_r1`, also `qwen3`, `gpt_oss`) routes thinking into `delta.reasoning_content` (recently renamed to `delta.reasoning`; both accepted) and emits the answer via `delta.content`. DeepSeek-R1-Distill and Qwen3 sometimes finish reasoning-only (eval prompts, the cheap-model fast-path router squeezy ships in `crates/squeezy-agent/src/turn_router.rs`). Notice — phrased for "user expected a reply" — fires for every legitimate reasoning-only completion.
- **Impact**: Every R1 / Qwen3 / gpt-oss reasoning-only turn through self-hosted vLLM ships confusing transcript text. Compounds DS-1 (`openai-compatible.md:278-283`) and H-31 (`TICKETS.md:456-460`); vLLM hit hardest because **all** popular vLLM reasoning serves take this path.
- **Fix sketch**: When `reasoning_buf` non-empty and `saw_visible_output` false, treat as normal reasoning completion: drain reasoning, no notice.

### VL_VLLM-3 (medium) — Tool-calling silently falls back to raw text without `--enable-auto-tool-choice`

- **Location**: `compatible.rs:247-294` (tool emission); no per-preset guidance.
- **Observed**: Squeezy ships `tools: [...]` + `tool_choice` whenever `request.tools` is non-empty. vLLM accepts the body but **emits `tool_calls` only when started with `--enable-auto-tool-choice --tool-call-parser <parser>`** (`hermes` / `llama3_json` / `mistral` / `internlm`). Without those flags the model produces `content` containing a JSON-ish string; squeezy never sees `tool_calls` and surfaces the text verbatim.
- **Issue**: Tool-shy local checkpoints (Llama 3.1 8B without `llama3_json`, Mistral 7B without `mistral`) are the default vLLM target. Agent loop treats text as final answer; never re-fires.
- **Fix sketch**: When `request.tools` non-empty and `preset == VLlm`, detect tool-call-shaped JSON in first chunk's `content` and raise a structured hint about `--enable-auto-tool-choice --tool-call-parser=...`.
- **Reference**: vLLM Tool Calling docs: "Without `--enable-auto-tool-choice`, vLLM will not generate `tool_calls`, only raw text".

### VL_VLLM-4 (medium) — `chat_template_kwargs.enable_thinking` unreachable

- **Location**: `compatible.rs:134-297` (`request_body`).
- **Observed**: Reasoning-capable vLLM serves (DeepSeek V4, GLM 4.6+, Qwen3, Kimi K2.5/K2.6) gate the thinking pass on `chat_template_kwargs.enable_thinking: true`. Squeezy emits `reasoning_effort` + `reasoning: { effort }` — vLLM ignores both.
- **Issue**: Setting `reasoning_effort = "high"` pays the premium but the thinking pass never fires (chat template short-circuits). Same root cause as Baseten BT-2 (`preset-baseten.md:35-42`); vLLM is upstream.
- **Fix sketch**: When `request.reasoning_effort.is_some()` and `preset == VLlm`, also emit `chat_template_kwargs: { enable_thinking: true }`. Precedent: opencode `transform.ts:1070-1075`.

### VL_VLLM-5 (medium) — Prefix-cache hits absent from `usage` payload

- **Location**: `compatible.rs:1138-1164`.
- **Observed**: vLLM's automatic prefix caching (`--enable-prefix-caching`, on-by-default for v0.5+) hits the KV cache but does **not** emit `prompt_tokens_details.cached_tokens` in streamed `usage` (engine-internal today). `parse_chat_usage` reads that field, returns `None` for vLLM.
- **Issue**: Ledger reports `cached_input_tokens = None` even when the prefix is 99% reused. User can't tell whether `--enable-prefix-caching` is doing anything.
- **Fix sketch**: Either (a) poll vLLM's `/metrics` (`vllm:gpu_prefix_cache_hits_total` / `..._queries_total`) for a rolling hit-rate, or (b) document the limitation and wait for vLLM to expose per-request data.

### VL_VLLM-6 (medium) — `model = ""` default 400s with no hint

- **Location**: `lib.rs:2150`; `compatible.rs:445-481`.
- **Observed**: `OpenAiCompatiblePreset::VLlm::default_model() == ""`. vLLM enumerates exactly one id at `GET /v1/models`; squeezy never probes. With no `model = ...` override, body carries `{"model": ""}`; vLLM 400s with `'model' is required` — shape-identical to a typo.
- **Fix sketch**: On `from_config` for `VLlm`/`LMStudio`/`LlamaCpp`, when `model` empty, attempt `GET {base_url}/models` (short timeout, same Bearer), pick first id, stamp on resolved config. Append a startup hint when 400 says "model is required". Parallels LMStudio audit F05.

### VL_VLLM-7 (low) — `stream_options.include_usage` may 422 on ancient vLLM builds

- **Location**: `compatible.rs:210`.
- **Observed**: Shared core unconditionally sets `stream_options: { include_usage: true }`. vLLM has supported this since v0.5 (mid-2024); older installs 422.
- **Impact**: Low — <0.5 installs rare in 2026. Same shape as LC-1 (llama.cpp) and CB-2 (Cerebras); tracked under M-49/M-50.
- **Fix sketch**: Per-preset opt-out config or detect 422 + retry without `stream_options`.

### VL_VLLM-8 (low) — Images land as plain `image_url`; no `--limit-mm-per-prompt` awareness

- **Location**: `compatible.rs:202` (`chat_message`).
- **Observed**: `chat_message` emits OpenAI-shape `image_url` blocks. vLLM accepts this for vision models (LLaVA, Llama-3.2-Vision, Pixtral, Qwen2-VL/Qwen3-VL) but caps per-prompt count at the server-side `--limit-mm-per-prompt 'image=N'` (default 1).
- **Impact**: Attaching N>1 images against unflagged vLLM vision server 400s with no hint pointing at the server flag.
- **Fix sketch**: Detect 400 message ("expected at most N images") and append a hint about `--limit-mm-per-prompt`.

### VL_VLLM-9 (low) — Zero `models.json` entries

- **Location**: `crates/squeezy-llm/src/models.json` (no `vllm` namespace).
- **Observed**: `grep '"provider": "vllm"' models.json` → zero matches. `is_full_tier == false`; fabricated `ModelInfo` returned.
- **Issue**: Cost reporting always zero; context warnings absent; vision capability defaults false. Unlike other zero-entry presets, **vLLM is unbounded** — operators serve literally anything (DeepSeek V4, Qwen3-235B, Kimi K2.5, a fine-tune nobody else has). A curated catalog is a moving target.
- **Fix sketch**: Either (a) probe `/v1/models` + the model card for `context_length` (matches Ollama H-14 / `ollama.rs:139-158`), or (b) seed entries for the most-served checkpoints.

### VL_VLLM-10 (low) — `cache_salt` extension for prefix-cache isolation unreachable

- **Location**: `compatible.rs:225-237` (cache-key handling).
- **Observed**: Squeezy emits `prompt_cache_key`; vLLM treats this as a no-op (prefix cache is content-addressed by token hash). vLLM's native isolation is `extra_body={"cache_salt": "<value>"}`.
- **Impact**: Shared-tenant vLLM users leak prefix cache to other tenants (timing side-channel per vLLM RFC #16016). Niche.
- **Fix sketch**: Map `cache_spec.key` → `cache_salt` for `VLlm`. Tracked under shared H3 / H-26.

### VL_VLLM-11 (low) — No costly or mock test

- **Location**: `crates/squeezy-llm/tests/` — no `vllm_*.rs`.
- **Impact**: Shared-core regressions (C-10, H4, H-27) ship without vLLM-side check.
- **Fix sketch**: Mock-server scenario covering `reasoning_content` reasoning-only stop (catches VL_VLLM-2 + H-31), missing `prompt_tokens_details.cached_tokens` (VL_VLLM-5), and raw-text-instead-of-tool_calls (VL_VLLM-3). Slot into parameterized harness T-53 (`.audit/providers/TICKETS.md:622-629`).

### VL_VLLM-12 (nit) — Display name and config section

`"vLLM"` display name (`lib.rs:2037`) and `[providers.vllm]` section across CLI / config schema / TOML / telemetry are consistent and match docs. Nothing to fix.

## Catalog

vLLM has no "catalog" — a server serves whatever the operator loaded. The list below names the **classes of checkpoint** vLLM users most commonly serve, with the wire shape each requires:

| Checkpoint family | Tool-calling parser | Reasoning parser | Vision | In `models.json`? |
|---|---|---|---|---|
| `deepseek-ai/DeepSeek-V4-*` / `DeepSeek-R1-*` | hermes / n/a | deepseek_r1 | ✗ | ✗ |
| `Qwen/Qwen3-*` | hermes | deepseek_r1 (or qwen3) | model-dep | ✗ |
| `Qwen/Qwen2-VL-*` / `Qwen3-VL-*` | hermes | n/a | ✓ | ✗ |
| `meta-llama/Llama-3.1-*` | llama3_json | n/a | ✗ | ✗ |
| `meta-llama/Llama-3.2-*-Vision` | llama3_json | n/a | ✓ | ✗ |
| `mistralai/Mistral-*` | mistral | n/a | ✗ | ✗ |
| `openai/gpt-oss-120b` | hermes | gpt_oss | ✗ | ✗ |
| `nvidia/Nemotron-*` | hermes | deepseek_r1 | ✗ | ✗ |

vLLM has zero entries in `models.json` (VL_VLLM-9).

## Test Coverage Gaps

Nothing today. No costly test, no mock test, no fixture, no `models.json` row. vLLM rides entirely on the shared `OpenAiCompatibleProvider` test surface. Parameterized mock harness T-53 is the right vehicle. Scenarios to add:

1. `reasoning_content` reasoning-only stop (VL_VLLM-2): assert no `[squeezy] ...` notice.
2. Empty-key startup (VL_VLLM-1): assert `from_config` succeeds with no Bearer header on the wire.
3. Raw-text fallback when `auto-tool-choice` off (VL_VLLM-3): assert hint surfaces.
4. `cached_tokens` absent (VL_VLLM-5): assert `cached_input_tokens = None`, no crash.
5. `model = ""` (VL_VLLM-6): assert squeezy probes `/v1/models` or returns structured error.

## Verification Strategy

- **401-ping with auth on**: `vllm serve <model> --api-key test123`; `curl -i -H "Authorization: Bearer wrong" http://127.0.0.1:8000/v1/chat/completions ...` returns 401.
- **No-auth smoke**: `vllm serve <model>` default; `curl http://127.0.0.1:8000/v1/chat/completions -d '{"model":"<id>","messages":[...]}'` returns 200. squeezy refuses today; post VL_VLLM-1, this is the smoke test.
- **Reasoning round-trip**: `vllm serve deepseek-ai/DeepSeek-R1-Distill-Qwen-1.5B --enable-reasoning --reasoning-parser deepseek_r1`. Reasoning-eliciting prompt. Assert `ReasoningDelta` events arrive and `Completed` lands without spurious notice.
- **Tool-call flag check**: compare behavior with vs. without `--enable-auto-tool-choice --tool-call-parser hermes`.

## References

- `crates/squeezy-core/src/lib.rs:113, 1984-1985, 2012, 2037, 2081, 2119, 2150, 2177, 2208, 8752` — vLLM preset metadata.
- `crates/squeezy-llm/src/compatible.rs:134-297` (`request_body`), `:1138-1164` (`parse_chat_usage`), `:1053-1054` (reasoning_content), `:1094-1109` (reasoning-only stop notice), `:84` (key resolution) — shared touch points.
- `crates/squeezy-cli/src/auth.rs:126-131` — CLI auth scaffolding.
- `crates/squeezy-core/src/config_schema.rs:372`, `crates/squeezy-llm/src/registry.rs:232` — namespace registration.
- `.audit/providers/openai-compatible.md:372-379` — prior vLLM section (VL_VLLM-1 + N3).
- `.audit/providers/TICKETS.md:175-180` (X-17), `:456-460` (H-31).
- `.audit/providers/preset-baseten.md:35-42` (BT-2, same `chat_template_kwargs.enable_thinking` injection vLLM needs upstream).
- vLLM OpenAI-Compatible Server: https://docs.vllm.ai/en/stable/serving/openai_compatible_server/
- vLLM Reasoning Outputs: https://docs.vllm.ai/en/latest/features/reasoning_outputs/
- vLLM Tool Calling: https://docs.vllm.ai/en/latest/features/tool_calling/
- vLLM Security: https://docs.vllm.ai/en/stable/usage/security/
- vLLM Automatic Prefix Caching: https://docs.vllm.ai/en/stable/design/prefix_caching/
- vLLM Multimodal Inputs: https://docs.vllm.ai/en/stable/serving/multimodal_inputs.html
- vLLM `cache_salt` issue #16016: https://github.com/vllm-project/vllm/issues/16016
- vLLM `prompt_cache_key` vs `cache_salt` discussion #33264: https://github.com/vllm-project/vllm/issues/33264
