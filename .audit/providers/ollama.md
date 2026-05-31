# Ollama Provider Audit

## Summary

- Severity tally: **2 critical / 6 high / 7 medium / 5 low / 3 nit**
- Top 3 actionable recommendations:
  1. **Set `num_ctx` to a sensible default (16k–32k)** on every native `/api/chat` request — Ollama's server default is 4096 tokens, which silently truncates agent prompts long before the underlying model's true context window. Tool calling reliability collapses below ~16k. (See HIGH-1.)
  2. **Detect terminal `done: true` chunks correctly and stop the stream loop** — the current parser yields `LlmEvent::Completed` on the first terminal frame but keeps polling `bytes.next()`. Ollama happily emits *intermediate* `done_reason: "load"` frames before the first content chunk; squeezy treats those as terminal and prematurely closes the turn with zero tokens. (See CRITICAL-1.)
  3. **Fix the `OLLAMA_BASE_URL` → `/api` baked-in assumption** that breaks every downstream URL builder when the user follows the official Ollama convention (`OLLAMA_HOST=http://host:11434`, no `/api` suffix). `fetch_ollama_context_window` and `pull_model` then POST to `…:11434/show` and `…:11434/pull` instead of `…/api/show` and `…/api/pull`, which silently 404 in OpenAI-compat mode and return HTML in native mode. (See CRITICAL-2.)

## Implementation Overview

The Ollama integration lives in a single file: `crates/squeezy-llm/src/ollama.rs` (552 lines). It owns three responsibilities:

1. **Streaming completions** via `OllamaProvider`, which implements `LlmProvider::stream_response`. The struct holds a `reqwest::Client`, a base URL string, the transport config, and an `Option<LMStudioProvider>` field named `compat`. When `OllamaConfig::route_style == OpenAiCompatible`, construction wires up a delegate `LMStudioProvider` (sibling provider at `lmstudio.rs:62`) pointed at the OpenAI-shaped `/v1` root and `stream_response` forwards to it (`ollama.rs:194–196`). When `route_style == Native` (the default per `squeezy_core::OllamaRoute::default`, see `crates/squeezy-core/src/lib.rs:2354–2364`), the provider speaks Ollama's proprietary `/api/chat` NDJSON itself.

2. **Model-pull streaming** via the free-floating `pull_model(base_url, model, cancel)` function (`ollama.rs:457`) plus the `PullEvent` enum (`ollama.rs:433–444`) and `parse_pull_line` helper (`ollama.rs:519`). This is a separate public API — `pull_model` is *not* gated by `route_style` and always hits the native `/pull` endpoint.

3. **Metadata probes**: `fetch_ollama_context_window` (`/show`, `ollama.rs:88`) and `fetch_ollama_model_names` (`/tags`, `ollama.rs:106`) used by `squeezy-agent` to populate the per-turn accounting context window override (`crates/squeezy-agent/src/lib.rs:1973–1978`) and the model picker.

The native request lifecycle: `request_body` (`ollama.rs:49`) calls `normalize_tool_ids_for_replay` (shared cross-provider helper at `crates/squeezy-llm/src/lib.rs:396`), builds a `{model, messages, stream:true, [options.num_predict], [tools]}` JSON body, posts it to `{base_url}/chat`, then streams the response through `JsonLineDecoder` (an in-file NDJSON line splitter at `ollama.rs:314`) and `parse_ollama_line` (`ollama.rs:349`). The OpenAI-compat route reuses the entire LM Studio stack — SSE decoding, tool-call accumulation, finish-reason mapping. There is therefore significant *protocol-level* duplication: native NDJSON parsing in `ollama.rs` is intentionally distinct from the SSE Chat-Completions parser in `lmstudio.rs`.

## Findings

### [CRITICAL-1] Intermediate `done_reason: "load"` chunks are treated as turn terminals

- **Location**: `crates/squeezy-llm/src/ollama.rs:404–423`
- **Observed**:
  ```rust
  if value.get("done").and_then(Value::as_bool) == Some(true) {
      let stop_reason = value.get("done_reason")...
      events.push(LlmEvent::Completed { ... });
  }
  ```
  The stream loop (`ollama.rs:220–256`) keeps polling `bytes.next()` after this push, but the consumer side of `LlmStream` in `squeezy-agent` returns on the first `LlmEvent::Completed`.
