//! Streaming controller for assistant text deltas.
//!
//! Splits the in-flight reply into:
//!   * `committed` — bytes that have crossed a `\n` boundary, so they
//!     are safe to send through the markdown/highlighter pipeline.
//!   * `tail`      — bytes since the last `\n`. Painted as plain text
//!     so a half-streamed fence (`` ```ru ``…) doesn't flash with the
//!     wrong style until the closing newline arrives.
//!
//! The controller exposes the read API needed by the existing render
//! path (`text()`, `is_empty()`, `trim_is_empty()`) so it can drop in
//! where the previous `String` lived.

use std::fmt;
use std::hash::Hasher;

#[derive(Debug, Default, Clone)]
pub(crate) struct StreamingController {
    committed: String,
    /// Lines that are inside an unclosed fence — buffered so a fenced
    /// code block doesn't render with the wrong style until its closing
    /// fence arrives.
    held: String,
    /// Bytes since the last `\n` (incomplete current line).
    pending: String,
    in_fence: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamingMutation {
    /// Tail grew; tail-only repaint suffices.
    TailGrew,
    /// One or more committed lines flushed; committed region needs a relayout.
    CommittedGrew,
    /// Nothing changed (e.g. empty delta).
    NoOp,
}

impl StreamingController {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.committed.is_empty() && self.held.is_empty() && self.pending.is_empty()
    }

    pub(crate) fn trim_is_empty(&self) -> bool {
        self.committed.trim().is_empty()
            && self.held.trim().is_empty()
            && self.pending.trim().is_empty()
    }

    /// Render-equivalent full text (committed + held + pending).
    pub(crate) fn text(&self) -> String {
        let mut out =
            String::with_capacity(self.committed.len() + self.held.len() + self.pending.len());
        self.write_to(&mut out)
            .expect("writing to String cannot fail");
        out
    }

    pub(crate) fn segments(&self) -> impl Iterator<Item = &str> {
        [
            self.committed.as_str(),
            self.held.as_str(),
            self.pending.as_str(),
        ]
        .into_iter()
        .filter(|segment| !segment.is_empty())
    }

    pub(crate) fn write_to(&self, f: &mut impl fmt::Write) -> fmt::Result {
        for segment in self.segments() {
            f.write_str(segment)?;
        }
        Ok(())
    }

    pub(crate) fn hash_text<H: Hasher>(&self, state: &mut H) {
        for segment in self.segments() {
            state.write(segment.as_bytes());
        }
        state.write_u8(0xff);
    }

    #[allow(dead_code)]
    pub(crate) fn committed(&self) -> &str {
        &self.committed
    }

    /// Returns the live tail region (lines held inside an open fence
    /// plus the in-progress current line). What the renderer paints
    /// "below" the committed/cached region.
    #[allow(dead_code)]
    pub(crate) fn tail(&self) -> String {
        if self.held.is_empty() {
            self.pending.clone()
        } else {
            let mut out = String::with_capacity(self.held.len() + self.pending.len());
            out.push_str(&self.held);
            out.push_str(&self.pending);
            out
        }
    }

    pub(crate) fn clear(&mut self) {
        self.committed.clear();
        self.held.clear();
        self.pending.clear();
        self.in_fence = false;
    }

    /// Append a delta, promoting newline-terminated runs into `committed`
    /// when they sit outside an open fence. Lines inside an unclosed
    /// fence are held until the closing fence arrives.
    pub(crate) fn push_delta(&mut self, delta: &str) -> StreamingMutation {
        if delta.is_empty() {
            return StreamingMutation::NoOp;
        }
        let mut mutation = StreamingMutation::TailGrew;
        for ch in delta.chars() {
            self.pending.push(ch);
            if ch != '\n' {
                continue;
            }
            let line = std::mem::take(&mut self.pending);
            let was_in_fence = self.in_fence;
            let toggled = Self::line_is_fence(&line);
            if toggled {
                self.in_fence = !self.in_fence;
            }
            if was_in_fence {
                // We were inside a fence at the start of this line.
                self.held.push_str(&line);
                if !self.in_fence {
                    // Closing fence: flush held block now.
                    let flushed = std::mem::take(&mut self.held);
                    self.committed.push_str(&flushed);
                    mutation = StreamingMutation::CommittedGrew;
                }
            } else if self.in_fence {
                // Opening fence — buffer the opening line.
                self.held.push_str(&line);
            } else {
                self.committed.push_str(&line);
                mutation = StreamingMutation::CommittedGrew;
            }
        }
        mutation
    }

    /// Drain everything into `committed` and return the final text.
    /// Used on `AssistantCompleted` to flush whatever's in flight.
    #[allow(dead_code)]
    pub(crate) fn finalize(&mut self) -> String {
        if !self.held.is_empty() {
            let held = std::mem::take(&mut self.held);
            self.committed.push_str(&held);
        }
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            self.committed.push_str(&pending);
        }
        self.in_fence = false;
        std::mem::take(&mut self.committed)
    }

    fn line_is_fence(line: &str) -> bool {
        // Delegates to the crate-private free function so the copy substrate
        // (`crate::copy`) can reuse the exact same CommonMark §4.5 fence test
        // without depending on the streaming controller's private method.
        line_is_fence(line)
    }
}

/// Whether `line` is a CommonMark §4.5 code-fence line: its trimmed body
/// starts with three or more backticks (```` ``` ````) or three or more
/// tildes (`~~~`). The opening fence may carry an info string (a language
/// tag) after the run; only the leading prefix decides.
///
/// Extracted from [`StreamingController::line_is_fence`] (which now delegates
/// here) so the semantic-copy code-block resolver in `crate::copy` can detect
/// fenced blocks with the identical rule the streamer uses. Kept loose on
/// purpose: we only key off the opening prefix, not the info string.
pub(crate) fn line_is_fence(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

impl fmt::Display for StreamingController {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_to(f)
    }
}

impl std::hash::Hash for StreamingController {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.hash_text(state);
    }
}

#[cfg(test)]
#[path = "streaming_tests.rs"]
mod tests;
