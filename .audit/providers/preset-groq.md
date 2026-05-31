# Groq Preset Audit

## Summary

- Severity tally: **2 critical / 5 high / 7 medium / 4 low / 2 nit** = **20 findings** (preset-only; shared findings tracked in `openai-compatible.md`).
- Top 3 actionable recommendations:
  1. **Replace the dead `moonshotai/kimi-k2-instruct` registry row** (`crates/squeezy-llm/src/models.json:654-682`). Groq deprecated the un-dated alias 2025-09-10, then deprecated `kimi-k2-instruct-0905` on 2026-03-23 (retire 2026-04-15) for `openai/gpt-oss-120b`. The bundled entry points at a dead SKU; the catalog ships zero rows for the actual flagships (`gpt-oss-{120,20}b`, `llama-4-scout`, `qwen-3-32b`). See **GQ-PR-1**.
  2. **Land shared-core C1 for Groq** — Groq emits the trailing `usage` chunk after `finish_reason: "stop"`, and C1 (`compatible.rs:563-580`) returns before parsing it. Every Groq turn today reports $0 cost. See **GQ-PR-3**.
  3. **Plumb `reasoning_format` and `include_reasoning`** through `LlmRequest`, and gate `reasoning_effort` to reasoning models. `gpt-oss-*` rejects `reasoning_format` (400 if both arrive); the shipped body builder cannot express either of the two visibility knobs users actually need. See **GQ-PR-7**.

## Verified Configuration Surface

| Aspect | squeezy value | Source (file:line) | Verified |
|---|---|---|---|
| Base URL | `https://api.groq.com/openai/v1` | `crates/squeezy-core/src/lib.rs:86` | ✓ |
| Default model | `llama-3.3-70b-versatile` | `crates/squeezy-core/src/lib.rs:87` | ✓ (active; production-tier, 128K ctx) |
| API key env | `GROQ_API_KEY` | `crates/squeezy-core/src/lib.rs:2102` | ✓ |
| Auth header | `Authorization: Bearer <key>` | `crates/squeezy-llm/src/compatible.rs:474` | ✓ |
| Preset key / alias | `"groq"` (single canonical) | `crates/squeezy-core/src/lib.rs:2001, 2166` | ✓ |
| Display name | `"Groq"` | `crates/squeezy-core/src/lib.rs:2026` | ✓ |
| `is_full_tier` | `true` | `crates/squeezy-core/src/lib.rs:2054` | ✓ |
| `models.json` entries | 3 (`llama-3.3-70b-versatile`, `llama-3.1-8b-instant`, `moonshotai/kimi-k2-instruct`) | `crates/squeezy-llm/src/models.json:594-682` | ✗ — 1 of 3 retired; flagship `gpt-oss-*` missing |
| Registry-known | yes | `crates/squeezy-llm/src/registry.rs:223` | ✓ |
| Default headers | none | (no preset branch) | ✓ |
| Default body extras | none beyond shared `stream_options.include_usage: true` | `crates/squeezy-llm/src/compatible.rs:134-297` | ✓ |
| Tool-fallback note | acknowledges Groq 4xx on unknown tools | `crates/squeezy-llm/src/model_discovery.rs:303` | ✓ |
| Costly test default model | `llama-3.1-8b-instant` | `crates/squeezy-llm/tests/groq_costly.rs:14-15` | ✓ |
| Telemetry preset | `Groq` | `crates/squeezy-telemetry/src/lib.rs:1017, 1062` | ✓ |

## Implementation Overview

Groq is a thin preset on top of `OpenAiCompatibleProvider`; no Groq-specific code path in `compatible.rs` beyond enum routing. Preset configuration is exhausted by the constants above plus the `Groq` arms (six in `crates/squeezy-core/src/lib.rs`, one in `registry.rs:223`, two in `squeezy-telemetry/src/lib.rs`). Docs: `crates/squeezy-skills/external-docs/PROVIDERS.md:252-258`. Groq's LPU stream batches multiple tokens per SSE event, identically stressing shared-core C1 (trailing usage chunk lost) and L4 (joined `[DONE]`). The single costly test (`tests/groq_costly.rs`) hits `llama-3.1-8b-instant` for an echo and catches neither.

