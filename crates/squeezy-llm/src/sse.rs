//! Server-Sent Events decoder shared by every provider client that streams
//! over SSE (OpenAI Responses, OpenAI-compatible Chat Completions, Google
//! Gemini, Anthropic Messages). Each provider parses the `data:` payload
//! itself; this module only frames the byte stream into individual events.

#[derive(Debug, Default)]
pub(crate) struct SseDecoder {
    buffer: Vec<u8>,
}

impl SseDecoder {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        let mut consumed = 0usize;

        while let Some((index, len)) = find_event_boundary(&self.buffer[consumed..]) {
            let event_end = consumed + index;
            if let Some(data) = decode_sse_event(&self.buffer[consumed..event_end]) {
                events.push(data);
            }
            consumed = event_end + len;
        }

        if consumed != 0 {
            let remaining = self.buffer.len() - consumed;
            self.buffer.copy_within(consumed.., 0);
            self.buffer.truncate(remaining);
        }

        events
    }

    pub(crate) fn finish(&mut self) -> Vec<String> {
        if self.buffer.is_empty() {
            return Vec::new();
        }

        let event = std::mem::take(&mut self.buffer);
        decode_sse_event(&event).into_iter().collect()
    }
}

fn find_event_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    for index in 0..bytes.len() {
        if bytes[index] == b'\n' && bytes.get(index + 1) == Some(&b'\n') {
            return Some((index, 2));
        }
        if bytes[index] == b'\r'
            && bytes.get(index + 1) == Some(&b'\n')
            && bytes.get(index + 2) == Some(&b'\r')
            && bytes.get(index + 3) == Some(&b'\n')
        {
            return Some((index, 4));
        }
    }
    None
}

fn decode_sse_event(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut data = String::new();
    let mut saw_data = false;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(line_data) = line.strip_prefix("data:") {
            if saw_data {
                data.push('\n');
            }
            data.push_str(line_data.trim_start());
            saw_data = true;
        }
    }
    saw_data.then_some(data)
}

#[cfg(test)]
#[path = "sse_tests.rs"]
mod tests;
