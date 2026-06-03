//! Styled "plan card" renderer.
//!
//! Plan-mode v3 (PR-F) replaces the original log-line representation of
//! newly proposed plans with a structured cell:
//!
//! ```text
//! ╭─ Plan plan-abc12 · 4 steps ─╮
//! │ .squeezy/plans/<session>/... │
//! │                              │
//! │ <markdown-rendered body>     │
//! │                              │
//! │ <diff vs parent, if any>     │
//! ╰──────────────────────────────╯
//! ```
//!
//! Body text and the diff are read from disk at render time so the card
//! survives transcript compaction (the persisted plan file is the
//! source of truth, not the cell's captured snapshot).

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use similar::TextDiff;
use std::path::{Path, PathBuf};

use crate::proposed_plan;
use crate::render::markdown;

/// Static metadata captured at the moment a plan was persisted. The
/// actual body is *not* cached here — readers go through
/// [`proposed_plan::read_plan_body`] so PR-G's auto-checkmarks and
/// PR-C's in-place refinements are reflected automatically.
#[derive(Debug, Clone)]
pub(crate) struct PlanCardData {
    pub plan_id: String,
    pub path: PathBuf,
    pub parent_plan_id: Option<String>,
}

/// Top-of-card render entry point. Pulls the body from disk and
/// composes the styled lines. Returns a single-line fallback ("plan
/// file missing") when the file has been deleted out from under us so
/// the transcript never silently empties.
///
/// `width` is the terminal column count the card will be painted into.
/// The box is clamped so it never exceeds it; without a clamp a single
/// long prose line makes the card wider than the viewport and the
/// `Wrap { trim: false }` paint shatters the border rows.
pub(crate) fn render_plan_card(data: &PlanCardData, width: Option<u16>) -> Vec<Line<'static>> {
    let body = match proposed_plan::read_plan_body(&data.path) {
        Ok(body) => body,
        Err(_) => return missing_file_card(data, width),
    };
    let step_count = crate::count_plan_steps(&body);
    let mut lines = Vec::new();
    lines.push(card_path_line(&data.path));
    lines.push(blank_card_line());
    for line in markdown::render_markdown(&body) {
        lines.push(line);
    }
    lines.push(blank_card_line());

    if let Some(parent_id) = data.parent_plan_id.as_deref() {
        let parent_path = sibling_plan_path(&data.path, parent_id);
        if let Ok(parent_body) = proposed_plan::read_plan_body(&parent_path) {
            lines.push(diff_header_line(parent_id));
            lines.extend(render_plan_diff(&parent_body, &body));
            lines.push(blank_card_line());
        }
    }
    boxed_card_lines(plan_title(&data.plan_id, step_count), lines, width)
}

/// Card shown when the backing file is missing. Stays in palette so
/// the transcript layout doesn't jump.
fn missing_file_card(data: &PlanCardData, width: Option<u16>) -> Vec<Line<'static>> {
    boxed_card_lines(
        format!("Plan {} · file missing", data.plan_id),
        vec![Line::from(vec![Span::styled(
            data.path.display().to_string(),
            Style::default().fg(crate::render::theme::red()),
        )])],
        width,
    )
}

fn plan_title(plan_id: &str, step_count: usize) -> String {
    match step_count {
        0 => format!("Plan {plan_id}"),
        1 => format!("Plan {plan_id} · 1 step"),
        _ => format!("Plan {plan_id} · {step_count} steps"),
    }
}

fn card_path_line(path: &Path) -> Line<'static> {
    Line::from(vec![Span::styled(
        path.display().to_string(),
        Style::default().fg(crate::render::theme::quiet()),
    )])
}

fn blank_card_line() -> Line<'static> {
    Line::from("")
}

/// Minimum inner box width, chosen so short headers don't collapse to a
/// sliver. The four-column frame (`│ ` + ` │`) is added on top.
const MIN_INNER_WIDTH: usize = 24;
/// Columns the box frame itself consumes: a leading `│ ` and trailing
/// ` │` on every content row. `inner_width + BOX_FRAME_COLS` is the full
/// painted width, which must stay within the terminal.
const BOX_FRAME_COLS: usize = 4;

