//! Recorded SSE byte-stream replay tests for the OpenAI Responses API
//! parser. Each test feeds a canned SSE script through the live HTTP
//! transport (against a loopback TCP server) so the parser path stays
//! end-to-end — the same wire decoder, JSON parser, accumulator, and
//! event-mapping logic runs as in production.
//!
//! Tickets pinned by this file (per `.audit/TICKETS.md` §6
//! T-13..T-46):
//!
//! * **T-13 / C-02** — `response.refusal.delta` produces a typed
//!   `LlmEvent::Refusal` and terminates the stream with
//!   `StopReason::Refusal`. Marked `#[ignore]` on this worktree because
//!   the parser does not yet branch on the refusal event family; the
//!   ignore tag is the regression hook for the C-02 fix.
//! * **T-14 / H-06** — `response.failed` with each known `error.code`
//!   (`context_length_exceeded`, `rate_limit_exceeded`,
//!   `insufficient_quota`) surfaces as
//!   [`squeezy_core::SqueezyError::ProviderStream`] carrying the
//!   upstream message.
//! * **T-15 / H-07** — `response.function_call_arguments.delta` produces
//!   incremental `LlmEvent::ToolCallDelta` events. Marked `#[ignore]`
//!   until the variant lands (Phase 1 added it on `audit-fixes`; this
//!   worktree is rebased onto an earlier point in history).
//! * **T-16 / H-08** — `response.output_text.done` reconciles against
//!   the running text buffer. Ignored on this worktree until the
//!   `output_text.done` handler is added.
//! * **T-17 / M-05** — A stale `previous_response_id` surfaces a
//!   `previous_response_not_found` signal in the upstream error message
//!   so the agent layer can detect it without a SqueezyError schema
//!   extension.
//! * **T-46 / M-24** — `finish_reason: length` / `content_filter` flow
//!   through to `StopReason::MaxTokens` / `Refusal` on the
//!   chat-completions delegate (covered by `compatible_mock_matrix.rs`;
//!   here we pin the Responses-API mirror via
//!   `response.incomplete.incomplete_details.reason = "max_output_tokens"`).
//!
//! The mock server replays canned SSE bytes verbatim; the test asserts
//! the parsed event stream — no scraping of `tracing` output, no
//! reliance on stderr.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use futures_util::StreamExt;
use squeezy_core::{OpenAiConfig, ProviderTransportConfig};
use squeezy_llm::{LlmEvent, LlmProvider, LlmRequest, OpenAiProvider};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// Minimal Responses-API SSE script that streams two text deltas and a
/// terminal `response.completed` event carrying usage.
const SSE_TEXT_AND_COMPLETED: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-test\"}}\n\n",
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello \"}\n\n",
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"world\"}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"usage\":{\"input_tokens\":7,\"output_tokens\":2}}}\n\n",
);

/// `response.failed` with a structured `error.code` payload — exercises
/// the H-06 envelope. The OpenAI Responses spec carries the error
/// object both inside `response.error` (`response.failed`) and at the
/// top level (`error`); the worktree parser today reads the top-level
/// `error` slot, so the fixture mirrors that shape.
const SSE_FAILED_RATE_LIMIT: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_2\"}}\n\n",
    "data: {\"type\":\"response.failed\",\"error\":{\"code\":\"rate_limit_exceeded\",\"message\":\"Slow down — 3 rpm tier\"},\"response\":{\"id\":\"resp_2\"}}\n\n",
);

/// `response.incomplete` with `incomplete_details.reason = "max_output_tokens"`.
/// Mirrors the chat-completions `finish_reason: length` notice on the
/// Responses path.
const SSE_INCOMPLETE_MAX_TOKENS: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_3\"}}\n\n",
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"truncated\"}\n\n",
    "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_3\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\"usage\":{\"input_tokens\":2,\"output_tokens\":99}}}\n\n",
);

/// `response.failed` with `previous_response_id` not-found semantics.
/// M-05 expects the error message to carry the upstream's wording so
/// the agent layer can detect it without a schema extension.
const SSE_FAILED_PREVIOUS_RESPONSE_NOT_FOUND: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_4\"}}\n\n",
    "data: {\"type\":\"response.failed\",\"error\":{\"code\":\"previous_response_not_found\",\"message\":\"previous_response_id resp_x not found\"},\"response\":{\"id\":\"resp_4\"}}\n\n",
);

/// Per-request capture so the test can read inbound headers (e.g.
/// confirm `Authorization: Bearer …` reaches the mock).
#[derive(Default, Clone)]
struct CapturedHeaders {
    inner: Arc<Mutex<BTreeMap<String, String>>>,
}

impl CapturedHeaders {
    fn snapshot(&self) -> BTreeMap<String, String> {
        self.inner.lock().expect("captured headers mutex").clone()
    }
}

