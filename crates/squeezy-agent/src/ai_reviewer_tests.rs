use super::*;
use squeezy_core::{
    DEFAULT_AI_REVIEWER_MAX_TRANSCRIPT_TOKENS, PermissionCapability, PermissionRequest,
    PermissionRisk,
};

#[test]
fn bounded_transcript_keeps_last_user_with_caps() {
    let mut items = (0..20)
        .map(|index| TranscriptItem::assistant(format!("assistant {index} {}", "a".repeat(2400))))
        .collect::<Vec<_>>();
    items.push(TranscriptItem::user(
        "important final user request".to_string(),
    ));
    let snapshot = AiReviewerTranscriptSnapshot {
        entry_count: items.len(),
        history_version: 0,
        items,
    };
    let rendered = bounded_transcript(&snapshot, None, DEFAULT_AI_REVIEWER_MAX_TRANSCRIPT_TOKENS);
    assert!(rendered.contains("important final user request"));
    assert!(approx_tokens(&rendered) <= DEFAULT_AI_REVIEWER_MAX_TRANSCRIPT_TOKENS + 200);
    assert!(!rendered.contains("assistant 0"));
}

#[test]
fn bounded_transcript_compacts_older_into_summary() {
    let mut items: Vec<TranscriptItem> = Vec::new();
    items.push(TranscriptItem::user(
        "original intent: refactor permissions broker".to_string(),
    ));
    for index in 0..30 {
        items.push(TranscriptItem::assistant(format!(
            "assistant turn {index} did some intermediate work"
        )));
    }
    items.push(TranscriptItem::user("latest follow-up".to_string()));
    let snapshot = AiReviewerTranscriptSnapshot {
        entry_count: items.len(),
        history_version: 0,
        items,
    };
    let rendered =
        bounded_transcript(&snapshot, None, DEFAULT_AI_REVIEWER_MAX_TRANSCRIPT_TOKENS);
    assert!(rendered.contains("summary of"));
    assert!(rendered.contains("earlier turn(s)"));
    assert!(rendered.contains("latest follow-up"));
    // The earliest assistant turn should be elided into the summary line.
    assert!(!rendered.contains("assistant turn 0 "));
}

#[test]
fn bounded_transcript_respects_small_budget() {
    let items = (0..40)
        .map(|index| TranscriptItem::user(format!("user message {index}")))
        .collect::<Vec<_>>();
    let snapshot = AiReviewerTranscriptSnapshot {
        entry_count: items.len(),
        history_version: 0,
        items,
    };
    let rendered = bounded_transcript(&snapshot, None, 512);
    // The most recent user message must always be present.
    assert!(rendered.contains("user message 39"));
    // The very first message should be folded into the summary, not printed.
    assert!(!rendered.contains("39:user: user message 0"));
}

#[test]
fn parse_reviewer_json_inside_text() {
    let decision =
        parse_reviewer_response("```json\n{\"action\":\"deny\",\"reason\":\"too broad\"}\n```")
            .expect("decision");
    assert_eq!(decision.action, PermissionAction::Deny);
    assert_eq!(decision.reason, "too broad");
}

#[test]
fn circuit_trips_after_consecutive_denials() {
    let mut state = AiReviewerState::default();
    assert!(state.record_denial(TurnId::new(7)).is_none());
    let reason = state.record_denial(TurnId::new(7)).expect("tripped");
    assert!(reason.contains("consecutively"));
    assert!(state.bypass_reason(TurnId::new(7)).is_some());
}

#[test]
fn transcript_delta_marker_mentions_prior_entries() {
    let mut state = AiReviewerState::default();
    let first = AiReviewerTranscriptSnapshot {
        items: vec![TranscriptItem::user("one")],
        history_version: 2,
        entry_count: 1,
    };
    let second = AiReviewerTranscriptSnapshot {
        items: vec![
            TranscriptItem::user("one"),
            TranscriptItem::assistant("two"),
        ],
        history_version: 2,
        entry_count: 2,
    };
    assert!(state.transcript_delta_marker(&first).is_none());
    assert_eq!(
        state.transcript_delta_marker(&second),
        Some("[1 earlier entries reviewed previously and unchanged]".to_string())
    );
}

#[test]
fn prompt_contains_policy_and_request() {
    let config = AppConfig::default();
    let mut state = AiReviewerState::default();
    let request = PermissionRequest {
        call_id: "call".to_string(),
        tool_name: "read_file".to_string(),
        capability: PermissionCapability::Read,
        target: "path:README.md".to_string(),
        risk: PermissionRisk::Low,
        summary: "read README".to_string(),
        metadata: BTreeMap::new(),
        suggested_rules: Vec::new(),
    };
    let prompt = build_review_prompt(&config, &request, None, "test policy", &mut state);
    assert!(prompt.contains("test policy"));
    assert!(prompt.contains("\"capability\":\"read\""));
    assert!(prompt.contains("\"target\":\"path:README.md\""));
}
