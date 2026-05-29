use std::collections::BTreeMap;
use std::sync::Arc;

use aws_config::{BehaviorVersion, SdkConfig};
use aws_sdk_bedrockruntime::config::{Credentials, Region, SharedCredentialsProvider};
use aws_sdk_bedrockruntime::types::{
    CachePointType, ContentBlock, ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStopEvent,
    ConversationRole, ConverseStreamMetadataEvent, ConverseStreamOutput, MessageStopEvent,
    ReasoningContentBlockDelta, StopReason, SystemContentBlock, TokenUsage, ToolInputSchema,
};
use aws_smithy_types::{Document, Number};
use serde_json::json;
use squeezy_core::SqueezyError;

use aws_sdk_bedrockruntime::operation::converse_stream::builders::ConverseStreamInputBuilder;
use squeezy_core::{BedrockConfig, BedrockThinkingDisplay, ProviderTransportConfig};

use super::{
    BedrockProvider, BedrockStreamState, bedrock_request_metadata_map, build_bedrock_client,
    conversation_messages, handle_bedrock_event, json_to_document, system_blocks,
    tool_configuration,
};
use crate::anthropic_betas::bedrock_extra_body_betas;
use crate::{LlmEvent, LlmInputItem, LlmToolSpec, ReasoningKind, ReasoningPayload};

#[test]
fn system_blocks_skip_blank_instructions() {
    assert!(system_blocks("", false).expect("ok").is_empty());
    assert!(system_blocks("   \n  ", false).expect("ok").is_empty());
    assert!(system_blocks("", true).expect("ok").is_empty());
}

#[test]
fn system_blocks_emit_single_text_block() {
    let blocks = system_blocks("be helpful", false).expect("ok");
    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        SystemContentBlock::Text(text) => assert_eq!(text, "be helpful"),
        _ => panic!("expected Text system block"),
    }
}

