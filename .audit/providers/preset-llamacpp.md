# llama.cpp Preset Audit

## Summary

- Severity tally: **0 critical / 4 high / 6 medium / 4 low / 3 nit** = **17 preset-specific findings** (shared-core findings tracked in `openai-compatible.md`).
- Top 3 actionable recommendations:
  1. **Drop the empty-key 401 wall** (shared X-17). `LLAMACPP_API_KEY` is a squeezy invention; llama-server runs unauthenticated unless `--api-key` is passed. `resolve_api_key_with_inline` (`crates/squeezy-llm/src/credentials.rs:45`) returns `ProviderNotConfigured` on empty, so a vanilla `provider = "llamacpp"` setup is broken until the user exports a dummy value.
  2. **Gate `stream_options.include_usage` per llama.cpp version** (LC-1). 2026 builds accept it and attach usage to the `finish_reason` chunk (issue #15443) — actually side-steps shared-core C1. Older builds and `llama-cpp-python` wrappers 400 on the field. Document the divergent terminal-chunk shape.
  3. **Document `--jinja --reasoning-format deepseek` requirement** for tool calling + reasoning. `parse_chat_event` already reads `delta.reasoning_content` (`compatible.rs:1053-1054`), but squeezy sends a `reasoning_effort` body field that llama-server silently ignores, and a `tools: [...]` request without server-side `--jinja` returns an unhinted 500.

## Verified Configuration Surface

| Aspect | squeezy value | Source | Verified |
|---|---|---|---|
| Base URL | `http://127.0.0.1:8080/v1` | `lib.rs:114` | OK (matches llama-server default port 8080 on `127.0.0.1`) |
| Default model | `""` | `lib.rs:2151` | OK (operator picks GGUF) |
| API key env | `LLAMACPP_API_KEY` | `lib.rs:2120` | invention — llama-server uses `--api-key` flag, no env convention |
| Auth header | `Authorization: Bearer <key>` | `compatible.rs:474` | OK (server also accepts `X-Api-Key`) |
| Preset key / aliases | `"llamacpp"`, `llama_cpp`, `llama_cpp_server` | `lib.rs:2013, 2178` | OK |
| Display name | `"llama.cpp server"` | `lib.rs:2038` | OK |
| `is_full_tier` | `false` | `lib.rs:2048-2059` | OK (per-host catalog) |
| `models.json` entries | 0 | `crates/squeezy-llm/src/models.json` | acceptable |
| Registry-known | yes | `registry.rs:233` | OK |
| Default headers | none | `compatible.rs:762-775` | OK |
| Default body extras | `stream_options.include_usage = true` | `compatible.rs:210` | risk — LC-PR-1 |
| Telemetry preset | `LlamaCpp` | `crates/squeezy-telemetry/src/lib.rs:1034-1035, 1074` | OK |
| TOML section | `[providers.llamacpp]` | `lib.rs:8753` | OK |

## Implementation Overview

llama.cpp routes through the shared `OpenAiCompatibleProvider`. No llama.cpp-specific code path exists beyond the `OpenAiCompatiblePreset::LlamaCpp` enum arms in `crates/squeezy-core/src/lib.rs`, one registry entry (`crates/squeezy-llm/src/registry.rs:233`), two telemetry arms (`crates/squeezy-telemetry/src/lib.rs:1034, 1074`), and PROVIDERS.md mentions.

Squeezy targets **`llama-server`** (the C++ binary from `ggml-org/llama.cpp`), not `llama-cpp-python` (port 8000, FastAPI shim with divergent semantics — see LC-PR-15). Request lifecycle is identical to every other preset: `request_body` builds `{model, messages, stream:true, stream_options:{include_usage:true}, max_tokens, tools?, tool_choice?, reasoning_effort?, reasoning?{effort}, prompt_cache_key?}`, POSTs to `{base_url}/chat/completions`, SSE chunks parsed by the shared `parse_chat_event`. None of llama.cpp's server-side flags (`--jinja`, `--reasoning-format`, `--mmproj`, `--api-key`, `--parallel`) have a squeezy-side counterpart.

## llama.cpp-Specific Findings

### LC-PR-1 (high) — `stream_options.include_usage` unconditionally emitted (LC-1)

`crates/squeezy-llm/src/compatible.rs:210` sets `stream_options.include_usage = true` for every preset. Upstream behavior (June 2026):

- Modern `llama-server` accepts it, but attaches the usage payload to the **same chunk that carries `finish_reason: "stop"`** rather than a separate trailing chunk (llama.cpp issue #15443) — diverging from OpenAI's "empty choices + usage" terminal chunk.
- Older builds (pre-b3000) reject `stream_options` with `{"error":{"code":400,"message":"unknown field stream_options"}}` — the bare `LC-1` risk.
- `llama-cpp-python` issue #1082: usage *never* emitted when streaming, even with `include_usage: true` (port 8000, mistaken target — LC-PR-15).

Because llama-server attaches usage inline with `finish_reason`, shared-core C1's "drop usage after finish_reason" bug happens to *not* fire here (usage is parsed by the same `parse_chat_event` call at `compatible.rs:1051-1132`). C1 is benign on llama.cpp. But the older-build 400 path is real — squeezy has no version probe.

**Fix**: per-preset gate (`preset_supports_include_usage`), or `[providers.llamacpp].include_usage = false` knob. Document the terminal-chunk shape difference from OpenAI canonical.

**Reference**: llama.cpp issue #15443.

### LC-PR-2 (high) — `LLAMACPP_API_KEY` is squeezy's invention; empty key blocks startup (X-17)

`crates/squeezy-core/src/lib.rs:2120` returns `"LLAMACPP_API_KEY"`. llama-server has no env-var convention — operators pass `--api-key <KEY>` at startup, and the documented default is no auth. `resolve_api_key_with_inline` (`compatible.rs:84`) errors `ProviderNotConfigured` on empty.

A user doing `llama-server -hf Qwen/Qwen3-8B-GGUF` then `squeezy --provider llamacpp --model Qwen/Qwen3-8B-GGUF` sees `ProviderNotConfigured: set LLAMACPP_API_KEY` against a happy unauthenticated server. Tracked as X-17.

**Fix**: shared X-17 — return `""` for local presets; skip `Bearer` injection when key empty. Pair with LC-PR-3's loopback gate.

### LC-PR-3 (high) — Base URL not gated to loopback when auth is empty

`check_base_url_scheme` (`crates/squeezy-core/src/lib.rs:8564`) is called for the openai-compatible arm (`lib.rs:8547-8548`) but `is_loopback_host` only requires loopback for `http://`. A user pointing `base_url = "http://gpu-cluster.internal:8080/v1"` against a non-loopback server with `--api-key` *not* configured ships prompts plaintext over HTTP with no auth.

Shared-core M5 instantiated for this preset; couples to X-17.

**Fix**: when preset is `LlamaCpp`/`LMStudio`/`VLlm` and `Bearer` is empty, hard-require loopback host (or allow-listed LAN address). Document operator responsibility for `--api-key` on non-loopback bindings.

### LC-PR-4 (high) — Tool calling needs server-side `--jinja`; squeezy gives no hint

llama-server's function calling requires `--jinja` plus a tool-aware chat template (`docs/function-calling.md`). Without `--jinja`, sending `tools: [...]` returns HTTP 500 from the Jinja renderer. Squeezy surfaces this as `ProviderRequest("llama.cpp server 500: {raw body}")` (`compatible.rs:525-527`) with no actionable hint. Base/non-tool-template models fail similarly even with `--jinja` on. Squeezy doesn't probe `GET /props` to read the loaded `chat_template`.

**Fix**: when `request.tools.is_some()` and `preset == LlamaCpp` and a 4xx/5xx body matches `"jinja"|"template"|"tool"`, append a hint pointing at `--jinja` + tool-template GGUFs. Optionally cache `/props.chat_template` after the first response and `tracing::warn!` upfront. Pairs with LC-PR-9 as a `squeezy doctor --provider llamacpp` check.

### LC-PR-5 (medium) — `reasoning_content` parsing works; `reasoning_effort` body silently dropped

`llama-server --reasoning-format deepseek` (default on reasoning templates) emits `choices[].delta.reasoning_content`. Squeezy reads it at `compatible.rs:1053-1054` via `collect_delta_text(delta.get("reasoning_content"))` — DeepSeek-R1 / Qwen3-Thinking / gpt-oss reasoning *renders* correctly.

The request side is the gap. Squeezy emits `reasoning_effort` + `reasoning: { effort }` (`compatible.rs:215-223`) unconditionally; llama-server has no on-wire reasoning-effort control (depth is set by the GGUF template or prompt — e.g. gpt-oss `<|reasoning|>low|medium|high<|/reasoning|>`). So `reasoning_effort = "high"` is silently noop on llama.cpp today, and any future strict-body validation would 4xx.

Also: the `reasoning_only_stop` notice at `compatible.rs:1106-1108` advises `tool_choice = "required"` — misdirected for a legitimate thinking-then-stop finish (same as DeepSeek DS-1, Cerebras CB-PR-3).

**Fix**: shared-core M6. Document the `--jinja --reasoning-format deepseek` server-side requirement in `PROVIDERS.md`.

### LC-PR-6 (medium) — Vision pre-flight uses generic capability table

`stream_response` at `compatible.rs:445-447` calls `request.ensure_vision_support("llamacpp")`. The capability registry has no `llamacpp` entries, so the call returns the conservative-fallback `vision: false`. A user with `--mmproj` loaded on a Gemma-4 or Qwen3-Omni checkpoint hits a `provider does not support vision` hard-fail pre-request. Shared-core M11.

**Fix**: short-circuit `ensure_vision_support` to "trust the operator" for local presets (`LlamaCpp`/`LMStudio`/`VLlm`). Or cache the multimodal flag from `GET /v1/models` (upstream docs explicitly call this out for multimodal capability probing).

### LC-PR-7 (medium) — Error `type` field dropped on the floor

llama-server error shape: `{"error":{"code":503,"message":"Loading model","type":"unavailable_error"}}`. `format_chat_error` (`compatible.rs:976-998`) reads `error.message` but drops `error.code` and `error.type` (`unavailable_error` / `invalid_request_error` / `not_supported_error`).

`unavailable_error` (503 during model load) lands as `ProviderRequest`; the retry policy (`retry.rs:46-55`) does retry 5xx so the *body* is correct, but the user sees raw JSON instead of a "model still loading" notice. `not_supported_error` is exactly LC-PR-4's tools-without-`--jinja` signal — a hint hook.

**Fix**: extend `format_chat_error` (shared-core H6) to surface `error.type`. Emit a "model still loading, retrying…" notice when `type == unavailable_error`.

### LC-PR-8 (medium) — `localhost` vs `127.0.0.1` Windows IPv6 pitfall (LM Studio F02 sibling)

`DEFAULT_LLAMACPP_BASE_URL = "http://127.0.0.1:8080/v1"` (`lib.rs:114`) is safe. But users following upstream tutorials often paste `base_url = "http://localhost:8080/v1"`. On Windows, `localhost` resolves IPv6 first; llama-server binds IPv4 → `ConnectionRefused`. Same shape as LM Studio F02.

**Fix**: detect `localhost` in `from_config`, log a `tracing::warn!` suggesting `127.0.0.1`.

### LC-PR-9 (medium) — Default model is empty; no `/v1/models` probe

`lib.rs:2151` sets `LlamaCpp => ""`. The decision is correct (operator chose the GGUF at startup), but first-run UX bites: there's no auto-discovery against `GET /v1/models` (which returns `{"data":[{"id":"<hf-repo>:<quant>", ...}]}`). LM Studio has `fetch_lmstudio_model_names`; llama.cpp has no counterpart.

**Fix**: add `fetch_llamacpp_model_names(&LlamaCppConfig)` GETting `/v1/models`. Surface in the TUI startup picker and `squeezy doctor`. Bonus: parse `/props.chat_template` for LC-PR-4 hint logic.

### LC-PR-10 (medium) — Future health probe must not send Bearer

`--api-key` keeps `/health` and `/v1/health` public (upstream issue #22474). A future squeezy doctor health-check that sends `Authorization: Bearer` will succeed even when `--api-key` is misconfigured, masking auth bugs.

**Fix**: when implementing the LC-PR-9 health probe, skip Bearer on `/health` so misconfigured auth still surfaces clearly when chat-completions fail.

### LC-PR-11 (low) — `prompt_cache_key` lines up with future slot-pinning

Squeezy emits `prompt_cache_key` (`compatible.rs:236`). llama-server's `--parallel N` + `--cont-batching` keys prefix caches per slot today; unknown body fields are dropped. If a future llama.cpp release surfaces an explicit prefix-pin API, squeezy's existing emission would line up. Subject to shared-core H8.

### LC-PR-12 (low) — Shared `reqwest::Client` keeps connection warm to a `--parallel` slot

One `reqwest::Client` per provider via `shared_client` (`transport.rs`). HTTP/1.1 keep-alive against `--parallel 1` (docs default; issue #17989 notes the binary may init 4) keeps the same TCP stream warm. Cancellation races on the SSE reader can leak the slot until the next request flushes. Shared-core L2 pattern.

### LC-PR-13 (low) — `parallel_tool_calls` ignored (shared-core M2)

`LlmRequest::parallel_tool_calls` not forwarded by chat-completions. llama-server's `--jinja` path emits multiple tool calls when the template supports it; no body knob to control. No llama.cpp-specific action.

### LC-PR-14 (low) — `response_format` / `output_schema` dropped (shared-core M3)

llama-server supports `response_format: {type:"json_schema",...}` via the GBNF bridge. Squeezy never forwards `output_schema` on chat-completions. Shared-core M3.

### LC-PR-15 (nit) — Documentation conflates `llama-server` with `llama-cpp-python`

`PROVIDERS.md:353-355` says "llama.cpp to `http://localhost:8080`" without distinguishing from `llama-cpp-python` (port 8000, FastAPI shim with different stream semantics — abetlen issue #1082, never emits usage in streams). Users following abetlen's docs configure `base_url = "http://localhost:8000/v1"` against the `llamacpp` preset and see permanent $0 cost.

**Fix**: rename PROVIDERS.md bullet to "llama-server (the C++ binary from `ggml-org/llama.cpp`)"; footnote `llama-cpp-python` users toward the `Custom` preset.

### LC-PR-16 (nit) — Telemetry tag matches preset key — OK

`crates/squeezy-telemetry/src/lib.rs:1034-1035, 1074`. Matches `as_str()`. No action.

### LC-PR-17 (nit) — Display name `"llama.cpp server"` disambiguates from `llama-cpp-python` — OK

`lib.rs:2038`. Keep.

## Catalog Verification (June 2026)

llama-server has no fixed catalog — operator picks the GGUF. Zero `llamacpp` entries in `models.json` is correct. `CONSERVATIVE_FALLBACK_CAPABILITIES` (`crates/squeezy-llm/src/model_discovery.rs:308-318`) treats unknown local models as `vision: false, tools: true` — LC-PR-6 gap.

Representative GGUF families verified working with llama-server in June 2026: Qwen3 instruct/thinking (`--jinja --reasoning-format deepseek`), DeepSeek-R1-Distill (PR #11607), gpt-oss-20b/120b (template-controlled depth, discussion #15341), Llama-3.3 / Mistral-Nemo (tools with `--jinja`), Gemma-4 vision (`--mmproj`).

## Test Coverage

No `crates/squeezy-llm/tests/llamacpp_*` files; no `compatible_tests.rs` case keyed on `OpenAiCompatiblePreset::LlamaCpp`; 0 `models.json` entries. **No coverage** — same gap as the openai-compatible aggregator table row.

## Verification Strategy

Each finding validates in ~10 min against a local binary (`llama-server -hf Qwen/Qwen3-8B-GGUF:Q8_0 --jinja --reasoning-format deepseek`):

1. **LC-PR-1**: capture SSE; assert `usage` rides the `finish_reason: "stop"` chunk and `cost.input_tokens > 0`. Then test an older build for the `stream_options` 400 path.
2. **LC-PR-2 / LC-PR-3 (X-17)**: empty `LLAMACPP_API_KEY` → `ProviderNotConfigured` today / no-auth success post-fix. Non-loopback `base_url` without auth → loopback gate triggers post-fix.
3. **LC-PR-4**: `--jinja` off + `tools` → 500 today / hinted post-fix.
4. **LC-PR-5**: Qwen3-Thinking; assert `reasoning_buf` populates from `delta.reasoning_content`; reasoning-only-stop notice suppressed.
5. **LC-PR-6**: `--mmproj` + base64 image → vision-fail today / success post-fix.
6. **LC-PR-7**: mid-startup request → "model still loading" notice post-fix.
7. **LC-PR-8**: Windows + `localhost` → `127.0.0.1` warning.
8. **LC-PR-9**: GET `/v1/models`; surface `data[].id` in the picker.

A parameterized mock harness folds the llama.cpp row in without a real server: emit usage on the `finish_reason: "stop"` chunk to catch the LC-PR-1 chunk-shape difference.

## References

- llama-server README: https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md
- Function calling (`--jinja` requirement): https://github.com/ggml-org/llama.cpp/blob/master/docs/function-calling.md
- Multimodal (`--mmproj`): https://github.com/ggml-org/llama.cpp/blob/master/docs/multimodal.md
- DeepSeek-R1 reasoning_content (PR #11607): https://app.semanticdiff.com/gh/ggml-org/llama.cpp/pull/11607/overview
- gpt-oss reasoning depth (discussion #15341): https://github.com/ggml-org/llama.cpp/discussions/15341
- `stream_options.include_usage` chunk shape (issue #15443): https://github.com/ggml-org/llama.cpp/issues/15443
- `llama-cpp-python` missing usage on streams (issue #1082): https://github.com/abetlen/llama-cpp-python/issues/1082
- `--parallel` default slot count (issue #17989): https://github.com/ggml-org/llama.cpp/issues/17989
- `--api-key` public `/health` (issue #22474): https://github.com/ggml-org/llama.cpp/issues/22474
- API key sending (discussion #9080): https://github.com/ggml-org/llama.cpp/discussions/9080
- Qwen canonical example: https://qwen.readthedocs.io/en/latest/run_locally/llama.cpp.html
- Debian llama-server manpage: https://manpages.debian.org/experimental/llama.cpp-tools/llama-server.1.en.html
- Shared-core audit (LC-1, C1, M5, M6, M11, N3): `/Users/abbassabra/esqueezy/squeezy/.audit/providers/openai-compatible.md`
- LM Studio sibling audit (F02/F05/F13/F14): `/Users/abbassabra/esqueezy/squeezy/.audit/providers/lmstudio.md`
- X-17 ticket: `/Users/abbassabra/esqueezy/squeezy/.audit/TICKETS.md:174-180`
- opencode reference (treats llama.cpp as plain OpenAI-compat): `/Users/abbassabra/esqueezy/others/opencode/packages/web/src/content/docs/providers.mdx:1282-1316`
