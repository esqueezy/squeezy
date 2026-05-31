# AWS Bedrock Provider Audit

## Summary
- Severity tally: 2 critical / 6 high / 7 medium / 4 low / 3 nit
- Top 3 actionable recommendations:
  1. **Set `inferenceConfig.maxTokens`** (and pipe stop sequences / temperature). Today every Converse request runs with the model's vendor default cap. Claude 4.5/4.6 on Bedrock can stop mid-reply or burn user budget unexpectedly because squeezy never bounds the response.
  2. **Honor `CacheRetention::Long` in `cache_point_block()`** by calling `CachePointBlock::builder().ttl(CacheTtl::OneHour)`. Long retention silently downgrades to 5-minute caching on the Bedrock route, defeating the intent of the cross-provider `CacheSpec` knob.
  3. **Plumb adaptive-thinking for Claude 4.6+** (`{"thinking":{"type":"adaptive"}, "output_config":{"effort":...}}`) and use `reasoning_config` instead of raw `thinking` on Claude 3.7/4.0. The current `thinking={type:enabled, budget_tokens:N}` block hits a hard 400 on adaptive-thinking models and ignores Anthropic's documented Bedrock contract for newer Claude families.

## Implementation Overview

The Bedrock provider lives in a single file: `crates/squeezy-llm/src/bedrock.rs` (763 lines). `BedrockProvider` (`bedrock.rs:37-46`) holds the region, optional endpoint override, optional bearer token (`AWS_BEARER_TOKEN_BEDROCK`), operator cost-allocation tags (`request_metadata`), the cross-provider `ProviderTransportConfig`, and an `Arc<OnceCell<SdkConfig>>` that lazily caches the AWS SDK configuration. `from_config` (`bedrock.rs:49-58`) clones every value off `BedrockConfig` (declared in `crates/squeezy-core/src/lib.rs:2278-2298`). Region resolution lives in core (`squeezy-core/src/lib.rs:634-655`) and chains `AWS_REGION` → `AWS_DEFAULT_REGION` → `providers.bedrock.region` TOML → `DEFAULT_BEDROCK_REGION = "us-east-1"` (`squeezy-core/src/lib.rs:37`). The default model is `anthropic.claude-haiku-4-5-20251001-v1:0` (`squeezy-core/src/lib.rs:38`).

The request lifecycle (`bedrock.rs:126-253`) wraps the entire flow in an `async_stream::try_stream!`, builds a `ConverseStream` invocation, sends it, and pumps `ConverseStreamOutput` events through `handle_bedrock_event` (`bedrock.rs:322-435`). The stream loop applies `tokio::time::timeout(idle_timeout(transport), ...)` per event (`bedrock.rs:216-222`) using the cross-provider 300 s idle timeout. Auth resolution (`build_bedrock_client`, `bedrock.rs:93-119`) routes through the AWS SDK's default credential chain unless `AWS_BEARER_TOKEN_BEDROCK` is set, in which case it clears the SigV4 provider and installs a bearer-token identity. Cache breakpoints are emitted positionally as `ContentBlock::CachePoint` blocks in system, the last user message, and after the last non-`mcp__` tool (`bedrock.rs:456-672`).

Notable design choices: tool-call ids are canonicalized via `normalize_tool_ids_for_replay` (`lib.rs:396-456`) before being lowered, consecutive same-role messages are merged into a single multi-block message (`push_message`, `bedrock.rs:607-636`), images route through `bedrock_image_block` (`bedrock.rs:680-699`), reasoning blocks (Anthropic-on-Bedrock) round-trip via `ReasoningTextBlock`/`RedactedContent` (`bedrock.rs:543-573`), and unmapped `ConverseStreamOutput` variants silently no-op via a wildcard `_ => Ok(Vec::new())` (`bedrock.rs:433`). The provider is **not** wrapped in `with_stream_retry`, unlike `AnthropicProvider` (`anthropic.rs:490-492`).

## Findings

### [CRITICAL] `inferenceConfig` is never set — no `maxTokens`, no stop sequences, no temperature pinning
- **Location**: `crates/squeezy-llm/src/bedrock.rs:141-200`
- **Observed**: `client.converse_stream().model_id(&model)` is built up with `system`, `messages`, `tool_config`, `additional_model_request_fields`, and `request_metadata`. There is no `.inference_config(...)` call anywhere in the file, and `request.max_output_tokens` is never read in `bedrock.rs`.
- **Issue**: The Converse API treats absent `inferenceConfig.maxTokens` as "model vendor default." For Claude on Bedrock that default is the *model's* upper limit — Claude Sonnet 4.5 ships ~64 k; Claude 3.7 Sonnet ships ~131 k. The user-supplied `LlmRequest.max_output_tokens` (which the Anthropic native path enforces at `anthropic.rs:152-160`) is silently discarded on the Bedrock route. Reasoning-effort users are doubly bitten because the `budget_tokens` is forwarded (see below) but the hard `max_tokens > budget_tokens` invariant is never validated.
- **Impact**: (1) An operator who lowered `max_output_tokens = 4096` to bound spend gets unbounded replies on Bedrock; (2) reasoning-effort users with adaptive-thinking Claude 4.6+ models can pin a `budget_tokens=16384` while the model is free to spend `max_tokens=65536` of unrequested capacity; (3) deterministic eval harnesses that pass `temperature=0` cannot do so today — `inferenceConfig.temperature` is also unwired.
- **Fix sketch**:
  ```rust
  use aws_sdk_bedrockruntime::types::InferenceConfiguration;
  // … inside stream_response after tool_config wiring:
  let mut inf = InferenceConfiguration::builder();
  if let Some(max) = request.max_output_tokens {
      inf = inf.max_tokens(i32::try_from(max).unwrap_or(i32::MAX));
  }
  // future: temperature / top_p / stop_sequences once LlmRequest gains those.
  builder = builder.inference_config(inf.build());
  ```
