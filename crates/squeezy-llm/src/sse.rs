//! Server-Sent Events decoder shared by every provider client that streams
//! over SSE (OpenAI Responses, OpenAI-compatible Chat Completions, Google
//! Gemini, Anthropic Messages). Each provider parses the `data:` payload
//! itself; this module only frames the byte stream into individual events.

#[derive(Debug, Default)]
pub(crate) struct SseDecoder {
    buffer: Vec<u8>,
    /// Byte offset into `buffer` where the next boundary scan should
    /// resume. Without this, every push re-scans the entire buffer with
    /// `.windows(2)` — O(n²) on multi-MB reasoning streams where a
    /// single event can span many push calls before the `\n\n`
    /// terminator arrives.
    scan_pos: usize,
}

impl SseDecoder {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();

        loop {
            match find_event_boundary(&self.buffer, self.scan_pos) {
                Some((index, len)) => {
                    let event = self.buffer.drain(..index + len).collect::<Vec<_>>();
                    // Drained the entire prefix the scanner had walked
                    // (the boundary itself is part of that prefix), so
                    // resume from byte 0 of the now-shorter buffer.
                    self.scan_pos = 0;
                    if let Some(data) = decode_sse_event(&event) {
                        events.push(data);
                    }
                }
                None => {
                    // No boundary yet. Park the cursor near the tail so
                    // the next push only scans newly-appended bytes.
                    // Keep a 3-byte overlap so a `\r\n\r\n` boundary
                    // straddling the push gap is still caught.
                    self.scan_pos = self.buffer.len().saturating_sub(3);
                    break;
                }
            }
        }

        events
    }

    pub(crate) fn finish(&mut self) -> Vec<String> {
        self.scan_pos = 0;
        if self.buffer.is_empty() {
            return Vec::new();
        }

        let event = std::mem::take(&mut self.buffer);
        decode_sse_event(&event).into_iter().collect()
    }
}

fn find_event_boundary(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    let start = start.min(bytes.len());
    let tail = &bytes[start..];
    let lf = tail
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (start + index, 2));
    let crlf = tail
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (start + index, 4));

    [lf, crlf].into_iter().flatten().min_by_key(|b| b.0)
}

fn decode_sse_event(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut data_lines = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(data) = line.strip_prefix("data:") {
            // SSE spec (WHATWG EventSource §9.2) allows empty `data:`
            // lines as keep-alive padding. Some providers (notably OpenAI
            // on long reasoning turns) emit them between real chunks;
            // forwarding `""` to `serde_json::from_str` crashes the
            // stream. Drop empties; also tolerate trailing whitespace
            // around the `[DONE]` sentinel (some providers send
            // `data: [DONE] \n`).
            let payload = data.trim();
            if payload.is_empty() {
                continue;
            }
            data_lines.push(payload);
        }
    }
    if data_lines.is_empty() {
        None
    } else {
        Some(data_lines.join("\n"))
    }
}

#[cfg(test)]
#[path = "sse_tests.rs"]
mod tests;