## Preset-Specific Findings

### GQ-PR-1 (critical) — `moonshotai/kimi-k2-instruct` row retired; flagship `gpt-oss-*` rows missing

`crates/squeezy-llm/src/models.json:653-682` lists `moonshotai/kimi-k2-instruct` (profile `strong`, $1.00/$3.00 per MTok, 131072 ctx). Groq deprecated bare `kimi-k2-instruct` on **2025-09-10** for `kimi-k2-instruct-0905`, which Groq itself deprecated **2026-03-23** (retire **2026-04-15**) in favor of `openai/gpt-oss-120b`. A `--provider groq --model moonshotai/kimi-k2-instruct` invocation 400s today; the row also shadows discovery so `squeezy doctor` claims curated coverage where Groq's `/models` returns nothing.

Flagships actually served today carry **zero** `models.json` rows: `openai/gpt-oss-120b` (production, 131K ctx, tools + parallel + reasoning + prompt caching), `openai/gpt-oss-20b` (same, lower price), `meta-llama/llama-4-scout-17b-16e-instruct` (production, 128K ctx, vision, tools), `qwen-3-32b` (production, 131K ctx, tools, `reasoning_format`). `is_full_tier=true` (`lib.rs:2054`) promises curated models; 1/3 rows is dead and none match the flagship tier.

**Fix**: drop the `kimi-k2-instruct` row (or alias it to `gpt-oss-120b`). Add curated rows for the four flagships with `reasoning_tokens=true`, `reasoning_effort=true`, `prompt_caching=true` on the `gpt-oss-*` pair and `vision=true` on `llama-4-scout`. Update `PROVIDERS.md`.

### GQ-PR-2 (critical) — `vision: false` on every Groq row blocks Llama-4-Scout image inputs

`request.ensure_vision_support(self.preset.as_str())` (`compatible.rs:445-447`) calls `capabilities_for("groq", &model)` (`crates/squeezy-llm/src/lib.rs:344-355`). All three Groq rows have `vision: false` (`models.json:601, :631, :661`). `meta-llama/llama-4-scout-17b-16e-instruct` accepts up to 5 images per turn per Groq's vision docs, but attaching an image errors at the adapter (`model ... does not support image inputs (capabilities.vision = false)`, `lib.rs:354-356`) before reaching the wire. Same root cause as shared-core M11, lands at preset level today.

**Fix**: GQ-PR-1's new `llama-4-scout` row carries `vision: true`.

### GQ-PR-3 (high) — Shared-core C1 lands hard on Groq (cost = $0)

Groq emits the final usage chunk **after** the chunk carrying `finish_reason: "stop"`, gated on `stream_options.include_usage: true` (unconditionally set at `compatible.rs:210`). Per shared-core C1 (`compatible.rs:563-580`), the outer loop returns once `state.completed_emitted` flips inside `parse_chat_event`'s `"stop"` arm (`compatible.rs:1081-1109`), so the usage-only chunk never reaches `parse_chat_event` (`compatible.rs:1044-1046`). `state.cost` stays zero on every Groq turn — cost telemetry, per-turn budget enforcement, and the cheap-model routing in `per-turn-model-routing` (current branch) all observe 0/0 tokens. Codex and opencode parse-then-complete on `[DONE]`, not on `finish_reason`.

**Fix**: shared-core C1. No Groq-specific work.

### GQ-PR-4 (high) — `tool_choice` cannot pin to a specific function

Groq accepts `tool_choice ∈ {"none","auto","required",{"type":"function","function":{"name":"<id>"}}}`. Squeezy's `LlmRequest::tool_choice: Option<String>` (forwarded verbatim at `compatible.rs:292-294`) can only carry the three string variants; the explicit-function object is unreachable from TOML. Same shape as shared-core GQ-3. Steering `gpt-oss-120b` and `llama-4-scout` into a known tool is the documented Groq pattern for skipping conversational preambles on tool-shy turns.

**Fix**: widen `LlmRequest::tool_choice` to accept a struct variant `{name: String}` and serialize to the documented object form.

### GQ-PR-5 (high) — `parallel_tool_calls` not forwarded; Groq advertises it by default