- **Reference**: [InferenceConfiguration — Amazon Bedrock](https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_InferenceConfiguration.html), [`anthropic.rs:152-160`](../../crates/squeezy-llm/src/anthropic.rs), `others/opencode/packages/llm/src/protocols/bedrock-converse.ts:374-385`.

### [CRITICAL] `Long` cache retention silently downgrades to 5-minute caching on Bedrock
- **Location**: `crates/squeezy-llm/src/bedrock.rs:456-463`
- **Observed**: 
  ```rust
  fn cache_point_block() -> Result<CachePointBlock> {
      CachePointBlock::builder()
          .r#type(CachePointType::Default)
          .build()
  ```
  Every cache point — system, last user message, tool tail — is built without a `ttl` field, ignoring `request.effective_cache_spec().retention`.
- **Issue**: The Rust SDK's `CachePointBlock` exposes `.ttl(CacheTtl::OneHour)` since the same release as the docs (`docs.rs/aws-sdk-bedrockruntime` confirms). Bedrock honors `ttl: "1h"` for Claude Opus 4.5 / Haiku 4.5 / Sonnet 4.5. Without the setter, `CacheRetention::Long` is honored on the Anthropic-native path (`anthropic.rs` emits `cache_control: { type: "ephemeral", ttl: "1h" }` via `cache_policy::ephemeral_marker`) but silently degrades to the 5-minute default on Bedrock.
- **Impact**: An agent that opted into `Long` retention to amortize the cache write across a multi-hour run gets the writes but never the extended-TTL reads — every 5 minutes the prefix re-bills.
- **Fix sketch**: Take the effective retention into the helper:
  ```rust
  fn cache_point_block(retention: CacheRetention) -> Result<CachePointBlock> {
      let mut b = CachePointBlock::builder().r#type(CachePointType::Default);
      if retention == CacheRetention::Long {
          b = b.ttl(CacheTtl::OneHour);
      }
      b.build().map_err(...)
  }
  ```
  Thread `retention` through `system_blocks`, `conversation_messages`, and `tool_configuration` (already takes `prompt_caching: bool`; promote to `Option<CacheRetention>`).
