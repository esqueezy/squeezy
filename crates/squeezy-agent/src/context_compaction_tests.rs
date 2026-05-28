use serde_json::json;
use squeezy_core::{AppConfig, ContextCompactionState};
use squeezy_llm::LlmInputItem;

use super::{build_compaction_summary, strip_media_for_compaction};

fn function_call(call_id: &str, name: &str, arguments: serde_json::Value) -> LlmInputItem {
    LlmInputItem::FunctionCall {
        call_id: call_id.to_string(),
        name: name.to_string(),
        arguments,
    }
}

/// A 220-byte base64 blob built from a repeating pattern. Long enough to
/// exceed `STRIP_MEDIA_MIN_LEN` (100) and to survive `compact_text`'s
/// 260-char tool-output cap, so a leaked URI would land in the summary
/// without this guard.
fn long_base64_payload() -> String {
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/".repeat(4)
}

fn function_call_output(call_id: &str, output: &str) -> LlmInputItem {
    LlmInputItem::FunctionCallOutput {
        call_id: call_id.to_string(),
        output: output.to_string(),
    }
}

#[test]
fn strip_image_data_uri_from_function_call_output() {
    let payload = long_base64_payload();
    let body = format!("Screenshot saved. data:image/png;base64,{payload} (end of output)");
    let items = vec![function_call_output("call-1", &body)];

    let stripped = strip_media_for_compaction(&items);
    let LlmInputItem::FunctionCallOutput { output, .. } = &stripped[0] else {
        panic!("expected FunctionCallOutput");
    };

    assert!(
        output.contains("[image]"),
        "placeholder missing; got {output:?}"
    );
    assert!(
        !output.contains("data:image/png;base64,"),
        "data URI prefix leaked through; got {output:?}"
    );
    assert!(
        !output.contains(payload.as_str()),
        "base64 payload leaked through; got {output:?}"
    );
    assert!(
        output.starts_with("Screenshot saved."),
        "leading prose dropped; got {output:?}"
    );
    assert!(
        output.ends_with("(end of output)"),
        "trailing prose dropped; got {output:?}"
    );
}

#[test]
fn strip_document_data_uri_uses_document_placeholder() {
    let payload = long_base64_payload();
    let body = format!("report attached: data:application/pdf;base64,{payload}");
    let items = vec![function_call_output("call-1", &body)];

    let stripped = strip_media_for_compaction(&items);
    let LlmInputItem::FunctionCallOutput { output, .. } = &stripped[0] else {
        panic!("expected FunctionCallOutput");
    };

    assert!(
        output.contains("[document]"),
        "document placeholder missing; got {output:?}"
    );
    assert!(
        !output.contains("base64,"),
        "data URI marker leaked; got {output:?}"
    );
}

#[test]
fn strip_handles_multiple_uris_in_one_output() {
    let payload = long_base64_payload();
    let body = format!(
        "first data:image/jpeg;base64,{payload} between data:image/webp;base64,{payload} tail"
    );
    let items = vec![function_call_output("call-1", &body)];

    let stripped = strip_media_for_compaction(&items);
    let LlmInputItem::FunctionCallOutput { output, .. } = &stripped[0] else {
        panic!("expected FunctionCallOutput");
    };

    assert_eq!(
        output.matches("[image]").count(),
        2,
        "expected two placeholders; got {output:?}"
    );
    assert!(output.starts_with("first "));
    assert!(output.contains(" between "));
    assert!(output.ends_with(" tail"));
}

#[test]
fn strip_media_does_not_touch_in_memory_state() {
    let payload = long_base64_payload();
    let body = format!("data:image/png;base64,{payload}");
    let original = vec![
        LlmInputItem::UserText("hello".to_string()),
        function_call_output("call-1", &body),
    ];
    let snapshot = original.clone();

    let _ = strip_media_for_compaction(&original);

    assert_eq!(original, snapshot, "input slice was mutated");
}

#[test]
fn strip_leaves_non_function_call_output_items_unchanged() {
    let payload = long_base64_payload();
    let body = format!("data:image/png;base64,{payload}");
    // A UserText with a data URI is left alone: the recommendation
    // targets FunctionCallOutput because that is the realistic ingress
    // path for tool-produced screenshots/PDFs. User prose with an inline
    // data URI is a knowing decision by the user.
    let items = vec![LlmInputItem::UserText(body.clone())];
    let stripped = strip_media_for_compaction(&items);
    assert_eq!(stripped, items);
}

#[test]
fn strip_skips_short_outputs() {
    // Anything under STRIP_MEDIA_MIN_LEN (100) is cloned through unchanged
    // so plain short tool outputs don't pay the scan cost.
    let body = "short output, no media";
    let items = vec![function_call_output("call-1", body)];
    let stripped = strip_media_for_compaction(&items);
    let LlmInputItem::FunctionCallOutput { output, .. } = &stripped[0] else {
        panic!("expected FunctionCallOutput");
    };
    assert_eq!(output, body);
}