Groq documents `parallel_tool_calls` (bool, default `true`), honored across the tool-supporting catalog. `LlmRequest::parallel_tool_calls` (`crates/squeezy-llm/src/lib.rs:166`) exists but chat-completions ignores it (shared-core M2). A `[model] parallel_tool_calls = false` in `squeezy.toml` is silently dropped — `gpt-oss-120b` agent loops that need serialized tool execution to preserve state-machine invariants cannot opt out.

**Fix**: shared-core M2.

### GQ-PR-6 (high) — `service_tier` unreachable; flex tier blocked

Groq's chat-completions accepts `service_tier ∈ {"on_demand","flex","auto"}` (defaults `on_demand`). `flex` is a paid-tier opt-in with 10× higher rate limits and quick-fail status `498 capacity_exceeded`. `LlmRequest` carries no `service_tier`, and `request_body` (`compatible.rs:134-297`) has no escape hatch (shared-core H3). Users on Groq's paid tier cannot route squeezy traffic into the higher-throughput pool, and the `498` short-circuit has no special-case handler (`format_chat_error` at `compatible.rs:976-998` falls through to `default_message`).

**Fix**: extend `LlmRequest` with `service_tier: Option<&'static str>`; forward when set; add a `498`-class error formatter.

### GQ-PR-7 (high) — `reasoning_format` / `include_reasoning` missing; `reasoning_effort` partially wrong-shape

Groq's reasoning docs:

- `reasoning_format ∈ {"hidden","raw","parsed"}` on Qwen / DeepSeek-R1-distilled. Mutually exclusive with `include_reasoning`.
- `include_reasoning: bool` on `gpt-oss-{20,120}b` (default `true`). `gpt-oss-*` rejects `reasoning_format` (400 if both arrive). When JSON mode or tool use is on, Groq forces `parsed` and **400s if `raw` is explicitly set**.
- `reasoning_effort ∈ {"low","medium","high"}` honored by `gpt-oss-*`.

Squeezy at `compatible.rs:215-223` always emits `reasoning_effort` + `reasoning: { effort }`. The top-level field works on `gpt-oss-*`; the nested form is OpenRouter-shaped and undocumented by Groq, but Groq silently ignores unknown fields so no 400 today. Gaps:

1. No way to set `reasoning_format` on Qwen/DeepSeek-distilled — users can't choose `parsed` vs `hidden` for transcript UX.
2. No way to set `include_reasoning=false` on `gpt-oss-*` — reasoning tokens count against output billing and can't be suppressed.
3. The `reasoning_only_stop` notice at `compatible.rs:1106-1108` recommends `tool_choice = "required"` — same misadvice as DS-1 lands on Groq `gpt-oss-*` thinking-only turns.

**Fix**: add `LlmRequest::reasoning_format` and `include_reasoning`. Emit per-family: `gpt-oss-*` → `include_reasoning`; Qwen/DeepSeek-distilled → `reasoning_format`. Gate `reasoning_effort` emission on `gpt-oss-*` (pre-empts 422 risk on Llama-4-Scout/Maverick as Groq tightens validation).

### GQ-PR-8 (medium) — `response_format` / `json_schema` dropped

Groq supports `response_format ∈ { {"type":"json_object"}, {"type":"json_schema", "json_schema":{ name, description, schema, strict: true } } }` on the gpt-oss / llama-4 families. `LlmRequest::output_schema` (`crates/squeezy-llm/src/lib.rs:160-161`) is read only by the Responses provider; chat-completions drops it (shared-core M3). Groq users lose the contract.

**Fix**: shared-core M3.

### GQ-PR-9 (medium) — `prompt_cache_key` is wired but a no-op on Groq

Groq's automatic prompt caching is live for `gpt-oss-{20,120}b` (formerly also `kimi-k2-instruct-0905`). Groq does **not** document a request-side `prompt_cache_key`; the prefix is derived server-side. Squeezy emits the field unconditionally (`compatible.rs:225-237`); Groq silently ignores it, so no harm but a false-impression footgun. Cache-hit visibility does flow through: Groq populates `usage.prompt_tokens_details.cached_tokens`, which `parse_chat_usage` (`compatible.rs:1147-1151`) reads. Once GQ-PR-3 (shared C1) lands, cached-input accounting surfaces correctly.