fn boxed_card_lines(
    title: String,
    inner: Vec<Line<'static>>,
    width: Option<u16>,
) -> Vec<Line<'static>> {
    let title_width = text_width(&title);
    let content_width = inner.iter().map(line_width).max().unwrap_or(0);
    let mut inner_width = content_width
        .saturating_add(2)
        .max(title_width.saturating_add(3))
        .max(MIN_INNER_WIDTH);
    // Clamp so the full box (content + frame) fits the viewport; without
    // this a single long line makes the card wider than the terminal and
    // `Wrap { trim: false }` splits every border row, shattering the box.
    if let Some(cols) = width {
        let max_inner = usize::from(cols).saturating_sub(BOX_FRAME_COLS);
        if max_inner >= MIN_INNER_WIDTH {
            inner_width = inner_width.min(max_inner);
        }
    }
    let border = Style::default()
        .fg(crate::render::theme::accent())
        .add_modifier(Modifier::BOLD);
    let mut lines = Vec::with_capacity(inner.len() + 2);
    let title_fill = inner_width.saturating_sub(title_width.saturating_add(3));
    lines.push(Line::from(vec![
        Span::styled("╭─ ", border),
        Span::styled(title, border),
        Span::styled(format!(" {}╮", "─".repeat(title_fill)), border),
    ]));
    // Content rows get two columns of inner padding (`│ ` … ` │`), so the
    // text itself may be at most `inner_width - 2` wide before it would
    // force the box past `inner_width`.
    let text_budget = inner_width.saturating_sub(2).max(1);
    for line in inner {
        for row in wrap_line(line, text_budget) {
            lines.push(boxed_content_line(row, inner_width));
        }
    }
    lines.push(Line::from(vec![Span::styled(
        format!("╰{}╯", "─".repeat(inner_width)),
        border,
    )]));
    lines
}

/// Hard-wrap a styled line to at most `width` display columns per row,
/// splitting on char boundaries and preserving each span's style. A blank
/// line yields a single empty row so vertical spacing is kept.
fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if line_width(&line) <= width {
        return vec![line];
    }
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut row_spans: Vec<Span<'static>> = Vec::new();
    let mut row_width = 0usize;
    for span in line.spans {
        let style = span.style;
        let mut chunk = String::new();
        for ch in span.content.chars() {
            if row_width >= width {
                if !chunk.is_empty() {
                    row_spans.push(Span::styled(std::mem::take(&mut chunk), style));
                }
                rows.push(Line::from(std::mem::take(&mut row_spans)));
                row_width = 0;
            }
            chunk.push(ch);
            row_width += 1;
        }
        if !chunk.is_empty() {
            row_spans.push(Span::styled(chunk, style));
        }
    }
    if !row_spans.is_empty() {
        rows.push(Line::from(row_spans));
    }
    rows
}

fn boxed_content_line(line: Line<'static>, inner_width: usize) -> Line<'static> {
    let border = Style::default()
        .fg(crate::render::theme::accent())
        .add_modifier(Modifier::BOLD);
    let content_width = line_width(&line);
    let padding = inner_width.saturating_sub(content_width.saturating_add(2));
    let mut spans = vec![Span::styled("│ ", border)];
    spans.extend(line.spans);
    if padding > 0 {
        spans.push(Span::raw(" ".repeat(padding)));
    }
    spans.push(Span::styled(" │", border));
    Line::from(spans)
}

fn line_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| text_width(span.content.as_ref()))
        .sum()
}

fn text_width(text: &str) -> usize {
    text.chars().count()
}

fn diff_header_line(parent_id: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "diff vs ",
            Style::default().fg(crate::render::theme::quiet()),
        ),
        Span::styled(
            parent_id.to_string(),
            Style::default()
                .fg(crate::render::theme::magenta())
                .add_modifier(Modifier::ITALIC),
        ),
    ])
}

/// Construct a sibling plan file's absolute path. Plan files live next
/// to each other inside the session's plan dir, so we just rewrite the
/// last path component.
fn sibling_plan_path(path: &Path, sibling_id: &str) -> PathBuf {
    let mut parent = path.to_path_buf();
    parent.pop();
    parent.join(format!("{sibling_id}.md"))
}

/// Unified diff between the parent plan body and the new plan body,
/// rendered with the same color scheme as the patch viewer. Lines are
/// indented two spaces so they read as a sub-section of the card.
pub(crate) fn render_plan_diff(parent: &str, current: &str) -> Vec<Line<'static>> {
    let diff = TextDiff::from_lines(parent, current);
    let mut out = Vec::new();
    for change in diff.iter_all_changes() {
        let (sigil, fg) = match change.tag() {
            similar::ChangeTag::Equal => (' ', crate::render::theme::quiet()),
            similar::ChangeTag::Insert => (
                '+',
                crate::render::theme::color(crate::render::theme::token::DIFF_ADDED),
            ),
            similar::ChangeTag::Delete => (
                '-',
                crate::render::theme::color(crate::render::theme::token::DIFF_REMOVED),
            ),
        };
        let text = change.to_string();
        let text = text.trim_end_matches('\n').to_string();
        let span_text = format!("  {sigil} {text}");
        out.push(Line::from(vec![Span::styled(
            span_text,
            Style::default().fg(fg),
        )]));
    }
    out
}

#[cfg(test)]
#[path = "plan_card_tests.rs"]
mod tests;