#[test]
fn system_blocks_append_cache_point_when_caching_enabled() {
    let blocks = system_blocks("be helpful", true).expect("ok");
    assert_eq!(blocks.len(), 2);
    assert!(matches!(&blocks[0], SystemContentBlock::Text(text) if text == "be helpful"));
    let SystemContentBlock::CachePoint(cache_point) = &blocks[1] else {
        panic!("expected CachePoint after system text, got {:?}", blocks[1]);
    };
    assert_eq!(*cache_point.r#type(), CachePointType::Default);
}

#[test]
fn conversation_messages_merge_consecutive_user_turns() {
    let messages = conversation_messages(
        &[
            LlmInputItem::UserText("hello".to_string()),
            LlmInputItem::UserText("again".to_string()),
            LlmInputItem::AssistantText("hi".to_string()),
        ],
        false,
    )
    .expect("build messages");

    assert_eq!(messages.len(), 2);
    assert_eq!(*messages[0].role(), ConversationRole::User);
    assert_eq!(messages[0].content().len(), 2);
    assert_eq!(*messages[1].role(), ConversationRole::Assistant);
}

#[test]
fn conversation_messages_round_trip_tool_call_and_result() {
    let messages = conversation_messages(
        &[
            LlmInputItem::FunctionCall {
                call_id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: json!({"query": "rust"}),
            },
            LlmInputItem::FunctionCallOutput {
                call_id: "call_1".to_string(),
                output: "ok".to_string(),
            },
        ],
        false,
    )
    .expect("build messages");

    assert_eq!(messages.len(), 2);
    assert_eq!(*messages[0].role(), ConversationRole::Assistant);
    match &messages[0].content()[0] {
        ContentBlock::ToolUse(tool) => {
            assert_eq!(tool.tool_use_id(), "call_1");
            assert_eq!(tool.name(), "search");
        }
        _ => panic!("expected tool use block"),
    }
    assert_eq!(*messages[1].role(), ConversationRole::User);
    match &messages[1].content()[0] {
        ContentBlock::ToolResult(result) => {
            assert_eq!(result.tool_use_id(), "call_1");
        }
        _ => panic!("expected tool result block"),
    }
}

#[test]
fn conversation_messages_append_cache_point_to_last_user_message() {
    let messages = conversation_messages(
        &[
            LlmInputItem::UserText("first".to_string()),
            LlmInputItem::AssistantText("ack".to_string()),
            LlmInputItem::UserText("second".to_string()),
        ],
        true,
    )
    .expect("build messages");

    assert_eq!(messages.len(), 3);
    let final_user = messages.last().expect("at least one message");
    assert_eq!(*final_user.role(), ConversationRole::User);
    let content = final_user.content();
    assert_eq!(content.len(), 2, "user text + cache point block");
    assert!(matches!(&content[0], ContentBlock::Text(text) if text == "second"));
    let ContentBlock::CachePoint(cache_point) = &content[1] else {
        panic!(
            "expected trailing CachePoint on final user message, got {:?}",
            content[1]
        );
    };
    assert_eq!(*cache_point.r#type(), CachePointType::Default);

    // No other message should carry a cache point.
    let mid_user = &messages[0];
    assert_eq!(*mid_user.role(), ConversationRole::User);
    for block in mid_user.content() {
        assert!(
            !matches!(block, ContentBlock::CachePoint(_)),
            "earlier user message should not carry a cache point"
        );
    }
}

#[test]
fn conversation_messages_skip_cache_point_when_no_user_message() {
    let messages = conversation_messages(&[LlmInputItem::AssistantText("solo".to_string())], true)
        .expect("build messages");

    assert_eq!(messages.len(), 1);
    assert_eq!(*messages[0].role(), ConversationRole::Assistant);
    for block in messages[0].content() {
        assert!(
            !matches!(block, ContentBlock::CachePoint(_)),
            "assistant message should not carry a cache point"
        );
    }
}

#[test]
fn tool_configuration_round_trips_json_schema() {
    let specs = vec![
        LlmToolSpec {
            name: "search".to_string(),
            description: "Web search".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": ["query"]
            }),
            strict: false,
        }
        .into(),
    ];
    let config = tool_configuration(&specs, false)
        .expect("ok")
        .expect("present");
    assert_eq!(config.tools().len(), 1);
    let tool_spec = match &config.tools()[0] {
        aws_sdk_bedrockruntime::types::Tool::ToolSpec(spec) => spec,
        other => panic!("expected ToolSpec, got {other:?}"),
    };
    assert_eq!(tool_spec.name(), "search");
    let ToolInputSchema::Json(document) = tool_spec.input_schema().expect("schema") else {
        panic!("expected JSON schema")
    };
    let Document::Object(map) = document else {
        panic!("expected object document");
    };
    assert!(map.contains_key("properties"));
    assert!(map.contains_key("required"));
}