- **Reference**: [Prompt caching for faster model inference — Amazon Bedrock](https://docs.aws.amazon.com/bedrock/latest/userguide/prompt-caching.html), [CachePointBlock docs.rs](https://docs.rs/aws-sdk-bedrockruntime/latest/aws_sdk_bedrockruntime/types/struct.CachePointBlock.html), `others/opencode/packages/llm/src/protocols/utils/bedrock-cache.ts:24-35`.

### [HIGH] Reasoning payload uses the wrong schema for adaptive-thinking Claude (4.6+)
- **Location**: `crates/squeezy-llm/src/bedrock.rs:165-185`
- **Observed**: 
  ```rust
  let thinking = Document::Object([
      ("type".to_string(), Document::String("enabled".to_string())),
      ("budget_tokens".to_string(), Document::Number(Number::PosInt(budget as u64))),
  ]…);
  extra_fields.insert("thinking".to_string(), thinking);
  ```
- **Issue**: Anthropic's adaptive-thinking models (Claude 4.6 Opus / Sonnet) reject `thinking.type = "enabled"` — they require `{"thinking":{"type":"adaptive"}, "output_config":{"effort":"low|medium|high"}}`. The Anthropic-native path handles this (`anthropic.rs:186-193` via `model_uses_adaptive_thinking`); the Bedrock path always emits the `enabled + budget_tokens` shape. Additionally, AWS docs and the Bedrock Converse reference call out that the *Converse* envelope expects `reasoning_config` (not `thinking`) for Claude 3.7 Sonnet's reasoning surface; the literal `thinking` key works for Anthropic-shaped models but the canonical Bedrock surface is `reasoning_config`.
- **Impact**: Setting `reasoning_effort` on `anthropic.claude-opus-4-6` (or any adaptive-thinking model) via Bedrock returns a hard 400 every turn until the user clears `reasoning_effort`. Even for non-adaptive models, the `max_tokens > budget_tokens` invariant Anthropic enforces (`anthropic.rs:195-221`) is missing here, so a too-small `max_output_tokens` is silently rejected by the upstream.
- **Fix sketch**: Mirror `anthropic.rs:186-223`:
  ```rust
  if model_uses_adaptive_thinking(&model) {
      extra_fields.insert("thinking", Document::Object(/* type=adaptive */));
      extra_fields.insert("output_config",
          Document::Object([("effort", anthropic_effort_label(effort))]));
  } else {
      let ceiling = max_tokens.saturating_sub(THINKING_REPLY_HEADROOM);
      if ceiling >= ANTHROPIC_MIN_THINKING_BUDGET_TOKENS {
          // emit enabled+budget_tokens shape
      } else {
          tracing::warn!(...);
      }
  }
  ```
- **Reference**: [Adaptive thinking — Amazon Bedrock](https://docs.aws.amazon.com/bedrock/latest/userguide/claude-messages-adaptive-thinking.html), [Use Anthropic Claude 3.7 Sonnet's reasoning capability on Amazon Bedrock](https://docs.aws.amazon.com/bedrock/latest/userguide/bedrock-runtime_example_bedrock-runtime_Converse_AnthropicClaudeReasoning_section.html), `crates/squeezy-llm/src/anthropic.rs:186-223`.

### [HIGH] No application-inference-profile or cross-region detection / no `Vec` of fallback regions
- **Location**: `crates/squeezy-llm/src/bedrock.rs:141` (`model_id(&model)` ships the raw string)
- **Observed**: `model_id` is forwarded verbatim. There's no logic to detect inference-profile ARNs (`arn:aws:bedrock:<region>:<acct>:inference-profile/...`, `application-inference-profile/...`) or cross-region inference prefixes (`us.`, `eu.`, `apac.`, `jp.`, `global.`).
- **Issue**: Newer Claude models (Sonnet 4.6 / Opus 4.6 / Sonnet 4.5) on Bedrock **require** a cross-region inference profile and refuse on-demand throughput — calling `anthropic.claude-sonnet-4-6-...` directly returns a `ValidationException` until the caller prefixes `us.` / `eu.` / `apac.`. clear-code ships dedicated helpers (`others/clear-code/src/utils/model/bedrock.ts:189-265`: `getBedrockRegionPrefix`, `applyBedrockRegionPrefix`, `isFoundationModel`, `getBedrockInferenceProfiles`) that the squeezy provider lacks.
- **Impact**: An operator pointing squeezy at Claude 4.6 on Bedrock has to manually pre-prefix every model id in their config; the registry default (`anthropic.claude-haiku-4-5-20251001-v1:0`) probably fails on regions where on-demand throughput isn't available. There's also no telemetry about which inference profile was actually used (Bedrock returns the resolved model in headers; we don't surface `ServerModel`).
- **Fix sketch**: Add a `region_prefix: Option<String>` (or `auto_detect_inference_profile: bool`) on `BedrockConfig` and a helper that rewrites `anthropic.claude-*` to `us.anthropic.claude-*` based on `region` (`us-* → us`, `eu-* → eu`, `ap-* → apac`). Emit a `tracing::info!` when the rewrite fires and an `LlmEvent::ServerModel` if the resolved id differs from the requested one.
- **Reference**: [API restrictions — Amazon Bedrock](https://docs.aws.amazon.com/bedrock/latest/userguide/inference-api-restrictions.html), `others/clear-code/src/utils/model/bedrock.ts:189-265`.

### [HIGH] No stream-level retry wrapper — mid-stream `ModelStreamErrorException` / `ThrottlingException` is terminal
- **Location**: `crates/squeezy-llm/src/bedrock.rs:126-253`
- **Observed**: `stream_response` returns the raw `try_stream!` future directly; there is no `with_stream_retry(...)` wrapper as in `anthropic.rs:490-492`. Inside the loop, `recv_event` returns `SqueezyError::ProviderStream(...)` on any `ConverseStreamOutputError` (`bedrock.rs:262-266`) and propagates straight to the caller.
- **Issue**: AWS docs explicitly mark `ModelStreamErrorException` as retryable: *"An error occurred while streaming the response, you should retry your request."* `InternalServerException` and `ServiceUnavailableException` likewise. The squeezy stream-retry harness already tracks `emitted_text_chars` etc. so a reconnect won't duplicate output (`retry.rs:354-484`). Bedrock is the only Anthropic-shaped provider not wrapped in it. Errors flowing through the AWS SDK's *initial* `send()` are retried by the SDK's standard policy (3 attempts) but mid-stream errors are not.
- **Impact**: A flaky region, a transient throttle, or a brief Anthropic backend hiccup mid-stream tears the whole turn down. The agent loop sees `ProviderStream(...)` and has to re-issue the entire request (no `StreamSkipState` to suppress the already-emitted prefix), which double-bills the input tokens.
- **Fix sketch**: Wrap the body in `with_stream_retry`:
  ```rust
  let make_attempt = move || {
      let provider = self.clone();
      let request = request.clone();
      let cancel = cancel.clone();
      Box::pin(provider.stream_once(request, cancel))
  };
  with_stream_retry("bedrock", RetryPolicy::provider_stream(transport), cancel, make_attempt)
  ```
- **Reference**: [ModelStreamErrorException Class — AWS SDK for .NET V3](https://docs.aws.amazon.com/sdkfornet/v3/apidocs/items/BedrockRuntime/TModelStreamErrorException.html), [`crates/squeezy-llm/src/anthropic.rs:490-492`](../../crates/squeezy-llm/src/anthropic.rs), [`crates/squeezy-llm/src/retry.rs:492-581`](../../crates/squeezy-llm/src/retry.rs).

### [HIGH] Mid-stream `ModelStreamErrorException`, `ThrottlingException`, `ValidationException` aren't matched explicitly
- **Location**: `crates/squeezy-llm/src/bedrock.rs:322-435`
- **Observed**: `handle_bedrock_event` matches `MessageStart`, `ContentBlockStart`, `ContentBlockDelta`, `ContentBlockStop`, `MessageStop`, `Metadata` and falls through to `_ => Ok(Vec::new())`. The AWS SDK exposes mid-stream errors as Smithy stream errors surfaced through `recv_event` rather than `ConverseStreamOutput` variants, but the `ConverseStreamOutputError` envelope carries the discriminant.
- **Issue**: A `ModelStreamErrorException` is currently rendered as a generic `Bedrock event stream error: {err}` (`bedrock.rs:265`). The retry harness in `retry.rs:583-588` checks `is_retryable_stream_error` purely on the `SqueezyError` variant, so it would actually retry — but the upstream error type (`ValidationException` vs `ThrottlingException` vs `ModelStreamErrorException`) is lost. opencode classifies these distinctly to mark `ThrottlingException` retryable and `ValidationException` terminal (`others/opencode/packages/llm/src/protocols/bedrock-converse.ts:541-554`).
- **Impact**: Without classification, a hard `ValidationException` (e.g. tool schema rejected, malformed image) is treated identically to a transient `ThrottlingException`; if a stream-retry wrapper is added (see prior finding) it will pointlessly retry deterministic failures and burn the budget.
- **Fix sketch**: Match `ConverseStreamOutputError` discriminants in `recv_event` (or upstream) and emit dedicated variants — `SqueezyError::ProviderRequest` for validation, `SqueezyError::ProviderStream` (retryable) for throttling / internal-server / service-unavailable / model-stream.
- **Reference**: [ConverseStreamError — Rust SDK](https://docs.rs/aws-sdk-bedrockruntime/latest/aws_sdk_bedrockruntime/operation/converse_stream/enum.ConverseStreamError.html), `others/opencode/packages/llm/src/protocols/bedrock-converse.ts:541-554`.

### [HIGH] No document content support — PDFs / DOCX / CSVs go nowhere
- **Location**: `crates/squeezy-llm/src/bedrock.rs:479-582`
- **Observed**: `conversation_messages` handles `UserText`, `AssistantText`, `FunctionCall`, `FunctionCallOutput`, `Image`, and `Reasoning(Anthropic)`. There is no path for documents — partly because `LlmInputItem` (lib.rs:248-273) only models text + function + image + reasoning.
- **Issue**: Bedrock's Converse API supports `ContentBlock::Document` (pdf, txt, csv, doc, docx, xls, xlsx, html, md) up to 4.5 MB per document (Claude 4 lifts the PDF cap). opencode plumbs this (`others/opencode/packages/llm/src/protocols/utils/bedrock-media.ts:20-78`); pi also supports it. squeezy users on Bedrock cannot attach a PDF tool result even when the model can read it.
- **Impact**: Long-running agent flows that produce a PDF (web archive, report) can't feed it back to the model. Forces base64-text-blob workarounds that bloat input tokens.
- **Fix sketch**: Add `LlmInputItem::Document { media_type, name, bytes }` in lib.rs and a `bedrock_document_block` helper mirroring `bedrock_image_block`. Gate emission on `capabilities.document_input` (a new flag in models.json). Track this as a follow-up rather than blocking the provider audit.
- **Reference**: [DocumentBlock — Amazon Bedrock](https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_DocumentBlock.html), `others/opencode/packages/llm/src/protocols/utils/bedrock-media.ts:20-78`.

### [HIGH] AWS bearer token re-uses cached shared `SdkConfig` but never invalidates it
- **Location**: `crates/squeezy-llm/src/bedrock.rs:60-119`
- **Observed**: `client()` populates `self.shared` exactly once via `OnceCell`, then `build_bedrock_client` decides per-call whether to install a bearer token or keep the cached credentials. The cached `SdkConfig` is *immutable* for the lifetime of the provider.
- **Issue**: AWS Bedrock API keys carry an expiry — short-term keys last up to 12 hours, long-term keys 1/5/30/90/365 days. squeezy reads `AWS_BEARER_TOKEN_BEDROCK` once at `BedrockConfig` load time (`squeezy-core/src/lib.rs:648-650`) and never refreshes. Worse: when SigV4 credentials expire (typical session token lifetime is ~1h on STS-derived roles), the cached `SdkConfig.credentials_provider()` is a `SharedCredentialsProvider` — it *does* re-fetch on each request, so SigV4 is fine. But once the bearer-token path is taken, the bearer is the only auth — if the env var is rotated mid-process there is no refresh hook.
- **Impact**: A long-running agent session (eval suite, watchdog) using a short-term Bedrock API key starts failing with 401 after 12 hours and recovery requires restarting the process.
- **Fix sketch**: (1) Re-read `AWS_BEARER_TOKEN_BEDROCK` on each `client()` call (cheap) so a rotated env var is picked up; (2) Surface a `ProviderTokenExpired` distinct error variant for `401 ExpiredToken` from Bedrock so the agent can prompt for refresh; (3) Document the trade-off in the auth docstring.
- **Reference**: [API keys — Amazon Bedrock](https://docs.aws.amazon.com/bedrock/latest/userguide/api-keys.html).

### [MEDIUM] Wildcard match swallows brand-new `ConverseStreamOutput` variants without telemetry
- **Location**: `crates/squeezy-llm/src/bedrock.rs:433`
- **Observed**: `_ => Ok(Vec::new())` at the bottom of `handle_bedrock_event`. The AWS SDK marks `ConverseStreamOutput` `#[non_exhaustive]`.
- **Issue**: Future Bedrock features (e.g. native multi-modal output blocks, citation events, guardrail trace events) will surface as new variants. The current code silently no-ops them — no warning, no metric, no observable signal that the agent is missing data.
- **Impact**: Silent feature drift; when AWS GA's `citationsDelta` or `guardrailAssessment`, squeezy users won't see anything until somebody manually checks why the model returned half the expected output.
- **Fix sketch**:
  ```rust
  other => {
      tracing::debug!(
          provider = "bedrock",
          variant = ?std::mem::discriminant(&other),
          "Bedrock ConverseStreamOutput variant not handled; dropping"
      );
      Ok(Vec::new())
  }
  ```
- **Reference**: [ConverseStreamOutput — Amazon Bedrock](https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ConverseStreamOutput.html).

### [MEDIUM] No `LlmEvent::ServerModel` emission when Bedrock rewrites the model id
- **Location**: `crates/squeezy-llm/src/bedrock.rs:209-253`
- **Observed**: The stream loop never emits `LlmEvent::ServerModel`. `ServerModelEcho` is defined in `lib.rs:628-657` and consumed by `retry.rs` for skip-state suppression on reconnect.
- **Issue**: When the caller passes a foundation-model id and Bedrock routes through an inference profile (or when an application-inference-profile ARN routes to a different backing foundation model — see `getInferenceProfileBackingModel` in clear-code), the actual model that produced the turn is not surfaced to the TUI, transcript, or cost-attribution logic.
- **Impact**: Transcripts say "this turn was Claude Haiku 4.5" when Bedrock actually routed to `claude-haiku-4-5-20251001-v1:0` in `eu-central-1` via `eu.anthropic.claude-haiku-4-5`. Cost attribution can also drift since application inference profiles can backfill from different models.
- **Fix sketch**: When `messageStop.additionalModelResponseFields` carries an echoed model id (Bedrock sometimes includes one in metadata), feed it through `ServerModelEcho::observe`. Also: when the caller's `model` was rewritten by an auto-prefix helper (see HIGH-2 above), emit `ServerModel` with the resolved id.
- **Reference**: `crates/squeezy-llm/src/lib.rs:628-657`, `others/clear-code/src/utils/model/bedrock.ts:141-176`.

### [MEDIUM] Image block discards `image/jpg` synonym silently elsewhere; bedrock_image_block doesn't enforce the 3.75 MB image limit
- **Location**: `crates/squeezy-llm/src/bedrock.rs:680-699`
- **Observed**: `bedrock_image_block` maps four MIME types to `ImageFormat`. It does **not** check `bytes.len()`.
- **Issue**: Bedrock Converse rejects images larger than the per-model image cap (3.75 MB for Claude; 20 MB for Nova; ~5 MB combined message size). Sending a too-big image surfaces as a `ValidationException` from the AWS SDK rather than a structured local error, and the AWS SDK error string is opaque.
- **Impact**: Users see "ValidationException: …" without an actionable hint about which image was too big.
- **Fix sketch**: After the format match, `if bytes.len() > BEDROCK_IMAGE_MAX_BYTES { return Err(SqueezyError::ProviderRequest(format!("image is {} bytes, exceeds Bedrock {} byte limit", bytes.len(), BEDROCK_IMAGE_MAX_BYTES))) }`. Make the limit per-vendor (Claude/Nova) once we have model metadata.
- **Reference**: [API restrictions — Amazon Bedrock](https://docs.aws.amazon.com/bedrock/latest/userguide/inference-api-restrictions.html).

### [MEDIUM] Reasoning signature accumulation may produce non-canonical concatenation
- **Location**: `crates/squeezy-llm/src/bedrock.rs:373-378`
- **Observed**: 
  ```rust
  ReasoningContentBlockDelta::Signature(sig) => {
      match block.signature.as_mut() {
          Some(existing) => existing.push_str(&sig),
          None => block.signature = Some(sig),
      }
  ```
- **Issue**: Anthropic's reasoning `signature` is a *full* opaque base64 token attached to the closing reasoning block — not a streaming buffer. Concatenating multiple `Signature` deltas yields a corrupted signature that Anthropic will reject on the next turn when squeezy replays the reasoning chain. The Anthropic-native provider (`anthropic.rs`) treats signature as a single-emit field.
- **Impact**: If Bedrock ever splits the signature across two `Signature` deltas (today it doesn't, but the API is non-exhaustive), the next-turn replay fails with `invalid signature` and the entire reasoning chain is invalidated.
- **Fix sketch**: Treat any later `Signature` delta as authoritative replacement rather than append, and `tracing::warn!` when overwriting; the Anthropic semantics expect "first signature wins, opaque blob."
- **Reference**: Cross-check `crates/squeezy-llm/src/anthropic.rs` handling of `signature_delta` events.

### [MEDIUM] `usage.input_tokens` accounting is non-Bedrock-canonical (double-counts cache writes)
- **Location**: `crates/squeezy-llm/src/bedrock.rs:283-303`, `419-430`
- **Observed**:
  ```rust
  let total_input = base.map(|b| b.saturating_add(cache_read).saturating_add(cache_write));
  ```
  The comment claims Bedrock follows Anthropic's "uncached delta only" semantics.
- **Issue**: The Bedrock Converse API documentation (and opencode's mapping in `others/opencode/packages/llm/src/protocols/bedrock-converse.ts:405-418`) state that **`usage.inputTokens` is the inclusive total**, with `cacheReadInputTokens` and `cacheWriteInputTokens` as *subsets*. opencode subtracts to get non-cached: `nonCached = inputTokens - (cacheRead + cacheWrite)`. squeezy adds them, so it double-counts cache reads/writes when reporting `CostSnapshot.input_tokens`.
- **Impact**: Cost telemetry on Bedrock over-counts input tokens by `cacheRead + cacheWrite` per turn. For a heavy cache-hit workflow this can be 80% inflation of reported input-token counts.
- **Fix sketch**: Drop the `saturating_add` and surface `input_tokens` as-received; keep `cached_input_tokens` / `cache_write_input_tokens` populated. Adjust the comment to reflect the correct convention. The cross-provider `CostSnapshot` semantics should also be revisited against Anthropic native — if native truly reports uncached-only, that's the field to harmonize *up* to a total in the per-provider mapper, not the other way around.
- **Reference**: [TokenUsage — Boto3 docs](https://boto3.amazonaws.com/v1/documentation/api/latest/reference/services/bedrock-runtime/client/converse_stream.html), `others/opencode/packages/llm/src/protocols/bedrock-converse.ts:405-418`.

### [MEDIUM] Cache breakpoint budget exceeds Bedrock's 4-breakpoint hard cap
- **Location**: `crates/squeezy-llm/src/bedrock.rs:144-159, 465-477, 638-672`
- **Observed**: Squeezy emits up to 1 system + 1 last-user + 1 tools-tail = 3 cache points when caching is on. No budget tracking, no warning when more would be needed.
- **Issue**: Bedrock's hard cap is 4 cachePoints per request, and the order is `tools → system → messages` (longer-TTL must precede shorter-TTL). opencode tracks a budget counter with `dropped` warnings (`others/opencode/packages/llm/src/protocols/utils/bedrock-cache.ts:19,29-34`). Today squeezy is fine because the auto-policy only inserts 3 markers, but **if a future per-skill cache policy adds a per-message breakpoint or `mcp__`-prefix splitting**, the count can exceed 4 and Bedrock will 400 the whole request.
- **Impact**: Latent bomb — any future cache-policy extension that emits a 5th breakpoint will hard-fail at request time with no graceful degradation.
- **Fix sketch**: Track a `Breakpoints { remaining: usize, dropped: usize }` struct (mirror opencode's), thread through helpers, and `tracing::warn!` when `dropped > 0`.
- **Reference**: [Prompt caching for faster model inference — Amazon Bedrock](https://docs.aws.amazon.com/bedrock/latest/userguide/prompt-caching.html), `others/opencode/packages/llm/src/protocols/utils/bedrock-cache.ts:16-35`.

### [MEDIUM] No `toolChoice` plumbing on Bedrock route
- **Location**: `crates/squeezy-llm/src/bedrock.rs:638-672`
- **Observed**: `tool_configuration` populates `set_tools` but never `tool_choice`.
- **Issue**: `LlmRequest.tool_choice` (`lib.rs:159`) is forwarded by Anthropic / OpenAI / Google but silently dropped on Bedrock. Bedrock's `ToolConfiguration.tool_choice` accepts `Auto`, `Any`, or `Tool { name }`. Tool-shy models (Mistral / Nova) benefit from `Any` to force a tool call.
- **Impact**: `tool_choice="required"` is silently ignored on Bedrock; an agent that relies on it gets free-form text instead of a tool call.
- **Fix sketch**: Mirror the opencode mapping (`others/opencode/packages/llm/src/protocols/bedrock-converse.ts:232-238`):
  ```rust
  match request.tool_choice.as_deref() {
      Some("auto") => Some(ToolChoice::Auto(AutoToolChoice::builder().build())),
      Some("required") | Some("any") => Some(ToolChoice::Any(AnyToolChoice::builder().build())),
      Some(name) if !name.is_empty() => Some(ToolChoice::Tool(SpecificToolChoice::builder().name(name).build()?)),
      _ => None,
  }
  ```
- **Reference**: [ToolChoice — Converse](https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ToolChoice.html).

### [LOW] `recv_event` collapses all Smithy stream errors into one string
- **Location**: `crates/squeezy-llm/src/bedrock.rs:256-266`
- **Issue**: Loses the structured discriminator (transport vs deserialization vs Smithy error frame). Reduced observability when triaging stream failures.
- **Fix sketch**: Downcast to `ConverseStreamOutputError` and emit distinct error variants.

### [LOW] `hex_encode` uses per-byte `format!` rather than `write!`
- **Location**: `crates/squeezy-llm/src/bedrock.rs:437-444`
- **Issue**: Per-byte `String` allocation; wasteful on large redacted-reasoning blobs.
- **Fix sketch**: `std::fmt::Write::write!(&mut out, "{b:02x}").unwrap();` or pull in `hex` crate.

### [LOW] `idle_timeout` 300 s applies uniformly; high-reasoning turns may spuriously time out
- **Location**: `crates/squeezy-llm/src/bedrock.rs:221`, `crates/squeezy-core/src/lib.rs:211`
- **Issue**: Adaptive-thinking Claude 4.6 can think for >5 minutes between visible deltas at max effort. Trips `Bedrock stream idle timeout`.
- **Fix sketch**: Scale idle timeout when `reasoning_effort=High` and model is adaptive-thinking; long term track bytes/minute not last-event ts.

### [NIT] Doc comment on `cost()` mislabels Bedrock as Anthropic-uncached-only convention
- **Location**: `crates/squeezy-llm/src/bedrock.rs:284-291`
- **Issue**: Source of the MEDIUM token-double-counting bug above. Rewrite to "Bedrock reports inclusive total; we surface as-is."

### [NIT] No Bedrock-specific orphan-tool-replay test
- **Location**: `crates/squeezy-llm/src/bedrock_tests.rs`
- **Issue**: `normalize_tool_ids_for_replay` placeholder shape isn't asserted at the Bedrock-lowering layer where `ToolUseBlock::builder().input(...)` would emit `{"reason":"model_switched"}` as a Smithy `Document`.

### [NIT] No `tracing::span!` to scope per-request `model`/`region` fields
- **Location**: `crates/squeezy-llm/src/bedrock.rs:126-253`
- **Issue**: Cross-cutting; matches the rest of squeezy-llm. Easy to fix per-provider.

## Test Coverage Gaps
- **`inferenceConfig.maxTokens` round-trip**: no test asserts `max_output_tokens` lands on the wire input. *Severity*: critical (blocking item-1). *Mockable*: yes, via `ConverseStreamInputBuilder` (already used in `bedrock_tests.rs:464-468`).
- **Inference profile prefix rewriting**: no helper exists yet; once added, tests can assert `us.anthropic.claude-...` is preserved verbatim and `anthropic.claude-...` is auto-prefixed based on region. *Severity*: high. *Mockable*: pure-function tests, no AWS.
- **Mid-stream `ModelStreamErrorException` retry**: no test for the stream-retry wrapper on Bedrock. *Severity*: high. *Mockable*: yes, by constructing a synthetic `ConverseStreamOutput` sequence via a custom `EventReceiver` mock or by injecting a `try_stream!` that errors mid-flight before any retry plumbing is added.
- **Adaptive-thinking schema selection**: assert that `claude-opus-4-6-...` emits `thinking={type:adaptive}, output_config.effort=...` and that `claude-sonnet-4-0-...` emits `thinking={type:enabled, budget_tokens=...}`. *Severity*: high. *Mockable*: extra_fields inspection on the builder.
- **`CacheRetention::Long` honored**: assert that `cache_point_block()` carries `ttl: 1h` when retention is `Long`. *Severity*: critical. *Mockable*: pure-function.
- **Bedrock cache-budget overflow warning**: emit a fixture with 5 desired breakpoints and assert the 5th is dropped with a `tracing::warn`. *Severity*: medium. *Mockable*: yes.
- **Unsupported MIME on documents**: parallels the existing `conversation_messages_reject_unknown_image_mime` once documents land. *Severity*: medium. *Mockable*: yes.
- **`toolChoice` round-trip**: confirm `tool_choice="required"` reaches `ToolChoice::Any`. *Severity*: medium. *Mockable*: yes.
- **Token-usage accounting**: feed a Metadata event with `inputTokens=1000, cacheReadInputTokens=900, cacheWriteInputTokens=50` and assert `CostSnapshot.input_tokens == 1000`. *Severity*: medium. *Mockable*: yes — `handle_bedrock_event` already takes a state ref.
- **Bearer-token rotation**: simulate two `client()` calls with different `AWS_BEARER_TOKEN_BEDROCK` values; expected behavior: second call uses the new token. *Severity*: high. *Mockable*: yes if `bearer_token` is sourced per-call instead of cached.
- **`ServerModel` echo on inference-profile resolution**: scaffold once inference-profile plumbing lands. *Severity*: medium.
- **Idle timeout under reasoning effort**: deterministic test that the per-call timeout scales when `reasoning_effort=High` AND model is adaptive-thinking. *Severity*: low.

## Verification Strategy

Without AWS Bedrock entitlements you can still validate every finding above:

1. **Pure-function tests in `bedrock_tests.rs`**: 11 of 12 findings can be exercised via the SDK's builder-side `ConverseStreamInputBuilder::default().build()` pattern already used at `bedrock_tests.rs:464-468`. Inspect the built `ConverseStreamInput` for `inference_config`, `tool_config.tool_choice`, `additional_model_request_fields["thinking"]`, etc. No network.
2. **Event-stream replay**: `handle_bedrock_event` is `pub(super)` and takes a `&mut BedrockStreamState`. Build synthetic `ConverseStreamOutput::*` variants (the existing `metadata_event_records_usage_tokens` test already does this at `bedrock_tests.rs:362-378`) and feed token-accounting / signature-accumulation fixtures through it.
3. **Free AWS endpoint smoke**: `bedrock-runtime ListFoundationModels` is free (it's on the *control-plane* `bedrock` client, not `bedrock-runtime`), so a hosted CI runner with AWS credentials can validate region/auth/endpoint resolution without burning model-invocation quota. The existing `bedrock_costly.rs` test is hard-gated on `SQUEEZY_RUN_COSTLY_TESTS=1`; add a `bedrock_smoke.rs` for free-tier endpoint checks behind a separate gate (`SQUEEZY_RUN_FREE_TESTS=1`).
4. **Inference-profile prefix mappers**: implement & test as pure functions (region → prefix), and verify against the static prefix list (`us`, `eu`, `apac`, `jp`, `global`) from the AWS docs.
5. **Reasoning-schema selection**: replicate the Anthropic-provider check (`model_uses_adaptive_thinking`) once exposed publicly; pure-function tests don't need network.
6. **Concurrent timeout & retry**: drive `with_stream_retry` over a `futures_util::stream::iter` that emits a transient `SqueezyError::ProviderStream` mid-flow; assert the cursor skips the already-yielded prefix on the second attempt.

## References

- [ConverseStream](https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ConverseStream.html), [ConverseStreamOutput](https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ConverseStreamOutput.html), [Inference using Converse API](https://docs.aws.amazon.com/bedrock/latest/userguide/conversation-inference.html)
- [InferenceConfiguration](https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_InferenceConfiguration.html), [Prompt caching](https://docs.aws.amazon.com/bedrock/latest/userguide/prompt-caching.html), [DocumentBlock](https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_DocumentBlock.html)
- [Adaptive thinking](https://docs.aws.amazon.com/bedrock/latest/userguide/claude-messages-adaptive-thinking.html), [Claude 3.7 Sonnet reasoning with Converse](https://docs.aws.amazon.com/bedrock/latest/userguide/bedrock-runtime_example_bedrock-runtime_Converse_AnthropicClaudeReasoning_section.html)
- [API restrictions](https://docs.aws.amazon.com/bedrock/latest/userguide/inference-api-restrictions.html), [API keys](https://docs.aws.amazon.com/bedrock/latest/userguide/api-keys.html), [Models at a glance](https://docs.aws.amazon.com/bedrock/latest/userguide/conversation-inference-supported-models-features.html)
- [CachePointBlock — Rust SDK](https://docs.rs/aws-sdk-bedrockruntime/latest/aws_sdk_bedrockruntime/types/struct.CachePointBlock.html), [ConverseStreamError — Rust SDK](https://docs.rs/aws-sdk-bedrockruntime/latest/aws_sdk_bedrockruntime/operation/converse_stream/enum.ConverseStreamError.html)
- [Configuring retries — Rust SDK](https://docs.aws.amazon.com/sdk-for-rust/latest/dg/retries.html), [Troubleshooting Bedrock API Errors](https://docs.aws.amazon.com/bedrock/latest/userguide/troubleshooting-api-error-codes.html)
- Reference impls under `/Users/abbassabra/esqueezy/others/`: `clear-code/src/utils/model/bedrock.ts`, `opencode/packages/llm/src/protocols/bedrock-converse.ts`, `opencode/packages/llm/src/protocols/utils/{bedrock-cache,bedrock-media}.ts`, `codex/codex-rs/aws-auth/src/lib.rs`, `pi/packages/ai/src/bedrock-provider.ts`.

### Verified: OK
- `system_blocks` skips emission when instructions are blank (`bedrock.rs:469-471`) — Bedrock rejects empty system blocks.
- Image MIME parsing is case-insensitive and rejects unknown MIME with an actionable error (`bedrock.rs:680-699`).
- `AWS_BEARER_TOKEN_BEDROCK` whitespace-trim + empty-rejection well tested (`bedrock_tests.rs:530-549`).
- `bedrock_extra_body_betas` drops header-only betas (`bedrock_tests.rs:343-360`).
- Multi-block coalescing for consecutive same-role turns (`bedrock_tests.rs:54-70`).
- `append_cache_point_to_last_user` only marks the most-recent user message (`bedrock_tests.rs:108-143`).
- Tool cache point skips trailing `mcp__`-prefixed tools (`bedrock_tests.rs:242-308`).
- `cancel.cancelled()` is observed at `client()`, `builder.send()`, and every event poll (`bedrock.rs:133-222`).
- `state.saw_message_stop` enforces stream completion vs silent truncation (`bedrock.rs:231-234`).
- `StopReason::from_bedrock` correctly maps `guardrail_intervened`/`content_filtered` to `Refusal` (`lib.rs:539`).