- **Issue**: Ollama emits a frame with `done: true` and `done_reason: "load"` when a model is loaded into memory (i.e. before generation actually starts) and another with `done_reason: "unload"` when `keep_alive: 0` is sent. Those are *not* turn terminals — they are housekeeping signals. The Ollama docs explicitly enumerate `"stop" | "load" | "unload"` for `done_reason`. squeezy's mapping in `StopReason::from_ollama` at `crates/squeezy-llm/src/lib.rs:545–551` only knows `"stop"` and `"length"`; everything else falls through to `Other(...)`. The parser then emits `Completed { stop_reason: Some(Other("load")), eval_count: 0 }` and the agent observes a zero-token, zero-text terminal — silently dropping the *actual* generation that follows.
- **Impact**: On any first request after Ollama has unloaded the model (which it does aggressively after 5 minutes idle by default — `OLLAMA_KEEP_ALIVE`), the user's turn fires, returns nothing, and the agent loop either retries or treats the empty response as the model refusing to answer.
- **Fix sketch**: Inside `parse_ollama_line`, treat `done_reason in {"load", "unload"}` as a no-op (return empty `events`). Only emit `Completed` for `"stop"`, `"length"`, missing `done_reason`, or any other reason. Add a test that feeds a `{"done":true,"done_reason":"load"}` frame followed by content frames and asserts the loop keeps going.
- **Reference**: [Ollama API docs](https://github.com/ollama/ollama/blob/main/docs/api.md); confirmed in WebFetch on `done_reason: "load" | "unload"`.

### [CRITICAL-2] `OLLAMA_HOST` users without `/api` suffix break every metadata probe and pull

- **Location**: `crates/squeezy-core/src/lib.rs:39` (`DEFAULT_OLLAMA_BASE_URL = "http://localhost:11434/api"`); `crates/squeezy-llm/src/ollama.rs:88–123` (`fetch_ollama_context_window` posts to `{base_url}/show`, `fetch_ollama_model_names` GETs `{base_url}/tags`); `ollama.rs:457–460` (`pull_model` POSTs to `{base_url}/pull`).
- **Observed**: squeezy's default base URL bakes `/api` into the string. Helpers then concatenate raw endpoint names (`/show`, `/tags`, `/pull`, `/chat`). If a user sets `OLLAMA_HOST=http://localhost:11434` (the official Ollama bind address variable per the [Ollama FAQ](https://docs.ollama.com/faq)) and squeezy reads that into `base_url`, every helper posts to `http://localhost:11434/show` instead of `…/api/show`.
- **Issue**: 
  1. The config reader only checks `OLLAMA_BASE_URL`, not the canonical `OLLAMA_HOST` (`crates/squeezy-core/src/lib.rs:657–659`). Users following Ollama upstream docs set `OLLAMA_HOST`; squeezy silently falls back to the localhost default.
  2. Even if the user sets `OLLAMA_BASE_URL=http://localhost:11434` (omitting `/api` because they expected a host root), all path joins are wrong.
  3. Codex's analog (`others/codex/codex-rs/ollama/src/url.rs:8–18`) splits host root vs. wire path cleanly: it accepts either `…/v1` or `…` and computes `host_root` then re-prefixes `/api/...` or `/v1/...` per endpoint.
- **Impact**: Silent 404s on every model picker refresh, context-window probe, and `pull_model` call when the user follows upstream Ollama convention. The 404s do not surface because `fetch_ollama_model_names` swallows all errors at `ollama.rs:115–121` (returns `Vec::new()`).
- **Fix sketch**:
  1. Read `OLLAMA_HOST` as a fallback for `OLLAMA_BASE_URL` in the config layer.
  2. Introduce a small URL helper analogous to `openai_compat_base_url` that normalizes any input shape (`http://x:11434`, `http://x:11434/`, `http://x:11434/api`, `http://x:11434/v1`) into a canonical *host root* and then always concatenates the per-endpoint path including `/api`. The shipping code does the right thing for `/chat` only because `DEFAULT_OLLAMA_BASE_URL` already ends in `/api`; the moment the assumption breaks every endpoint fails.
- **Reference**: [Ollama FAQ env vars](https://docs.ollama.com/faq); `others/codex/codex-rs/ollama/src/url.rs:1–40`.

### [HIGH-1] `num_ctx` is never set; default 4096 cripples agent workloads

- **Location**: `crates/squeezy-llm/src/ollama.rs:60–67`
- **Observed**:
  ```rust
  let mut body = json!({ "model": ..., "messages": ..., "stream": true });
  if let Some(max_output_tokens) = request.max_output_tokens {
      body["options"] = json!({ "num_predict": max_output_tokens });
  }
  ```
- **Issue**: Ollama's server default for `num_ctx` is 4096 tokens (per [Ollama FAQ](https://docs.ollama.com/faq) and `OLLAMA_CONTEXT_LENGTH=4096`). 4096 tokens is roughly enough for the system prompt and a single short turn; agentic flows with tool descriptions, history, and tool outputs blow through it instantly, then Ollama silently *drops* the oldest messages (no overflow signal). The opencode docs explicitly warn: *"If tool calls aren't working, try increasing `num_ctx`. Start around 16k - 32k"* ([opencode providers docs](https://opencode.ai/docs/providers/#ollama)).
- **Impact**: Tool calls stop working; agent loop misbehaves; the user sees the model "forget" history a few turns in with no error.
- **Fix sketch**: Pick a sensible default (e.g. 16384 or 32768) and stamp it onto `options.num_ctx` whenever the caller hasn't asked for something specific. Even better: probe the model's `model_info.*.context_length` from `/show` (the helper at `ollama.rs:139–158` already extracts this) and pick `min(probed_window, server_max)` at provider construction or first-use. The existing `fetch_ollama_context_window` is already wired into the agent's accounting path (`squeezy-agent/src/lib.rs:1975`) — round-trip it into the request body too.
- **Reference**: [opencode Ollama setup](https://opencode.ai/docs/providers); [Ollama FAQ](https://docs.ollama.com/faq).

### [HIGH-2] Tool-call `arguments` parsing assumes object; rejects array / scalar JSON

- **Location**: `crates/squeezy-llm/src/ollama.rs:377–402`
- **Observed**:
  ```rust
  let arguments = function.get("arguments").cloned()
      .unwrap_or_else(|| Value::Object(Default::default()));
  events.push(LlmEvent::ToolCall(LlmToolCall { call_id, name, arguments }));
  ```
- **Issue**: Ollama (unlike OpenAI) returns `arguments` as an already-parsed JSON value, not as a string — but the value can be any JSON type the model chose to emit. Smaller local models routinely emit arguments as a JSON *string* (`"arguments": "{\"path\": \"foo\"}"`) when they were trained on OpenAI conventions. The current code passes that string straight through as `Value::String(...)`, which downstream consumers expecting `Value::Object` for the function arguments will mis-handle (silently if the tool registry tolerates it, or with a confusing schema-validation error otherwise). LM Studio's sibling parser (`lmstudio.rs:407–419`) handles the string-vs-object distinction explicitly: it parses the string as JSON and on failure attaches the structured `INVALID_TOOL_ARGUMENTS_*` markers. The Ollama parser does not.
- **Impact**: Tool dispatch on smaller / quantized OSS models fails inconsistently depending on what training data the model saw.
- **Fix sketch**: When `arguments` is a `Value::String`, attempt `serde_json::from_str::<Value>(s)`; on success substitute; on failure attach the same `INVALID_TOOL_ARGUMENTS_KEY` envelope LM Studio uses. Pull the helper out of `lmstudio.rs` into a shared module so the contract stays in lockstep.
- **Reference**: `crates/squeezy-llm/src/lmstudio.rs:407–419`; smaller OSS model arguments-as-string behaviour confirmed in [apidog Ollama streaming tool calls](https://apidog.com/blog/ollama-streaming-responses-and-tool-calling/).

### [HIGH-3] No `keep_alive` plumbing — every turn pays the model-load tax

- **Location**: `crates/squeezy-llm/src/ollama.rs:60–67` (and entirely missing from `OllamaConfig` in `crates/squeezy-core/src/lib.rs:2301`).
- **Observed**: `request_body` never sets `keep_alive`. The struct has no field for it. The Ollama server default is 5 minutes, after which the model is unloaded.
- **Issue**: 13B+ models take seconds-to-tens-of-seconds to load. An agent loop that sits idle for >5 minutes (waiting on the user, on a slow shell command, etc.) silently pays the full load tax on the next turn — and *that* terminal load is what triggers the CRITICAL-1 "load done_reason" bug. For users who want fast iteration with a persistent local model, `keep_alive: -1` (load forever) is the desired knob.
- **Impact**: 10–60s latency on resume after idle; combined with CRITICAL-1, zero-token responses.
- **Fix sketch**: Add `keep_alive: Option<String>` to `OllamaConfig` (typed as string because Ollama accepts `"5m"`, `"24h"`, integer seconds, `0`, `-1`). Plumb into the request body when set. Document in the provider TOML reference.
- **Reference**: [Ollama FAQ keep_alive](https://docs.ollama.com/faq).

### [HIGH-4] Thinking-model support is missing (`think` parameter never sent)

- **Location**: `crates/squeezy-llm/src/ollama.rs:49–86`
- **Observed**: `request_body` never sends `"think"`, never reads `request.reasoning_effort`, and the parser at `ollama.rs:369–376` never looks for `message.thinking`.
- **Issue**: Ollama 0.6+ supports reasoning-trace separation for qwen3, deepseek-r1, deepseek-v3.1, gpt-oss via the `think: true` request parameter and `message.thinking` response field ([Ollama thinking capabilities docs](https://docs.ollama.com/capabilities/thinking)). gpt-oss requires `"low"|"medium"|"high"`. squeezy already has `LlmEvent::ReasoningDelta { text, kind }` and `LlmEvent::ReasoningDone` events plus a `reasoning_effort: Option<ReasoningEffort>` field on `LlmRequest` (`crates/squeezy-llm/src/lib.rs:136`), and other providers (OpenAI, Anthropic, Google) plumb them through. Ollama doesn't.
- **Impact**: Reasoning-only-stop detection (`reasoning_only_stop: bool` on `Completed`, see `lib.rs:611`) cannot fire for Ollama because reasoning deltas never enter the stream. Qwen3 / DeepSeek-R1 users see no `<think>` content separated out — it either bleeds into TextDelta or is wholly absent.
- **Fix sketch**: When `request.reasoning_effort.is_some()` (or the model is in a known thinking-capable allow-list), set `body["think"] = true` (or the gpt-oss low/medium/high string). In `parse_ollama_line`, branch on `message.thinking` and emit `LlmEvent::ReasoningDelta { kind: ReasoningKind::Native }`; on `done: true` emit `ReasoningDone` with the accumulated text.
- **Reference**: [Ollama thinking docs](https://docs.ollama.com/capabilities/thinking).

### [HIGH-5] NDJSON decoder silently drops non-UTF-8 chunks

- **Location**: `crates/squeezy-llm/src/ollama.rs:320–333`
- **Observed**:
  ```rust
  if let Ok(text) = String::from_utf8(line) {
      let text = text.trim();
      if !text.is_empty() { lines.push(text.to_string()); }
  }
  ```
- **Issue**: A line that fails UTF-8 conversion is silently discarded. The same is true in `finish()` at `ollama.rs:335–346`. If a single byte of a multi-byte sequence straddles chunk boundaries, `String::from_utf8` will fail on a *complete* JSON line whose UTF-8 happens to be malformed mid-line — and squeezy then loses a chat chunk or, worse, the terminal `done: true` frame. The buffer correctly accumulates across chunks but the *decoder* is byte-aligned by `\n` rather than codepoint-aligned. A line whose JSON body contains an emoji that ends exactly at the `\n` boundary works; one with a partial CR/LF on the boundary may not. Compare: codex's parser (`others/codex/codex-rs/ollama/src/client.rs:188–199`) bails the same way but at least serializes via `bytes::BytesMut` and `str::from_utf8(&line)`. The squeezy code is morally equivalent here — but neither buffers partial UTF-8 across the *line boundary*. The real risk is a server bug producing invalid UTF-8 mid-line; both drop silently. The bigger issue is the *swallow without error*: invalid UTF-8 should surface as a `ProviderStream` error so the stream-retry layer can reconnect, not produce a confusing empty-response.
- **Impact**: Rare in practice (Ollama emits ASCII JSON for nearly every field), but when it bites it hides the failure entirely.
- **Fix sketch**: Replace `String::from_utf8(line).ok()` with `std::str::from_utf8(&line).map_err(|e| SqueezyError::ProviderStream(format!("non-utf8 ndjson line: {e}")))` propagated through the loop. Optional: switch to a `bytes::BytesMut` accumulator analogous to codex's implementation.
- **Reference**: `others/codex/codex-rs/ollama/src/client.rs:177–200`.

### [HIGH-6] Pull endpoint has no de-duplication of concurrent identical pulls

- **Location**: `crates/squeezy-llm/src/ollama.rs:457–514`
- **Observed**: `pull_model` is a free function — every call opens a fresh `reqwest::Client` (line 458: `reqwest::Client::new()` — also notably *not* using the shared client cache) and POSTs `{"model": ..., "stream": true}` independently.
- **Issue**: Ollama's docs note multiple `POST /api/pull` calls for the same model *share* the same underlying download. The squeezy wrapper does not coordinate them — two concurrent agent threads pulling `qwen3-coder` each open their own HTTP request, each get the full NDJSON event stream, but each pays for the connection and the caller has to wire up its own UI deduplication. This is a latent bug rather than a functional one (the server handles it gracefully), but it matters for UX: a user who clicks "pull" twice in the TUI sees two competing progress bars.
- **Impact**: Confusing progress UX; redundant network sockets; future PullStream consumers (TUI) will need to handle this anyway.
- **Fix sketch**: Add a global `Mutex<HashMap<String, broadcast::Receiver<PullEvent>>>` (keyed by canonical model name) and fan out events to multiple concurrent subscribers. Or, more cheaply: document the contract and require callers to coordinate. The Codex implementation (`others/codex/codex-rs/ollama/src/client.rs:215–246`) similarly does not de-dupe; this is a *missed feature* rather than a defect, hence HIGH not CRITICAL.
- **Reference**: Ollama pull docs (concurrency note via WebFetch).

### [MEDIUM-1] `pull_model` uses unconfigured `reqwest::Client::new()` with no idle timeout

- **Location**: `crates/squeezy-llm/src/ollama.rs:458`
- **Observed**:
  ```rust
  let client = reqwest::Client::new();
  ```
- **Issue**: 
  1. Bypasses `shared_client` so the pull request opens a fresh TCP/TLS connection (no pool reuse with concurrent `/api/chat` traffic).
  2. No `transport`-side idle timeout: a hung pull will block indefinitely. Compare the chat path at `ollama.rs:225–230` which wraps `bytes.next()` in `tokio::time::timeout(idle_timeout(transport), ...)`.
  3. No retry; transient TCP failures during the multi-GB pull abort cleanly but the user sees the partial progress reset on the next attempt.
- **Impact**: Hung pulls hang the TUI forever; pulled bytes are not resumable through squeezy (Ollama's server-side resume works only if the client reconnects to the same in-flight pull — squeezy does, but the user has to manually re-invoke).
- **Fix sketch**: Use `shared_client(&self.transport)` (requires `pull_model` to take `&self` or accept a `&reqwest::Client`). Wrap the `bytes.next()` loop in `tokio::time::timeout(idle_timeout, ...)` matching the chat path. Surface a structured error on timeout.
- **Reference**: `crates/squeezy-llm/src/ollama.rs:225–230`; `crates/squeezy-llm/src/transport.rs:68`.

### [MEDIUM-2] `pull_model` is a free function — not on the provider — and not surfaced anywhere

- **Location**: `crates/squeezy-llm/src/ollama.rs:457`; exported via `crates/squeezy-llm/src/lib.rs:115`
- **Observed**: No callers in the repo. `grep -rn pull_model … --exclude-dir=ollama --exclude-dir=tests` returns zero hits.
- **Issue**: The API is public and tested but no UI code consumes it. The TUI model picker (`squeezy-tui/src/startup_model_picker.rs`) just lists installed models; there is no "pull missing model" flow even though Ollama's UX strongly assumes one. Compare codex's `ensure_oss_ready` at `others/codex/codex-rs/ollama/src/lib.rs:22–49` which auto-pulls the default OSS model if missing.
- **Impact**: A first-time user with an empty Ollama install gets "model not found" errors from `/chat` instead of an auto-pull.
- **Fix sketch**: Add a `ensure_model` step on Ollama provider startup or in the agent's first-request path that calls `fetch_ollama_model_names` + `pull_model` if missing. Hook into TUI progress UI.
- **Reference**: `others/codex/codex-rs/ollama/src/lib.rs:22–49`.

### [MEDIUM-3] Pull failure mid-stream leaves Ollama in a partially-pulled state with no rollback

- **Location**: `crates/squeezy-llm/src/ollama.rs:485–501`
- **Observed**: Stream loop bails with `Err(...)` on the first parser error; cancellation just returns early.
- **Issue**: Ollama keeps the partially-downloaded layer blobs on disk; subsequent calls will resume from where they left off, which is the *server's* expected behavior. squeezy does nothing wrong here, but: there is no diagnostic surface for the user telling them "the half-pulled model is taking up disk space — re-run `ollama pull qwen3-coder` to resume or `ollama rm` to clean up." For users without an `ollama` CLI on the host (e.g. remote-only access), this is a permanent footgun.
- **Impact**: Disk-fill scenarios on shared boxes; user confusion.
- **Fix sketch**: When a pull errors mid-stream, surface the error message verbatim plus a suggestion to retry; document on the public API that partial state remains on the server.

### [MEDIUM-4] No `/api/show` model validation before tool-calling

- **Location**: `crates/squeezy-llm/src/ollama.rs:68–83`
- **Observed**: Tools are added to the request body unconditionally whenever `request.tools` is non-empty.
- **Issue**: Tool-calling support in Ollama is per-model and per-version. Models without a `tools` template in their Modelfile will either silently ignore the tools or return a confusing error. The `/api/show` response contains a `capabilities` array that lists `"tools"` when supported (and `"thinking"`, `"vision"`, `"insert"`, `"embedding"`).
- **Impact**: First-time users picking a non-tool-capable local model (e.g. base llama3 without the instruct tag) get baffling no-op behavior instead of a clean "this model doesn't support tools" error.
- **Fix sketch**: Add a `fetch_ollama_capabilities` helper alongside `fetch_ollama_context_window` and gate the `request.ensure_tool_support("ollama")` check on it — analogous to the existing `ensure_vision_support` at `crates/squeezy-llm/src/lib.rs:344`.

### [MEDIUM-5] Multiple `tool_calls` get colliding `call_id` indices across NDJSON chunks

- **Location**: `crates/squeezy-llm/src/ollama.rs:382–402`
- **Observed**:
  ```rust
  for (index, tool_call) in tool_calls.iter().enumerate() {
      events.push(LlmEvent::ToolCall(LlmToolCall {
          call_id: format!("ollama_call_{index}"),
          ...
      }));
  }
  ```
- **Issue**: The index is *local to the chunk*, not a stream-global counter. If a model emits tool_calls across two separate chunks (e.g. one terminal chunk per the docs, but rare cases where streaming + thinking interleave), two distinct calls land with the same `ollama_call_0` id. The downstream `normalize_tool_ids_for_replay` at `crates/squeezy-llm/src/lib.rs:396` will canonicalize identical ids to the *same* `call_N`, collapsing two distinct calls into one.
- **Impact**: Rare today (Ollama emits all tool_calls in one final chunk per current docs) but a latent footgun that will manifest the moment Ollama streams tool calls incrementally per their [streaming-tool blog post](https://ollama.com/blog/streaming-tool) and Python SDK doc which says *"chunks can contain tool call data that gets combined"*.
- **Fix sketch**: Track a `tool_call_counter: usize` field on the stream-loop state and bump it for every emitted call. Or, when Ollama's `tool_call.function.id` field arrives (already documented as omitted but likely to be added), use it.
- **Reference**: [Ollama streaming + tool calling blog](https://ollama.com/blog/streaming-tool).

### [MEDIUM-6] `parse_ollama_line` ignores `done: false` chunks' usage fields

- **Location**: `crates/squeezy-llm/src/ollama.rs:404–423`
- **Observed**: `prompt_eval_count` / `eval_count` are read only when `done: true`.
- **Issue**: Per current Ollama docs the terminal chunk holds the totals so this is correct *today*. But cancelled streams (user hits Esc mid-token) skip the terminal frame entirely, so squeezy reports zero usage on every cancelled turn. The `Cancelled` branch at `ollama.rs:222–225` returns without emitting any cost. Compare LM Studio sibling at `lmstudio.rs:240–252` which drains pending state and emits a `Completed` with whatever it has on early termination. Ollama does not.
- **Impact**: Token accounting under-reports usage on every cancelled local turn. Free models, so dollar-impact is zero — but per-session token counts (used for context overflow detection) miss bytes.
- **Fix sketch**: On `Cancelled`, also emit a `Completed { stop_reason: None, cost: CostSnapshot::default(), ... }` so the agent loop terminates cleanly. Or estimate tokens from the buffered text.

### [MEDIUM-7] `parse_num_ctx` parameters-string fallback is fragile

- **Location**: `crates/squeezy-llm/src/ollama.rs:160–168`
- **Observed**:
  ```rust
  parameters.lines().find_map(|line| {
      let mut parts = line.split_whitespace();
      match (parts.next(), parts.next()) {
          (Some("num_ctx"), Some(value)) => value.parse().ok(),
          _ => None,
      }
  })
  ```
- **Issue**: The Modelfile parameters string can use quoted values, comments (`# foo`), or multi-token forms (`stop "<|im_end|>"`). The fallback only catches the simplest `num_ctx 8192` shape. Models that wrap the value in quotes (`num_ctx "8192"`) fall through. This is the *fallback path* — the primary `.context_length` extraction at `ollama.rs:142–151` should normally succeed for any modern model. But the fallback is exercised on older Ollama versions or hand-built models.
- **Impact**: Low — usually the primary path works. When the fallback engages on a quoted parameter, `fetch_ollama_context_window` returns `None` and the accounting layer falls back to the model-registry constant.
- **Fix sketch**: Strip trailing quotes / parse `value.trim_matches('"')`.

### [LOW-1] Connection-pool sharing for Ollama is per-config but no health check

- **Location**: `crates/squeezy-llm/src/ollama.rs:42`; `crates/squeezy-llm/src/transport.rs:68`
- **Observed**: `OllamaProvider` uses `shared_client(&config.transport)`. Good.
- **Verified: ✓** — shared TCP pool reuse confirmed. However, there is no liveness probe equivalent to codex's `OllamaClient::probe_server` (`others/codex/codex-rs/ollama/src/client.rs:81–101`). squeezy assumes the server is up and surfaces a stream error on the first request when it isn't.
- **Issue**: First-request UX: an offline Ollama produces a `ProviderRequest("error sending request")` with a transport message rather than a friendly "is `ollama serve` running?" hint.
- **Fix sketch**: Optional `probe_server` helper users can invoke at startup; or improve the error message in the request layer to detect connection-refused on the Ollama base URL and rewrite to a friendlier string.
- **Reference**: `others/codex/codex-rs/ollama/src/client.rs:22` (the `OLLAMA_CONNECTION_ERROR` constant).

### [LOW-2] Empty `message.content` with non-empty `tool_calls` is OK, but content-is-null isn't covered

- **Location**: `crates/squeezy-llm/src/ollama.rs:369–376`
- **Observed**:
  ```rust
  if let Some(content) = value.get("message")...get("content").and_then(Value::as_str)
      && !content.is_empty() { ... }
  ```
- **Verified: ✓** — empty-string content correctly skips the TextDelta emit. `Value::as_str()` returns `None` for `null`, so a `"content": null` field skips correctly too.
- **Issue**: minor: a `"content": 0` (numeric) would also skip without warning. Latent only; Ollama doesn't do this.
- **Fix**: none required.

### [LOW-3] Vision images are sent on a standalone "" content user message

- **Location**: `crates/squeezy-llm/src/ollama.rs:297–306`
- **Observed**:
  ```rust
  LlmInputItem::Image { ... } => {
      messages.push(json!({ "role": "user", "content": "", "images": [base64(bytes)] }));
  }
  ```
- **Issue**: This sends each image as its own user turn with no prompt text. The previous user-text turn (which usually says "what is this?") becomes a sibling of the image rather than the prompt for it. Some vision models pair the image with the *most recent* user text message; an empty-content image-only turn after the text turn changes the semantics.
- **Impact**: Vision accuracy regressions on llava/llama3.2-vision when the prompt is more than one short sentence.
- **Fix sketch**: When the previous `messages.last()` entry is a user message with non-empty content, attach the `images` array to *that* message instead of emitting a new one. This matches how Ollama's docs and the official Python SDK examples show images being attached.
- **Reference**: [Ollama API chat docs](https://docs.ollama.com/api/chat) — images are a field on a single user message.

### [LOW-4] `bytes_stream()` chunks are not size-bounded

- **Location**: `crates/squeezy-llm/src/ollama.rs:219–243` and `ollama.rs:484–502`
- **Observed**: `JsonLineDecoder::buffer: Vec<u8>` grows unbounded.
- **Issue**: A misbehaving server could feed `\n`-less bytes indefinitely; squeezy buffers everything in memory. There is no cap. Lines >64KB are absorbed without issue (no artificial limit) but pathological cases (megabytes per "line") could OOM.
- **Fix sketch**: Add a `MAX_NDJSON_LINE_BYTES` constant (1MB is a comfortable upper bound for tool-call JSON) and bail with a `ProviderStream` error if the buffer grows past it without seeing a newline.

### [LOW-5] OAuth/bearer auth for Ollama Cloud is not supported on the native route

- **Location**: `crates/squeezy-llm/src/ollama.rs:30–47`
- **Observed**: `OllamaProvider::from_config` does not take an `api_key`; no header is attached to native requests at `ollama.rs:204` either.
- **Issue**: Ollama Cloud (the hosted SaaS, see [opencode Ollama Cloud setup](https://opencode.ai/docs/providers/#ollama-cloud)) and self-hosted Ollama behind any reverse proxy that enforces a `Bearer` token are unreachable on the native route. The OpenAI-compat route inherits LM Studio's `api_key` field (`lmstudio.rs:46`), but that field is never populated by `OllamaProvider::from_config`'s OpenAI-compat fork at `ollama.rs:35–39` — it sets `api_key: None`.
- **Impact**: Cloud-Ollama users cannot use squeezy at all; reverse-proxy-protected self-hosters can't use squeezy in OpenAI-compat mode.
- **Fix sketch**: Add `api_key: Option<String>` to `OllamaConfig` and plumb to both the native request (via `bearer_auth`) and the LM Studio delegate.

### [NIT-1] Status-line semantics duplicate the `Status` event when pulling

- **Location**: `crates/squeezy-llm/src/ollama.rs:519–545`
- **Observed**: `parse_pull_line` emits `Status("success")` *and then* falls through to also yield `PullEvent::Success` via the dedicated guard. Wait — let me re-read. Actually the code returns `Some(PullEvent::Success)` early at line 530 before falling through to the generic-status branch, so the duplicate doesn't happen here. **Verified: ✓** — no dup.
- However the codex parser at `others/codex/codex-rs/ollama/src/parser.rs:8–13` *does* emit both Status("success") and Success in that order. squeezy's behavior of swallowing the Status frame is the cleaner choice; just worth flagging the deliberate divergence.

### [NIT-2] `fetch_ollama_context_window` has a hard 250ms timeout that drops slow Ollamas

- **Location**: `crates/squeezy-llm/src/ollama.rs:88–104`
- **Observed**:
  ```rust
  let client = reqwest::Client::builder().timeout(Duration::from_millis(250)).build().ok()?;
  ```
- **Issue**: 250 ms is fine on localhost but too tight for any remote / Tailscale / Docker-networked Ollama. Falls back to `None` silently. Same applies to `fetch_ollama_model_names` (`ollama.rs:107–110`).
- **Fix sketch**: Bump to 1 second; or read from the transport config (these helpers don't currently take one).

### [NIT-3] Tests do not cover `done_reason: "load"` / `"unload"` or the `LMStudioProvider` compat delegation end-to-end

- **Location**: `crates/squeezy-llm/src/ollama_tests.rs`; `crates/squeezy-llm/tests/ollama_smoke.rs`
- **Observed**: Compat route is verified at construction only (`ollama_tests.rs:255–278`); no streaming end-to-end test through the LM Studio path. No `done_reason: "load"` test (would catch CRITICAL-1).

## Test Coverage Gaps

- **CRITICAL** — No test for intermediate `done: true` chunks with `done_reason: "load"` or `"unload"`. Easy to mock (extend the existing `parser_extracts_text_tool_calls_and_usage` test pattern in `ollama_tests.rs:87`).
- **CRITICAL** — No test for `OLLAMA_HOST` URL shape variations. Add a parametrized test feeding each of `{http://x:11434, http://x:11434/, http://x:11434/api, http://x:11434/v1}` and asserting `pull_model` / `fetch_ollama_context_window` POST to the right path.
- **HIGH** — No mocked end-to-end `/api/chat` test (the only existing chat test is the network-gated smoke at `tests/ollama_smoke.rs`). Mockable with a `TcpListener` analogous to `tests/ollama_pull_mock.rs`.
- **HIGH** — No test for tool_calls with string-encoded arguments (HIGH-2). Trivially mockable.
- **HIGH** — No test for streaming the OpenAI-compat route from `OllamaProvider`. Currently only verifies compat is constructed — the actual delegation path is untested through Ollama's public surface.
- **MEDIUM** — No test for the cancellation path mid-stream (Esc behavior). The pull cancel path is also untested.
- **MEDIUM** — No test for images attached to a multi-message conversation (LOW-3 path).
- **MEDIUM** — No test for partial UTF-8 across `\n` boundaries (HIGH-5).
- **LOW** — No test for keep_alive plumbing once HIGH-3 is fixed.
- **LOW** — No test asserting `num_ctx` is set on every native request once HIGH-1 is fixed.
- **NIT** — No regression test for the `parse_num_ctx` quoted-value fallback (MEDIUM-7).

## Verification Strategy

Ollama is free; spin a local server up for end-to-end checks.

1. **Install + start**: `brew install ollama && ollama serve &` (binds 127.0.0.1:11434).
2. **Pull a tool-capable small model**: `ollama pull qwen3:0.6b` (~1GB, has tools+thinking support).
3. **Run the smoke test forced**: `SQUEEZY_OLLAMA_SMOKE=1 cargo test --test ollama_smoke`.
4. **Validate CRITICAL-1** — Reproduce the `load` done_reason bug:
   - Wait 6 minutes after last request (so server unloads).
   - Send a turn; observe whether squeezy reports the first response as zero-content or whether it correctly waits for the post-load generation chunks.
   - Alternative: `curl -s http://localhost:11434/api/chat -d '{"model":"qwen3:0.6b","messages":[{"role":"user","content":"hi"}],"stream":true,"keep_alive":0}' | head -5` to see the explicit `done_reason: "unload"` frame.
5. **Validate CRITICAL-2** — Set `OLLAMA_BASE_URL=http://localhost:11434` (no `/api`) and confirm `fetch_ollama_model_names` returns the empty list (it will silently swallow the 404).
6. **Validate HIGH-1** — Send a turn with 10k tokens of system+user; observe model "forgets" earlier history. Then patch in `options.num_ctx = 32768` and verify the same prompt produces grounded answers.
7. **Validate HIGH-4** — Pull `qwen3:8b` and toggle `think: true`; observe the `message.thinking` field on the wire (`curl -N`). Confirm squeezy never emits `ReasoningDelta`.
8. **Validate HIGH-6 pull dedup** — `pull_model("qwen3-coder")` twice concurrently; observe both stream the same NDJSON but as two separate sockets (use `lsof` on the squeezy PID).

## References

- [Ollama API docs](https://github.com/ollama/ollama/blob/main/docs/api.md)
- [Ollama FAQ — env vars](https://docs.ollama.com/faq) (`OLLAMA_HOST`, `OLLAMA_KEEP_ALIVE`, `OLLAMA_CONTEXT_LENGTH=4096`, `OLLAMA_NUM_PARALLEL=1`, `OLLAMA_MAX_QUEUE=512`)
- [Ollama chat endpoint docs](https://docs.ollama.com/api/chat)
- [Ollama tool calling capability](https://docs.ollama.com/capabilities/tool-calling)
- [Ollama thinking capability](https://docs.ollama.com/capabilities/thinking)
- [Ollama streaming + tool calling blog (May 2025)](https://ollama.com/blog/streaming-tool)
- [Apidog: Ollama Streaming Responses and Tool Calling](https://apidog.com/blog/ollama-streaming-responses-and-tool-calling/)
- [opencode providers docs — Ollama section](https://opencode.ai/docs/providers/#ollama) (num_ctx 16–32k guidance)
- Reference implementations in this repo: `others/codex/codex-rs/ollama/src/{client,parser,pull,url}.rs`; `others/codex/codex-rs/model-provider-info/src/lib.rs:402–514`.
- squeezy code: `crates/squeezy-llm/src/ollama.rs`, `crates/squeezy-llm/src/lmstudio.rs`, `crates/squeezy-llm/src/lib.rs:545–551`, `crates/squeezy-core/src/lib.rs:39, 656–666, 2300–2374`, `crates/squeezy-agent/src/lib.rs:1973–1978`.