**Fix**: no code change; document that `prompt_cache_key` is a no-op on Groq.

### GQ-PR-10 (medium) — `x-ratelimit-*` headers ignored on 200 OK

Groq emits `x-ratelimit-{limit,remaining,reset}-{requests,tokens}` on every chat-completions response. `parse_retry_after` (`crates/squeezy-llm/src/retry.rs:330-336`) only fires on retryable status codes (`retry.rs:120-153`); the proactive-pacing signal on 200 OK is dropped. First sign of throttling is a hard 429. Codex and opencode both pace pre-emptively from these headers. Shared-core GQ-4 restated.

**Fix**: surface `x-ratelimit-*` to the agent loop (e.g. `LlmEvent::ProviderHint { remaining_tokens, reset_seconds }`); use it for budget pacing in the cheap-model fast path (PR #213).

### GQ-PR-11 (medium) — `seed` blocked on `gpt-oss-*` regardless

Groq accepts `seed` on most SKUs but **400s on `gpt-oss-*`** (reasoning models reject `seed`). Squeezy doesn't emit `seed` (shared-core H3), so not broken today. When H3 lands, gate `seed` on `model_family != "gpt-oss"`.

### GQ-PR-12 (medium) — `delta.reasoning_content` already absorbed; notice still misfires

Groq's `gpt-oss-*` streams reasoning in `delta.reasoning_content`. `parse_chat_event` (`compatible.rs:1053-1054`) reads both `reasoning_content` and `reasoning`, so rendering works. The shared `reasoning_only_stop` notice at `compatible.rs:1106-1108` still misfires for legitimate `gpt-oss-*` thinking-only turns — same shape as DS-1 / CB-PR-3.

### GQ-PR-13 (low) — Multi-token-batched SSE events surface shared L2 + L4

Groq's LPU batches multiple deltas per SSE event. Shared-core L2 (O(n²) buffer re-scan in `sse.rs:36-47`) and L4 (`[DONE]` joined onto previous JSON) land on Groq identically to Cerebras. No Groq-specific action.

### GQ-PR-14 (low) — Telemetry preset id intact

`squeezy-telemetry/src/lib.rs:1017,1062` maps `Groq → TelemetryProvider::Groq`. Verified.

### GQ-PR-15 (low) — `PROVIDERS.md` performance number stale

`crates/squeezy-skills/external-docs/PROVIDERS.md:252-258` cites "Llama 3.x and Mixtral". Mixtral retired 2025-03-05; current marquee SKUs are Llama-4-Scout, `gpt-oss-120b`, Llama-3.3-70B-Versatile.

### GQ-PR-16 (low) — Costly test pins a stable SKU

`tests/groq_costly.rs:14-15` defaults to `llama-3.1-8b-instant` (cheapest active SKU). The `SQUEEZY_COSTLY_GROQ_MODEL` env allows targeted runs on `gpt-oss-120b` once GQ-PR-3 is verifiable. No action.

### GQ-PR-17 (nit) — `display_name` casing correct

`crates/squeezy-core/src/lib.rs:2026` = `"Groq"`. Matches brand. No action.

### GQ-PR-18 (nit) — `parse` alias coverage minimal

`crates/squeezy-core/src/lib.rs:2166` accepts only `"groq"`. No `"groqcloud"` etc. No documented user pain.

## Catalog Verification (June 2026)

| Groq model id | Status | Tools | Reasoning | Vision | squeezy aware |
|---|---|---|---|---|---|
| `openai/gpt-oss-120b` | production | ✓ + parallel | ✓ (`reasoning_effort`, `include_reasoning`) | ✗ | **no** |
| `openai/gpt-oss-20b` | production | ✓ + parallel | ✓ (same) | ✗ | **no** |
| `meta-llama/llama-4-scout-17b-16e-instruct` | production | ✓ | ✗ | ✓ (5 imgs) | **no** |
| `llama-3.3-70b-versatile` | production | ✓ | ✗ | ✗ | ✓ (default) |
| `llama-3.1-8b-instant` | production | ✓ | ✗ | ✗ | ✓ |
| `qwen-3-32b` | production | ✓ | ✓ (`reasoning_format`) | ✗ | **no** |
| `openai/gpt-oss-safeguard-20b` | production (T&S) | ✓ | ✓ | ✗ | **no** |
| `moonshotai/kimi-k2-instruct` | **retired 2025-09-10** | — | — | — | ✓ stale (GQ-PR-1) |
| `moonshotai/kimi-k2-instruct-0905` | **retired 2026-04-15** | — | — | — | no |
| `meta-llama/llama-4-maverick-17b-128e-instruct` | **retired 2026-02-20** | — | — | — | no |
| `meta-llama/llama-guard-4-12b` | **retired 2026-02-10** | — | — | — | no |
| `mixtral-8x7b-32768` | **retired 2025-03-05** | — | — | — | no |

Speech (Whisper, Orpheus) and Responses API live on separate endpoints (`/audio/*`, `/responses`); squeezy's chat-completions route does not address them.

## Test Coverage

| Surface | Costly | Mock | `models.json` | Status |
|---|---|---|---|---|
| Groq preset | ✓ (`groq_costly.rs`, single echo test on `llama-3.1-8b-instant`) | ✗ | partial (3 entries, 1 retired) | thin |

`tests/groq_costly.rs` exercises only the streaming-text smoke path. Missing coverage:
- Post-`finish_reason` usage chunk (catches GQ-PR-3 / shared-core C1).
- `gpt-oss-120b` reasoning stream (catches reasoning field-name + notice misfire).
- `llama-4-scout` image attachment (catches GQ-PR-2 vision-capability gate).
- `tool_choice = {"type":"function","function":{"name":"..."}}` (catches GQ-PR-4).
- `flex` service-tier short-circuit (catches GQ-PR-6, would require paid-tier creds).

## Verification Strategy

1. **Catalog drift lint**: CI assertion that every preset's `models.json` rows match Groq's `/openai/v1/models` listing within a tolerance window. Catches GQ-PR-1 next rotation; `kimi-k2-instruct` would have tripped this six months ago.
2. **Mock-server case in the parameterized harness** (shared-core §): Groq row with the post-`finish_reason` usage chunk + a `delta.reasoning_content` event + a `tool_choice` object payload. Asserts cost non-zero (GQ-PR-3), reasoning event emitted (GQ-PR-12), and the explicit-function form serializes correctly (GQ-PR-4).
3. **Costly extension** (`tests/groq_costly.rs`): add a second test that runs against `openai/gpt-oss-120b` with `reasoning_effort=low`. Assert `state.reasoning_buf` populates and `usage.prompt_tokens > 0`.
4. **Rate-limit-header recorder**: small reqwest middleware that logs `x-ratelimit-*` on every Groq response and asserts non-empty after a test run, to keep GQ-PR-10 progress measurable.

## References

- OpenAI compatibility: https://console.groq.com/docs/openai
- Chat completions reference: https://console.groq.com/docs/api-reference
- Supported models catalog: https://console.groq.com/docs/models
- Reasoning (`reasoning_format`, `include_reasoning`): https://console.groq.com/docs/reasoning
- Tool use (parallel tool calls, `tool_choice` object): https://console.groq.com/docs/tool-use
- Structured outputs / `json_schema`: https://console.groq.com/docs/structured-outputs
- Service tiers: https://console.groq.com/docs/service-tiers
- Flex processing (498 capacity_exceeded): https://console.groq.com/docs/flex-processing
- Vision: https://console.groq.com/docs/vision
- Rate-limit headers: https://console.groq.com/docs/rate-limits
- Prompt caching: https://console.groq.com/docs/prompt-caching
- Deprecations: https://console.groq.com/docs/deprecations
- Changelog: https://console.groq.com/docs/changelog
- Shared-core aggregator audit: `/Users/abbassabra/esqueezy/squeezy/.audit/providers/openai-compatible.md` (C1, GQ-1..GQ-4)
- opencode profile entry: `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/openai-compatible-profile.ts:12`
- opencode Groq binding: `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/openai-compatible.ts:64`