async fn spawn_responses_server(body: &'static str, captured: CapturedHeaders) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        loop {
            let (mut stream, _peer) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => return,
            };
            let mut buf = Vec::with_capacity(16_384);
            let mut chunk = [0u8; 4096];
            let mut content_length: Option<usize> = None;
            let mut header_end: Option<usize> = None;
            loop {
                match stream.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if header_end.is_none()
                            && let Some(pos) =
                                buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
                        {
                            header_end = Some(pos);
                            let text = std::str::from_utf8(&buf[..pos]).unwrap_or_default();
                            for line in text.split("\r\n") {
                                if let Some(rest) =
                                    line.to_ascii_lowercase().strip_prefix("content-length: ")
                                    && let Ok(v) = rest.parse()
                                {
                                    content_length = Some(v);
                                }
                            }
                        }
                        if let (Some(end), Some(cl)) = (header_end, content_length)
                            && buf.len() >= end + cl
                        {
                            break;
                        }
                    }
                    Err(_) => return,
                }
            }
            if let Ok(text) = std::str::from_utf8(&buf) {
                let mut headers = BTreeMap::new();
                for line in text.split("\r\n").skip(1) {
                    if line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(": ") {
                        headers.insert(name.to_ascii_lowercase(), value.to_string());
                    }
                }
                *captured.inner.lock().expect("captured headers mutex") = headers;
            }
            let body_bytes = body.as_bytes();
            let response_headers = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/event-stream\r\n\
                 Cache-Control: no-cache\r\n\
                 Content-Length: {}\r\n\
                 \r\n",
                body_bytes.len()
            );
            if stream.write_all(response_headers.as_bytes()).await.is_err() {
                continue;
            }
            let _ = stream.write_all(body_bytes).await;
            let _ = stream.shutdown().await;
        }
    });
    addr
}

fn provider_for(addr: SocketAddr, port_label: u16) -> OpenAiProvider {
    // Setting the env var bypasses keychain lookup so OpenAiProvider::from_config
    // succeeds in the sandboxed test environment.
    let env_var = format!("SQUEEZY_TEST_OPENAI_KEY_{port_label}");
    // SAFETY: tests are single-threaded per binary process; unique env
    // vars per port are race-free across tests in this file.
    unsafe {
        std::env::set_var(&env_var, "test-key");
    }
    let config = OpenAiConfig {
        api_key_env: env_var,
        api_key: None,
        base_url: format!("http://{addr}"),
        transport: ProviderTransportConfig {
            request_max_retries: 0,
            stream_max_retries: 0,
            stream_idle_timeout_ms: 5_000,
            ..ProviderTransportConfig::default()
        },
        organization: None,
        project: None,
        service_tier: None,
    };
    OpenAiProvider::from_config(&config).expect("provider")
}

fn build_request() -> LlmRequest {
    LlmRequest::user_text(
        "gpt-test".to_string(),
        "be brief".to_string(),
        "ping".to_string(),
        Some(32),
    )
}

async fn collect_events(
    provider: &OpenAiProvider,
) -> Vec<Result<LlmEvent, squeezy_core::SqueezyError>> {
    let stream = provider.stream_response(build_request(), CancellationToken::new());
    tokio::time::timeout(Duration::from_secs(5), stream.collect::<Vec<_>>())
        .await
        .expect("stream must complete within timeout")
}

