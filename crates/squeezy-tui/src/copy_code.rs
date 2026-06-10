//! Code-Aware Copy/Export (§12.5.5).
//!
//! The plain semantic-copy substrate ([`crate::copy`]) treats fenced code as
//! ordinary text: [`crate::copy::CopyScope::CodeBlockUnderCursor`] yields the
//! interior of the ONE block under the cursor and *drops* its language fence.
//! That is the right shape for "give me this snippet", but it cannot answer
//! "give me every code block from this answer, with the languages preserved, and
//! nothing else" — the request that matters when an assistant message interleaves
//! prose and several fenced blocks.
//!
//! This module owns that code-aware extraction. Given the resolved
//! [`TranscriptRow`] slice of a scope (already gutter-bearing, exactly as the
//! renderer painted it), [`extract_code_blocks`] walks the rows with the same
//! CommonMark fence rule the streamer and the plain copy path use
//! ([`crate::streaming::line_is_fence`]), and projects out a sequence of
//! [`CodeBlock`]s: each block's *language* (the opening fence's info string) plus
//! its *interior* lines, with the rail gutter stripped from every line so the
//! payload is clean code, never box-drawing.
//!
//! [`render_code_payload`] then serializes those blocks back into a clipboard- or
//! file-ready string in a chosen [`CodePayloadStyle`]:
//!
//! * [`CodePayloadStyle::Fenced`] re-emits a *clean* ```` ```lang ```` fence
//!   around each block (rails gone, language preserved) — the Markdown-faithful
//!   form that round-trips back into a doc.
//! * [`CodePayloadStyle::Bare`] concatenates just the code interiors (blocks
//!   separated by a blank line) — the paste-into-an-editor form, no fences.
//!
//! Both styles preserve the source line endings of the code interior verbatim
//! (no normalization), honoring the spec's "preserve source line endings by
//! default for code/diff export" platform note: this layer only ever *joins* and
//! *strips the gutter*, it never rewrites a line's body.
//!
//! `lib.rs` owns the side effects (clipboard write, status/toast); it resolves a
//! scope to a row range via [`crate::copy::resolve_scope`], then calls
//! [`extract_code_blocks`] + [`render_code_payload`] instead of the plain
//! [`crate::copy::format_rows`].

use crate::streaming::line_is_fence;
use crate::transcript_surface::{TranscriptRow, strip_gutter};

/// One fenced code block lifted from the transcript: its language (the opening
/// fence's info string, empty when the fence carried none) and the
/// gutter-stripped interior lines (the fence lines themselves excluded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeBlock {
    /// The opening fence's info string, lowercased and trimmed (e.g. `"rust"`).
    /// Empty when the fence had no language tag.
    pub(crate) language: String,
    /// The block's interior, one gutter-stripped line per element. May be empty
    /// for a fence pair with no body (``` immediately followed by ```), which
    /// [`extract_code_blocks`] still records so a deliberately-empty block is not
    /// silently dropped.
    pub(crate) lines: Vec<String>,
}

impl CodeBlock {
    /// The interior re-joined with `\n`, source line endings preserved (we only
    /// rejoin the rows the wrapper split; we never normalize a line body).
    fn body(&self) -> String {
        self.lines.join("\n")
    }
}

/// How [`render_code_payload`] serializes the extracted blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodePayloadStyle {
    /// Re-emit a clean ```` ```lang ```` fence around each block. Markdown-
    /// faithful: the language is preserved on the opening fence and the result
    /// round-trips back into a document.
    Fenced,
    /// Just the code interiors, no fences, blocks separated by one blank line.
    /// The paste-into-an-editor form.
    Bare,
}

/// Extract every fenced code block from `rows` in document order.
///
/// Walks the rows toggling an in-fence flag on each fence line (detected on the
/// *gutter-stripped* text so a rail-prefixed fence still registers). The opening
/// fence's info string becomes the block's [`CodeBlock::language`]; the lines
/// between the open and close fence become its interior. A dangling unclosed
/// fence at the end of `rows` is NOT emitted — only complete `[open, close]`
/// pairs are, mirroring [`crate::copy::resolve_scope`]'s code-block rule (an
/// in-progress stream never yields a half block).
pub(crate) fn extract_code_blocks(rows: &[TranscriptRow]) -> Vec<CodeBlock> {
    let mut blocks = Vec::new();
    let mut open: Option<CodeBlock> = None;

    for row in rows {
        let text = strip_gutter(&row.copy_text);
        if line_is_fence(text) {
            match open.take() {
                // No block open: this fence opens one. Capture its language.
                None => {
                    open = Some(CodeBlock {
                        language: fence_language(text),
                        lines: Vec::new(),
                    });
                }
                // A block is open: this fence closes it. Commit the block.
                Some(block) => blocks.push(block),
            }
        } else if let Some(block) = open.as_mut() {
            // Interior line of the currently-open block.
            block.lines.push(text.to_string());
        }
        // Else: prose outside any fence — ignored, this is a code-only extract.
    }

    // A trailing `open` is a dangling unclosed fence (still-streaming code); drop
    // it so the extract only ever contains complete blocks.
    blocks
}

/// The language tag (info string) of a fence line: the run of non-whitespace
/// characters after the leading backtick/tilde run, lowercased and trimmed.
/// Returns an empty string for a bare fence (`"```"`). Only the FIRST token of
/// the info string is taken (CommonMark allows trailing words; the language is
/// the first word).
fn fence_language(line: &str) -> String {
    let trimmed = line.trim_start();
    // Skip the leading run of the fence character (``` or ~~~ and any longer run).
    let fence_char = trimmed.chars().next();
    let info = match fence_char {
        Some(c @ ('`' | '~')) => trimmed.trim_start_matches(c),
        _ => return String::new(),
    };
    info.split_whitespace()
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// Serialize `blocks` into a clipboard/file-ready string in `style`. Returns
/// `None` when there is nothing to serialize (no blocks at all), so the caller
/// can surface a "no code to copy" no-op rather than copying an empty string.
pub(crate) fn render_code_payload(blocks: &[CodeBlock], style: CodePayloadStyle) -> Option<String> {
    if blocks.is_empty() {
        return None;
    }
    let rendered = match style {
        CodePayloadStyle::Fenced => blocks
            .iter()
            .map(render_block_fenced)
            .collect::<Vec<_>>()
            .join("\n\n"),
        CodePayloadStyle::Bare => blocks
            .iter()
            .map(CodeBlock::body)
            .collect::<Vec<_>>()
            .join("\n\n"),
    };
    Some(rendered)
}

/// One block as a clean fenced Markdown block: ```` ```lang ```` (language
/// omitted when the source fence carried none), the interior verbatim, then the
/// closing fence.
fn render_block_fenced(block: &CodeBlock) -> String {
    let mut out = String::with_capacity(block.body().len() + 8);
    out.push_str("```");
    out.push_str(&block.language);
    out.push('\n');
    let body = block.body();
    if !body.is_empty() {
        out.push_str(&body);
        out.push('\n');
    }
    out.push_str("```");
    out
}

/// One-shot helper mirroring [`crate::copy::gather`]: extract the code blocks of
/// `rows` and render them in `style`. `None` when the rows hold no complete
/// fenced block (e.g. plain prose, or a still-streaming unclosed fence) — the
/// caller turns that into a "no code to copy" status.
pub(crate) fn gather_code(rows: &[TranscriptRow], style: CodePayloadStyle) -> Option<String> {
    let blocks = extract_code_blocks(rows);
    render_code_payload(&blocks, style)
}

#[cfg(test)]
#[path = "copy_code_tests.rs"]
mod tests;