#[test]
fn strip_preserves_unicode_neighbours() {
    let payload = long_base64_payload();
    // Multi-byte UTF-8 on both sides of the data URI. Byte-index handling
    // would corrupt these scalars if the strip scanner ever sliced inside
    // a code point.
    let body = format!("héllo data:image/png;base64,{payload} 世界");
    let items = vec![function_call_output("call-1", &body)];
    let stripped = strip_media_for_compaction(&items);
    let LlmInputItem::FunctionCallOutput { output, .. } = &stripped[0] else {
        panic!("expected FunctionCallOutput");
    };
    assert!(output.contains("héllo "));
    assert!(output.contains(" 世界"));
    assert!(output.contains("[image]"));
}

#[test]
fn compaction_summary_does_not_carry_base64_image_payload() {
    // build_compaction_summary is invoked on the stripped older slice in
    // compact_conversation (see context_compaction.rs:148-167). If the
    // tool output contained a base64 PNG, the model-assisted summarizer
    // would otherwise receive it via `extractive_summary`. Verify the
    // built summary does not contain the raw base64 string.
    let payload = long_base64_payload();
    let body = format!("screenshot ready. data:image/png;base64,{payload} ok.");
    let older = vec![
        LlmInputItem::UserText("write a screenshot".to_string()),
        function_call_output("call-1", &body),
    ];
    let older_for_summary = strip_media_for_compaction(&older);

    let state = ContextCompactionState::default();
    let config = AppConfig::default();
    let summary = build_compaction_summary(1, &state, &older_for_summary, &[], None, &config);

    assert!(
        !summary.contains(payload.as_str()),
        "base64 payload reached the compaction summary"
    );
    assert!(
        !summary.contains("data:image/png;base64,"),
        "data URI prefix reached the compaction summary"
    );
}

fn lineage_block<'a>(summary: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>\n");
    let close = format!("\n</{tag}>");
    let start = summary.find(&open)? + open.len();
    let end_rel = summary[start..].find(&close)?;
    Some(&summary[start..start + end_rel])
}

#[test]
fn compaction_summary_emits_read_files_block() {
    // Two read_file calls land in <read-files>; the closing line of the
    // base summary stays put so the blocks really are an *append*.
    let older = vec![
        function_call(
            "call-1",
            "read_file",
            json!({"path": "crates/squeezy-tui/src/render/cache.rs"}),
        ),
        function_call(
            "call-2",
            "read_file",
            json!({"path": "crates/squeezy-llm/src/anthropic.rs"}),
        ),
    ];
    let state = ContextCompactionState::default();
    let config = AppConfig::default();

    let summary = build_compaction_summary(1, &state, &older, &[], None, &config);

    let body = lineage_block(&summary, "read-files").expect("<read-files> block missing");
    assert_eq!(
        body, "crates/squeezy-llm/src/anthropic.rs\ncrates/squeezy-tui/src/render/cache.rs",
        "read-files block content mismatch (alphabetic, deduped)"
    );
    assert!(
        !summary.contains("<modified-files>"),
        "modified block should not appear when no edits occurred"
    );
    assert!(
        summary.contains("Compacted 2 older model-visible item(s)"),
        "base summary tail must remain before the lineage blocks"
    );
}

#[test]
fn compaction_summary_emits_modified_files_block_for_write_apply_and_notebook() {
    // write_file, notebook_edit, and apply_patch all feed <modified-files>.
    // apply_patch is special: both legacy patches[] and modern operations[]
    // (including MoveFile's from/to) must populate the set.
    let older = vec![
        function_call(
            "call-1",
            "write_file",
            json!({"path": "crates/squeezy-tools/src/patch.rs", "content": "// ..."}),
        ),
        function_call(
            "call-2",
            "notebook_edit",
            json!({"path": "notebooks/explore.ipynb"}),
        ),
        function_call(
            "call-3",
            "apply_patch",
            json!({
                "patches": [
                    {"path": "crates/squeezy-agent/src/lib.rs", "search": "a", "replace": "b"}
                ],
                "operations": [
                    {"type": "move_file", "from": "old/file.rs", "to": "new/file.rs"},
                    {"type": "create_file", "path": "fresh/file.rs", "contents": ""}
                ]
            }),
        ),
    ];
    let state = ContextCompactionState::default();
    let config = AppConfig::default();

    let summary = build_compaction_summary(1, &state, &older, &[], None, &config);

    let body = lineage_block(&summary, "modified-files").expect("<modified-files> block missing");
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(
        lines,
        vec![
            "crates/squeezy-agent/src/lib.rs",
            "crates/squeezy-tools/src/patch.rs",
            "fresh/file.rs",
            "new/file.rs",
            "notebooks/explore.ipynb",
            "old/file.rs",
        ],
        "modified-files block must include every write/apply_patch/notebook_edit path",
    );
    assert!(
        !summary.contains("<read-files>"),
        "read block should not appear when no reads occurred"
    );
}