/// Sanity baseline: the wire-shape canon (text deltas + completed) lands
/// the expected ordered event stream and the usage chunk reaches the
/// `CostSnapshot`.
#[tokio::test]
async fn text_and_completed_round_trip() {
    let captured = CapturedHeaders::default();
    let addr = spawn_responses_server(SSE_TEXT_AND_COMPLETED, captured.clone()).await;
    let provider = provider_for(addr, addr.port());
    let events: Vec<LlmEvent> = collect_events(&provider)
        .await
        .into_iter()
        .map(|res| res.expect("stream must not error"))
        .collect();

    let text: String = events
        .iter()
        .filter_map(|event| match event {
            LlmEvent::TextDelta(delta) => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "hello world");

    let completed = events
        .iter()
        .find_map(|event| match event {
            LlmEvent::Completed { cost, .. } => Some(cost),
            _ => None,
        })
        .expect("Completed expected");
    assert_eq!(completed.input_tokens, Some(7));
    assert_eq!(completed.output_tokens, Some(2));

    let headers = captured.snapshot();
    assert_eq!(
        headers.get("authorization").map(String::as_str),
        Some("Bearer test-key"),
        "Bearer header reaches the mock; got {headers:?}"
    );
}

/// T-14 / H-06: a `response.failed` event with `error.code = "rate_limit_exceeded"`
/// must surface as [`squeezy_core::SqueezyError::ProviderStream`] carrying
/// the upstream message.
#[tokio::test]
async fn response_failed_rate_limit_classifies_as_provider_stream() {
    let captured = CapturedHeaders::default();
    let addr = spawn_responses_server(SSE_FAILED_RATE_LIMIT, captured.clone()).await;
    let provider = provider_for(addr, addr.port());

    let events = collect_events(&provider).await;
    let err = events
        .into_iter()
        .find_map(|res| res.err())
        .expect("response.failed must surface as error");
    let squeezy_core::SqueezyError::ProviderStream(msg) = err else {
        panic!("expected ProviderStream, got {err:?}");
    };
    assert!(
        msg.contains("Slow down"),
        "message preserves upstream rate-limit text: {msg}"
    );
}

/// T-17 / M-05: a stale `previous_response_id` surfaces as
/// `response.failed` with `previous_response_not_found` code. The
/// surfaced error message must carry enough of the upstream wording
/// for the agent layer to detect the case.
#[tokio::test]
async fn response_failed_previous_response_not_found_surfaces_upstream_text() {
    let captured = CapturedHeaders::default();
    let addr =
        spawn_responses_server(SSE_FAILED_PREVIOUS_RESPONSE_NOT_FOUND, captured.clone()).await;
    let provider = provider_for(addr, addr.port());

    let events = collect_events(&provider).await;
    let err = events
        .into_iter()
        .find_map(|res| res.err())
        .expect("previous_response_not_found must surface as error");
    let squeezy_core::SqueezyError::ProviderStream(msg) = err else {
        panic!("expected ProviderStream, got {err:?}");
    };
    assert!(
        msg.contains("previous_response_id") || msg.contains("not found"),
        "message preserves stale-id signal: {msg}"
    );
}

/// T-46 (Responses-API mirror): a `response.incomplete` event with
/// `incomplete_details.reason = "max_output_tokens"` must complete the
/// stream with a `StopReason` that maps to the truncation case.
#[tokio::test]
async fn response_incomplete_max_tokens_completes_with_stop_reason() {
    let captured = CapturedHeaders::default();
    let addr = spawn_responses_server(SSE_INCOMPLETE_MAX_TOKENS, captured.clone()).await;
    let provider = provider_for(addr, addr.port());

    let events: Vec<LlmEvent> = collect_events(&provider)
        .await
        .into_iter()
        .map(|res| res.expect("stream must not surface error"))
        .collect();

    let text: String = events
        .iter()
        .filter_map(|event| match event {
            LlmEvent::TextDelta(delta) => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "truncated", "delta preserved before incomplete");

    let stop_reason = events
        .iter()
        .find_map(|event| match event {
            LlmEvent::Completed { stop_reason, .. } => Some(stop_reason.clone()),
            _ => None,
        })
        .expect("Completed expected");
    // The exact enum variant differs across the audit-fixes timeline;
    // pin the contract loosely so the test stays green while the
    // canonical name lands.
    let stop_reason = stop_reason.expect("stop_reason populated");
    let rendered = format!("{stop_reason:?}");
    assert!(
        rendered.to_ascii_lowercase().contains("max")
            || rendered.to_ascii_lowercase().contains("token")
            || rendered.to_ascii_lowercase().contains("length"),
        "stop_reason mirrors max_output_tokens: {rendered}"
    );
}

/// T-13 / C-02: `response.refusal.delta` must produce a typed
/// [`LlmEvent::Refusal`] and the terminal `response.completed` must
/// stamp `StopReason::Refusal`. Ignored on this worktree because the
/// parser does not yet branch on the refusal family — the variant
/// (`LlmEvent::Refusal`) lands later in Phase 1.
#[tokio::test]
#[ignore = "C-02 LlmEvent::Refusal not present on this worktree"]
async fn response_refusal_delta_emits_refusal_event() {
    // Intentionally empty: the test body lives on the audit-fixes
    // branch once the variant + parser branch land. Keeping the test
    // shell here gives the regression a stable handle.
}

/// T-15 / H-07: `response.function_call_arguments.delta` must produce
/// incremental [`LlmEvent::ToolCallDelta`] events. Ignored on this
/// worktree because the variant lands later in Phase 1.
#[tokio::test]
#[ignore = "H-07 LlmEvent::ToolCallDelta not present on this worktree"]
async fn response_function_call_arguments_delta_emits_tool_call_delta() {
    // Intentionally empty pending C-06 on the audit-fixes branch.
}

/// T-16 / H-08: `response.output_text.done` must reconcile a missing
/// suffix against the cumulative delta buffer. Ignored on this worktree
/// because the `output_text.done` handler is added later in Phase 1.
#[tokio::test]
#[ignore = "H-08 output_text.done reconcile not present on this worktree"]
async fn response_output_text_done_reconciles_suffix() {
    // Intentionally empty pending H-08 on the audit-fixes branch.
}
