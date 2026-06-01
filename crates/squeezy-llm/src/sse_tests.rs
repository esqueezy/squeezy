use super::SseDecoder;

#[test]
fn splits_single_event() {
    let mut decoder = SseDecoder::default();
    let events = decoder.push(b"data: hello\n\n");
    assert_eq!(events, vec!["hello".to_string()]);
    assert!(decoder.finish().is_empty());
}

#[test]
fn splits_event_across_pushes() {
    let mut decoder = SseDecoder::default();
    assert!(decoder.push(b"data: hel").is_empty());
    assert!(decoder.push(b"lo").is_empty());
    let events = decoder.push(b"\n\n");
    assert_eq!(events, vec!["hello".to_string()]);
}

#[test]
fn joins_multiple_data_lines() {
    let mut decoder = SseDecoder::default();
    let events = decoder.push(b"data: line one\ndata: line two\n\n");
    assert_eq!(events, vec!["line one\nline two".to_string()]);
}

#[test]
fn ignores_comment_and_blank_lines() {
    let mut decoder = SseDecoder::default();
    let events = decoder.push(b": heartbeat\nevent: ping\ndata: payload\n\n");
    assert_eq!(events, vec!["payload".to_string()]);
}

#[test]
fn supports_crlf_boundaries() {
    let mut decoder = SseDecoder::default();
    let events = decoder.push(b"data: alpha\r\n\r\ndata: beta\r\n\r\n");
    assert_eq!(events, vec!["alpha".to_string(), "beta".to_string()]);
}

#[test]
fn returns_multiple_events_from_single_push() {
    let mut decoder = SseDecoder::default();
    let events = decoder.push(b"data: one\n\ndata: two\n\ndata: three\n\n");
    assert_eq!(
        events,
        vec!["one".to_string(), "two".to_string(), "three".to_string()],
    );
}

#[test]
fn finish_flushes_trailing_event_without_terminator() {
    let mut decoder = SseDecoder::default();
    assert!(decoder.push(b"data: dangling").is_empty());
    let events = decoder.finish();
    assert_eq!(events, vec!["dangling".to_string()]);
}

#[test]
fn finish_drops_buffer_with_no_data_lines() {
    let mut decoder = SseDecoder::default();
    assert!(decoder.push(b": just-a-comment").is_empty());
    assert!(decoder.finish().is_empty());
}

#[test]
fn decode_drops_empty_data_lines() {
    // X-02: WHATWG EventSource §9.2 allows empty `data:` heartbeats.
    // OpenAI emits them on long reasoning turns; forwarding `""` to
    // `serde_json::from_str` would crash the stream with EOF.
    let mut decoder = SseDecoder::default();
    let events = decoder.push(b"data:\n\n");
    assert!(
        events.is_empty(),
        "empty `data:` heartbeat must not surface as an event"
    );
}

#[test]
fn decode_drops_whitespace_only_data_lines() {
    // X-02: `data:   \n\n` (only whitespace) is still a heartbeat.
    let mut decoder = SseDecoder::default();
    let events = decoder.push(b"data:    \n\n");
    assert!(events.is_empty());
}

#[test]
fn decode_keeps_payload_when_only_some_data_lines_empty() {
    // X-02: a multi-`data:` event with one empty line should still yield
    // the non-empty payload (not be dropped as fully empty).
    let mut decoder = SseDecoder::default();
    let events = decoder.push(b"data: real\ndata:\n\n");
    assert_eq!(events, vec!["real".to_string()]);
}

#[test]
fn decode_trims_whitespace_around_done_sentinel() {
    // X-02: providers like Together / vLLM occasionally emit
    // `data: [DONE] \n\n` (trailing space). Downstream `[DONE]` literal
    // comparisons must match after trim.
    let mut decoder = SseDecoder::default();
    let events = decoder.push(b"data: [DONE] \n\n");
    assert_eq!(events, vec!["[DONE]".to_string()]);
}

#[test]
fn find_event_boundary_is_linear_across_pushes() {
    // L2: many small pushes without a terminator must not re-scan the
    // whole buffer each time. Wall-clock test: 50k 64-byte chunks of a
    // single un-terminated `data:` line, then close. The O(n^2) version
    // exhibited multi-second runtimes here; the linear version should
    // complete in well under a second on any reasonable machine.
    let chunk = vec![b'x'; 64];
    let mut decoder = SseDecoder::default();
    decoder.push(b"data: ");
    let start = std::time::Instant::now();
    for _ in 0..50_000 {
        assert!(decoder.push(&chunk).is_empty());
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "boundary scan should be linear; took {elapsed:?} for 50k pushes",
    );
    // Sanity: terminating the event still yields exactly one payload.
    let events = decoder.push(b"\n\n");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].len(), 50_000 * 64);
}

#[test]
fn crlf_boundary_split_across_pushes() {
    // L2: scan-position overlap must preserve the ability to detect a
    // `\r\n\r\n` boundary that straddles two pushes.
    let mut decoder = SseDecoder::default();
    assert!(decoder.push(b"data: hi\r\n\r").is_empty());
    let events = decoder.push(b"\n");
    assert_eq!(events, vec!["hi".to_string()]);
}

#[test]
fn lf_boundary_split_across_pushes() {
    // L2: same as above for the simpler `\n\n` boundary.
    let mut decoder = SseDecoder::default();
    assert!(decoder.push(b"data: hi\n").is_empty());
    let events = decoder.push(b"\n");
    assert_eq!(events, vec!["hi".to_string()]);
}