#[test]
fn compaction_summary_modified_files_supersedes_read_files() {
    // Pi rule (computeFileLists): a file that is both read and modified
    // is reported only under <modified-files>.
    let older = vec![
        function_call("call-1", "read_file", json!({"path": "src/a.rs"})),
        function_call("call-2", "read_file", json!({"path": "src/b.rs"})),
        function_call(
            "call-3",
            "write_file",
            json!({"path": "src/a.rs", "content": "// ..."}),
        ),
    ];
    let state = ContextCompactionState::default();
    let config = AppConfig::default();

    let summary = build_compaction_summary(1, &state, &older, &[], None, &config);

    let read_body = lineage_block(&summary, "read-files").expect("<read-files> block missing");
    let modified_body =
        lineage_block(&summary, "modified-files").expect("<modified-files> block missing");
    assert_eq!(
        read_body, "src/b.rs",
        "src/a.rs should be promoted to modified-only",
    );
    assert_eq!(modified_body, "src/a.rs");
}

#[test]
fn compaction_summary_omits_lineage_blocks_when_no_file_ops() {
    // Search-class tools (grep) target a starting directory, not a file,
    // so they are intentionally excluded from the lineage map.
    let older = vec![
        LlmInputItem::UserText("hello".to_string()),
        function_call(
            "call-1",
            "grep",
            json!({"pattern": "todo", "path": "crates"}),
        ),
    ];
    let state = ContextCompactionState::default();
    let config = AppConfig::default();

    let summary = build_compaction_summary(1, &state, &older, &[], None, &config);

    assert!(
        !summary.contains("<read-files>"),
        "no file-class tools were invoked; <read-files> must be absent"
    );
    assert!(
        !summary.contains("<modified-files>"),
        "no file-class tools were invoked; <modified-files> must be absent"
    );
}

#[test]
fn compaction_summary_carries_lineage_across_generations() {
    // The prior summary already lists paths; the current `older` slice
    // adds new ones and promotes one read into modified. The output
    // must reflect the union, with modified-wins semantics and dedup.
    let previous = "Some prose.\n\
        <read-files>\n\
        prior/read-only.rs\n\
        prior/shared.rs\n\
        </read-files>\n\
        <modified-files>\n\
        prior/changed.rs\n\
        </modified-files>";
    let state = ContextCompactionState {
        summary: Some(previous.to_string()),
        ..ContextCompactionState::default()
    };

    let older = vec![
        function_call("call-1", "read_file", json!({"path": "current/look.rs"})),
        function_call(
            "call-2",
            "write_file",
            json!({"path": "prior/shared.rs", "content": "// ..."}),
        ),
    ];
    let config = AppConfig::default();

    let summary = build_compaction_summary(2, &state, &older, &[], None, &config);

    let read_body = lineage_block(&summary, "read-files").expect("<read-files> block missing");
    let modified_body =
        lineage_block(&summary, "modified-files").expect("<modified-files> block missing");
    assert_eq!(
        read_body, "current/look.rs\nprior/read-only.rs",
        "prior/shared.rs must be promoted out of read; prior/read-only.rs survives",
    );
    assert_eq!(
        modified_body, "prior/changed.rs\nprior/shared.rs",
        "modified set must accumulate across generations",
    );
}

#[test]
fn compaction_summary_caps_lineage_at_limit_keeping_newest() {
    // Build 60 read calls. The cap should fire and keep the 50 most
    // recent paths (i.e., drop the chronologically oldest 10). Sorted
    // output then makes the kept set easy to assert as `file_010..file_059`.
    let older: Vec<LlmInputItem> = (0..60)
        .map(|i| {
            function_call(
                &format!("call-{i}"),
                "read_file",
                json!({"path": format!("crates/a/file_{i:03}.rs")}),
            )
        })
        .collect();
    let state = ContextCompactionState::default();
    let config = AppConfig::default();

    let summary = build_compaction_summary(1, &state, &older, &[], None, &config);

    let body = lineage_block(&summary, "read-files").expect("<read-files> block missing");
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(
        lines.len(),
        50,
        "lineage list must be capped at 50 entries; got {}",
        lines.len()
    );
    assert_eq!(
        lines.first(),
        Some(&"crates/a/file_010.rs"),
        "oldest-dropped: file_000..file_009 should have been evicted before sort",
    );
    assert_eq!(
        lines.last(),
        Some(&"crates/a/file_059.rs"),
        "newest entry must survive the cap",
    );
}

#[test]
fn compaction_summary_dedups_repeated_file_touches() {
    // The same read_file call repeated 5 times still produces a single
    // entry in <read-files>.
    let older: Vec<LlmInputItem> = (0..5)
        .map(|i| {
            function_call(
                &format!("call-{i}"),
                "read_file",
                json!({"path": "crates/squeezy-core/src/lib.rs"}),
            )
        })
        .collect();
    let state = ContextCompactionState::default();
    let config = AppConfig::default();

    let summary = build_compaction_summary(1, &state, &older, &[], None, &config);

    let body = lineage_block(&summary, "read-files").expect("<read-files> block missing");
    assert_eq!(body, "crates/squeezy-core/src/lib.rs");
}