#[test]
fn tool_configuration_appends_cache_point_after_last_tool() {
    let specs = vec![
        LlmToolSpec {
            name: "search".to_string(),
            description: "Web search".to_string(),
            parameters: json!({"type": "object"}),
            strict: false,
        }
        .into(),
        LlmToolSpec {
            name: "lookup".to_string(),
            description: "Lookup".to_string(),
            parameters: json!({"type": "object"}),
            strict: false,
        }
        .into(),
    ];
    let config = tool_configuration(&specs, true)
        .expect("ok")
        .expect("present");
    let tools = config.tools();
    assert_eq!(tools.len(), 3, "two specs plus trailing cache point");
    assert!(matches!(
        &tools[0],
        aws_sdk_bedrockruntime::types::Tool::ToolSpec(spec) if spec.name() == "search"
    ));
    assert!(matches!(
        &tools[1],
        aws_sdk_bedrockruntime::types::Tool::ToolSpec(spec) if spec.name() == "lookup"
    ));
    let aws_sdk_bedrockruntime::types::Tool::CachePoint(cache_point) = &tools[2] else {
        panic!("expected trailing Tool::CachePoint, got {:?}", tools[2]);
    };
    assert_eq!(*cache_point.r#type(), CachePointType::Default);
}

#[test]
fn tool_configuration_returns_none_when_empty() {
    assert!(tool_configuration(&[], false).expect("ok").is_none());
    assert!(
        tool_configuration(&[], true).expect("ok").is_none(),
        "no tools means no tool config, even when caching is requested"
    );
}

#[test]
fn json_to_document_preserves_numeric_kinds() {
    let document = json_to_document(&json!({
        "u": 42,
        "i": -7,
        "f": 3.5,
        "b": true,
        "n": null,
        "arr": [1, "two"],
    }));
    let Document::Object(map) = document else {
        panic!("expected object document");
    };
    assert!(matches!(
        map.get("u"),
        Some(Document::Number(Number::PosInt(42)))
    ));
    assert!(matches!(
        map.get("i"),
        Some(Document::Number(Number::NegInt(-7)))
    ));
    assert!(matches!(
        map.get("f"),
        Some(Document::Number(Number::Float(_)))
    ));
    assert!(matches!(map.get("b"), Some(Document::Bool(true))));
    assert!(matches!(map.get("n"), Some(Document::Null)));
    let Some(Document::Array(arr)) = map.get("arr") else {
        panic!("expected array document");
    };
    assert_eq!(arr.len(), 2);
}

#[test]
fn beta_headers_route_into_extra_body_params_on_bedrock() {
    // Bedrock's gateway strips non-standard HTTP headers; the routing
    // helper must keep only the body-param-eligible subset, which the
    // provider then attaches to `additional_model_request_fields`.
    let betas: Arc<[Arc<str>]> = Arc::from(vec![
        Arc::<str>::from("context-1m-2025-08-07"),
        Arc::<str>::from("interleaved-thinking-2025-05-14"),
        Arc::<str>::from("claude-code-20250219"),
    ]);
    let body_betas = bedrock_extra_body_betas(&betas);
    let body_strs: Vec<&str> = body_betas.iter().map(|b| b.as_ref()).collect();
    assert_eq!(
        body_strs,
        vec!["context-1m-2025-08-07", "interleaved-thinking-2025-05-14"],
        "Bedrock subset must drop header-only betas (claude-code-*) and preserve order",
    );
}

#[test]
fn metadata_event_records_usage_tokens() {
    let mut state = BedrockStreamState::default();
    let usage = TokenUsage::builder()
        .input_tokens(123)
        .output_tokens(45)
        .total_tokens(168)
        .build()
        .expect("build usage");
    let metadata = ConverseStreamMetadataEvent::builder().usage(usage).build();
    let events = handle_bedrock_event(ConverseStreamOutput::Metadata(metadata), &mut state)
        .expect("handle metadata");
    assert!(events.is_empty());
    assert!(state.saw_metadata);
    assert_eq!(state.input_tokens, Some(123));
    assert_eq!(state.output_tokens, Some(45));
}

#[test]
fn message_stop_without_metadata_leaves_usage_unset() {
    let mut state = BedrockStreamState::default();
    let stop = MessageStopEvent::builder()
        .stop_reason(StopReason::EndTurn)
        .build()
        .expect("build stop");
    let events = handle_bedrock_event(ConverseStreamOutput::MessageStop(stop), &mut state)
        .expect("handle stop");
    assert!(events.is_empty());
    assert!(state.saw_message_stop);
    assert!(
        !state.saw_metadata,
        "metadata flag should remain false when only messageStop has arrived"
    );
    let cost = state.cost();
    assert!(
        cost.input_tokens.is_none() && cost.output_tokens.is_none(),
        "cost reports None when Metadata never arrived, signalling missing usage rather than zero"
    );
}

#[test]
fn conversation_messages_emit_image_content_block() {
    use aws_sdk_bedrockruntime::types::ImageFormat;

    let bytes: Arc<[u8]> = Arc::from(vec![
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3,
    ]);
    let messages = conversation_messages(
        &[
            LlmInputItem::UserText("describe this".to_string()),
            LlmInputItem::Image {
                media_type: "image/png".to_string(),
                bytes: bytes.clone(),
            },
        ],
        false,
    )
    .expect("build messages");

    // User text + image coalesce into a single user message with two
    // content blocks so the Converse API sees them as one multimodal
    // turn.
    assert_eq!(messages.len(), 1);
    assert_eq!(*messages[0].role(), ConversationRole::User);
    let content = messages[0].content();
    assert_eq!(content.len(), 2);
    assert!(matches!(&content[0], ContentBlock::Text(text) if text == "describe this"));
    let ContentBlock::Image(image) = &content[1] else {
        panic!(
            "expected ContentBlock::Image for image input, got {:?}",
            content[1]
        );
    };
    assert_eq!(image.format(), &ImageFormat::Png);
    let source = image.source().expect("image source");
    let blob = source.as_bytes().expect("Bytes source");
    assert_eq!(blob.as_ref(), bytes.as_ref());
}

#[test]
fn config_request_metadata_appears_on_converse_stream_input() {
    // Cost-allocation tags supplied on `BedrockConfig.request_metadata`
    // must round-trip through `BedrockProvider` and land on
    // `ConverseStreamInput.request_metadata` so AWS billing can group
    // invocations by the operator's chosen labels. Empty maps stay
    // unset on the wire (the helper returns `None`) so we keep the
    // request payload minimal when no tags are configured.
    let mut tags = BTreeMap::new();
    tags.insert("team".to_string(), "platform".to_string());
    tags.insert("env".to_string(), "prod".to_string());

    let provider = BedrockProvider::from_config(&BedrockConfig {
        region: "us-east-1".to_string(),
        base_url: None,
        bearer_token: None,
        request_metadata: tags.clone(),
        thinking_display: Default::default(),
        transport: ProviderTransportConfig::default(),
    })
    .expect("provider builds from config with request_metadata");

    let sdk_map = bedrock_request_metadata_map(&provider.request_metadata)
        .expect("non-empty config tags must produce a HashMap, not None");
    let input = ConverseStreamInputBuilder::default()
        .model_id("test-model")
        .set_request_metadata(Some(sdk_map))
        .build()
        .expect("ConverseStreamInputBuilder::build is infallible for valid inputs");

    let echoed = input
        .request_metadata()
        .expect("request_metadata must be set on the input");
    assert_eq!(echoed.len(), 2);
    assert_eq!(echoed.get("team").map(String::as_str), Some("platform"));
    assert_eq!(echoed.get("env").map(String::as_str), Some("prod"));

    assert!(
        bedrock_request_metadata_map(&BTreeMap::new()).is_none(),
        "empty config tags must skip the field entirely so unconfigured callers don't ship an empty object"
    );
}

#[test]
fn conversation_messages_reject_unknown_image_mime() {
    // Bedrock's `ImageFormat` enum is `{Png, Jpeg, Gif, Webp}`. Any
    // MIME outside that allowlist must fail loud as a ProviderRequest
    // error with a stable, structured message so callers can surface
    // it directly instead of silently dropping the image or letting
    // the AWS service reject it after the wire call. We pin the
    // documented format here (`"bedrock: unsupported image format
    // '{mime}'; supported: png, jpeg, gif, webp"`) so accidental
    // wording drift surfaces as a test failure rather than a UX
    // regression for operators who script around the error text.
    let bytes: Arc<[u8]> = Arc::from(vec![1u8, 2, 3, 4]);
    let err = conversation_messages(
        &[LlmInputItem::Image {
            media_type: "image/avif".to_string(),
            bytes,
        }],
        false,
    )
    .expect_err("unsupported MIME must surface an explicit ProviderRequest error");
    let SqueezyError::ProviderRequest(message) = &err else {
        panic!("expected ProviderRequest, got {err:?}");
    };
    assert_eq!(
        message, "bedrock: unsupported image format 'image/avif'; supported: png, jpeg, gif, webp",
        "error text must match the documented format exactly",
    );
}

#[test]
fn aws_bearer_token_env_routes_through_bedrock_bearer_auth() {
    // BedrockConfig carries the `AWS_BEARER_TOKEN_BEDROCK` env-var
    // value once squeezy-core has lifted it (squeezy-core owns env
    // discovery; squeezy-llm owns the auth wiring). With a bearer
    // token in hand the provider must build a Bedrock client even
    // when the shared SdkConfig has no SigV4 credentials at all —
    // that is the whole point of the bearer route. We deliberately
    // build a credential-less SdkConfig so a regression that ignores
    // the bearer token would surface as the explicit
    // `ProviderNotConfigured` error rather than an Ok client.
    let bedrock = squeezy_core::BedrockConfig {
        region: squeezy_core::DEFAULT_BEDROCK_REGION.to_string(),
        base_url: None,
        bearer_token: Some("bedrock-api-key-test".to_string()),
        request_metadata: BTreeMap::new(),
        thinking_display: Default::default(),
        transport: squeezy_core::ProviderTransportConfig::default(),
    };
    let provider = crate::BedrockProvider::from_config(&bedrock)
        .expect("BedrockProvider::from_config must accept a bearer-token-only config");
    drop(provider);

    let shared = SdkConfig::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .build();
    assert!(
        shared.credentials_provider().is_none(),
        "test SdkConfig is intentionally credential-less",
    );
    let client = build_bedrock_client(&shared, bedrock.bearer_token.as_deref())
        .expect("bearer-token path must not require AWS credentials");
    drop(client);

    // Leading/trailing whitespace from a shell heredoc must not
    // poison the bearer header — `build_bedrock_client` trims before
    // wrapping the token in the SDK identity type. An all-whitespace
    // bearer is treated as "missing token" and surfaces a clean error
    // instead of an unauthenticated request.
    let trimmed = build_bedrock_client(&shared, Some("  bedrock-api-key-test\n"))
        .expect("whitespace-padded bearer token must be normalised, not rejected");
    drop(trimmed);
    let empty = build_bedrock_client(&shared, Some("   "))
        .expect_err("an all-whitespace bearer token must be rejected explicitly");
    assert!(
        matches!(&empty, SqueezyError::ProviderNotConfigured(message)
            if message.contains("AWS_BEARER_TOKEN_BEDROCK")),
        "empty bearer must surface ProviderNotConfigured mentioning the env var: {empty:?}",
    );
}

#[test]
fn missing_bearer_token_falls_back_to_default_credential_chain() {
    // Without a bearer token the provider trusts whatever the AWS
    // default credential chain resolved (env → ~/.aws/credentials →
    // IMDS / container roles). We exercise both legs of that branch:
    //
    // 1. A credential-bearing SdkConfig must yield Ok(client) so the
    //    SDK can sign with SigV4 the way it always has.
    // 2. A credential-less SdkConfig must surface ProviderNotConfigured
    //    with a message that points operators at the recovery paths
    //    (bearer token env, `aws configure`, AWS_PROFILE, raw env
    //    vars) — silently returning an unusable client would mask the
    //    misconfiguration until the first network request.
    let creds = Credentials::new("AKIDEXAMPLE", "SECRETEXAMPLE", None, None, "squeezy-test");
    let with_creds = SdkConfig::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(SharedCredentialsProvider::new(creds))
        .build();
    let client = build_bedrock_client(&with_creds, None)
        .expect("default chain with credentials must build a client");
    drop(client);

    let without_creds = SdkConfig::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .build();
    let err = build_bedrock_client(&without_creds, None)
        .expect_err("default chain with no credentials must surface ProviderNotConfigured");
    let SqueezyError::ProviderNotConfigured(message) = &err else {
        panic!("expected ProviderNotConfigured, got {err:?}");
    };
    assert!(
        message.contains("AWS_BEARER_TOKEN_BEDROCK"),
        "error must point operators at the bearer-token recovery path: {message}",
    );
    assert!(
        message.contains("aws configure") || message.contains("AWS_PROFILE"),
        "error must also point at the standard credential-chain recovery paths: {message}",
    );
}

/// Construct a `reasoningContent` text-delta event for the given block
/// index. The Bedrock stream handler routes these on
/// `state.thinking_display` so tests can compare event traces across
/// the Raw / Summarized / Hidden variants from the same input fixture.
fn reasoning_text_delta_event(index: i32, text: &str) -> ConverseStreamOutput {
    let delta = ContentBlockDeltaEvent::builder()
        .content_block_index(index)
        .delta(ContentBlockDelta::ReasoningContent(
            ReasoningContentBlockDelta::Text(text.to_string()),
        ))
        .build()
        .expect("ContentBlockDeltaEvent::build is infallible when content_block_index is set");
    ConverseStreamOutput::ContentBlockDelta(delta)
}

fn reasoning_block_stop_event(index: i32) -> ConverseStreamOutput {
    let stop = ContentBlockStopEvent::builder()
        .content_block_index(index)
        .build()
        .expect("ContentBlockStopEvent::build is infallible when content_block_index is set");
    ConverseStreamOutput::ContentBlockStop(stop)
}

/// Replay a deterministic reasoning sequence ("first", "second",
/// "third" on the same block) through `handle_bedrock_event` and
/// return the resulting `ReasoningDelta` text events plus the
/// terminal `ReasoningDone` payload yielded by `flush_reasoning`.
fn drive_reasoning_sequence(
    display: BedrockThinkingDisplay,
) -> (Vec<String>, Option<ReasoningPayload>) {
    let mut state = BedrockStreamState::with_thinking_display(display);
    let mut deltas = Vec::new();
    let sequence = [
        reasoning_text_delta_event(0, "first "),
        reasoning_text_delta_event(0, "second "),
        reasoning_text_delta_event(0, "third"),
        reasoning_block_stop_event(0),
    ];
    for event in sequence {
        let emitted = handle_bedrock_event(event, &mut state).expect("event handles cleanly");
        for llm_event in emitted {
            if let LlmEvent::ReasoningDelta { text, kind } = llm_event {
                assert!(
                    matches!(kind, ReasoningKind::Text),
                    "reasoning text deltas must carry ReasoningKind::Text",
                );
                deltas.push(text);
            } else {
                panic!("unexpected non-reasoning event during reasoning sequence: {llm_event:?}");
            }
        }
    }
    let payload = state.flush_reasoning();
    (deltas, payload)
}

#[test]
fn bedrock_thinking_display_parses_canonical_names_and_aliases() {
    assert_eq!(
        BedrockThinkingDisplay::parse("summarized"),
        Some(BedrockThinkingDisplay::Summarized)
    );
    assert_eq!(
        BedrockThinkingDisplay::parse("Summarized"),
        Some(BedrockThinkingDisplay::Summarized)
    );
    assert_eq!(
        BedrockThinkingDisplay::parse(" default "),
        Some(BedrockThinkingDisplay::Summarized)
    );
    assert_eq!(
        BedrockThinkingDisplay::parse("hidden"),
        Some(BedrockThinkingDisplay::Hidden)
    );
    assert_eq!(
        BedrockThinkingDisplay::parse("off"),
        Some(BedrockThinkingDisplay::Hidden)
    );
    assert_eq!(
        BedrockThinkingDisplay::parse("raw"),
        Some(BedrockThinkingDisplay::Raw)
    );
    assert_eq!(
        BedrockThinkingDisplay::parse("verbatim"),
        Some(BedrockThinkingDisplay::Raw)
    );
    assert_eq!(BedrockThinkingDisplay::parse("nope"), None);
    assert_eq!(
        BedrockThinkingDisplay::default(),
        BedrockThinkingDisplay::Summarized
    );
}

#[test]
fn bedrock_thinking_display_raw_emits_every_chunk_verbatim() {
    let (deltas, payload) = drive_reasoning_sequence(BedrockThinkingDisplay::Raw);
    assert_eq!(
        deltas,
        vec![
            "first ".to_string(),
            "second ".to_string(),
            "third".to_string()
        ],
        "Raw must forward each reasoningContent chunk verbatim, in order"
    );
    let Some(ReasoningPayload::Anthropic { blocks }) = payload else {
        panic!("Raw must still surface ReasoningDone so signed thinking can replay");
    };
    assert_eq!(blocks.len(), 1);
    assert_eq!(
        blocks[0].text, "first second third",
        "Raw payload must contain the full accumulated thinking text"
    );
}

#[test]
fn bedrock_thinking_display_summarized_emits_only_first_chunk() {
    let (deltas, payload) = drive_reasoning_sequence(BedrockThinkingDisplay::Summarized);
    assert_eq!(
        deltas,
        vec!["first ".to_string()],
        "Summarized must emit exactly one ReasoningDelta per block (the first chunk)"
    );
    let Some(ReasoningPayload::Anthropic { blocks }) = payload else {
        panic!("Summarized must still flush ReasoningDone so signed thinking can replay");
    };
    assert_eq!(blocks.len(), 1);
    assert_eq!(
        blocks[0].text, "first second third",
        "Summarized must still accumulate the full thinking text for replay, even though the deltas were elided"
    );
}

#[test]
fn bedrock_thinking_display_hidden_drops_reasoning_entirely() {
    let (deltas, payload) = drive_reasoning_sequence(BedrockThinkingDisplay::Hidden);
    assert!(
        deltas.is_empty(),
        "Hidden must suppress every reasoning delta; saw {deltas:?}"
    );
    assert!(
        payload.is_none(),
        "Hidden must suppress ReasoningDone too; saw {payload:?}"
    );
}

#[test]
fn bedrock_thinking_display_summarized_resets_between_blocks() {
    // A multi-block reasoning trace (e.g. interleaved thinking)
    // should re-emit the first chunk for *each* block under
    // Summarized — the "first" gate is per-block, not per-stream.
    let mut state = BedrockStreamState::with_thinking_display(BedrockThinkingDisplay::Summarized);
    let mut deltas = Vec::new();
    let sequence = [
        reasoning_text_delta_event(0, "alpha-1 "),
        reasoning_text_delta_event(0, "alpha-2"),
        reasoning_block_stop_event(0),
        reasoning_text_delta_event(1, "beta-1 "),
        reasoning_text_delta_event(1, "beta-2"),
        reasoning_block_stop_event(1),
    ];
    for event in sequence {
        for llm_event in handle_bedrock_event(event, &mut state).expect("handle event") {
            if let LlmEvent::ReasoningDelta { text, .. } = llm_event {
                deltas.push(text);
            }
        }
    }
    assert_eq!(
        deltas,
        vec!["alpha-1 ".to_string(), "beta-1 ".to_string()],
        "Summarized must emit the first chunk of each independent reasoning block"
    );
    let Some(ReasoningPayload::Anthropic { blocks }) = state.flush_reasoning() else {
        panic!("expected ReasoningDone payload for the two finalized blocks");
    };
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].text, "alpha-1 alpha-2");
    assert_eq!(blocks[1].text, "beta-1 beta-2");
}

#[test]
fn bedrock_thinking_display_default_matches_summarized() {
    // The default policy (used when TOML / env omit
    // `thinking_display`) must equal `Summarized`. We verify by
    // round-tripping through `BedrockStreamState::default` (no
    // explicit display value) and confirming the second chunk drops.
    let mut state = BedrockStreamState::default();
    let emit_first = handle_bedrock_event(reasoning_text_delta_event(0, "alpha"), &mut state)
        .expect("first chunk handles cleanly");
    let emit_second = handle_bedrock_event(reasoning_text_delta_event(0, "beta"), &mut state)
        .expect("second chunk handles cleanly");
    assert!(
        matches!(emit_first.as_slice(), [LlmEvent::ReasoningDelta { text, .. }] if text == "alpha"),
        "default state must emit the first chunk (Summarized policy): {emit_first:?}"
    );
    assert!(
        emit_second.is_empty(),
        "default state must drop subsequent chunks under the Summarized policy: {emit_second:?}"
    );
}
