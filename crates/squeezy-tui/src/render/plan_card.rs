//! Styled "plan card" renderer.
//!
//! Plan-mode v3 (PR-F) replaces the original log-line representation of
//! newly proposed plans with a structured cell:
//!
//! ```text
//! ● Plan plan-abc12 (4 steps)
//!   .squeezy/plans/<session>/plan-abc12.md
//!
//!   <markdown-rendered body>
//!
//!   <unified diff vs parent plan, if any>
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
use crate::render::{markdown, palette};

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

/// Background tint applied to every line of the card. Picked to read
/// well against both light and dark backgrounds via the existing
/// palette adapter; falls back to no background on `NO_COLOR` to stay
/// accessible.
fn card_background() -> Style {
    let tone = palette::palette_tone();
    let (r, g, b) = match tone {
        palette::PaletteTone::Dark => (28, 25, 38),
        palette::PaletteTone::Light => (245, 240, 255),
    };
    Style::default().bg(palette::best_color((r, g, b)))
}

/// Accent color for the leading bullet and the plan id header. Mirrors
/// `MODE_PURPLE` so the card visually ties to the Plan-mode indicator.
fn accent_style() -> Style {
    Style::default()
        .fg(palette::MODE_PURPLE)
        .add_modifier(Modifier::BOLD)
}

/// Top-of-card render entry point. Pulls the body from disk and
/// composes the styled lines. Returns a single-line fallback ("plan
/// file missing") when the file has been deleted out from under us so
/// the transcript never silently empties.
pub(crate) fn render_plan_card(data: &PlanCardData) -> Vec<Line<'static>> {
    let body = match proposed_plan::read_plan_body(&data.path) {
        Ok(body) => body,
        Err(_) => return missing_file_card(data),
    };
    let mut lines = Vec::new();
    let step_count = crate::count_plan_steps(&body);
    lines.push(card_header_line(&data.plan_id, step_count));
    lines.push(card_path_line(&data.path));
    lines.push(blank_card_line());
    for line in markdown::render_markdown(&body) {
        lines.push(apply_card_background(line));
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
    lines
}

/// Card shown when the backing file is missing. Stays in palette so
/// the transcript layout doesn't jump.
fn missing_file_card(data: &PlanCardData) -> Vec<Line<'static>> {
    let bg = card_background();
    vec![
        Line::from(vec![
            Span::styled("● Plan ", accent_style().patch(bg)),
            Span::styled(data.plan_id.clone(), accent_style().patch(bg)),
            Span::styled(
                " — file missing",
                Style::default().fg(palette::ERROR_RED).patch(bg),
            ),
        ]),
        Line::from(vec![Span::styled(
            data.path.display().to_string(),
            Style::default().fg(palette::QUIET).patch(bg),
        )]),
    ]
}

fn card_header_line(plan_id: &str, step_count: usize) -> Line<'static> {
    let bg = card_background();
    let mut spans = vec![
        Span::styled("● ", accent_style().patch(bg)),
        Span::styled("Plan ", accent_style().patch(bg)),
        Span::styled(plan_id.to_string(), accent_style().patch(bg)),
    ];
    if step_count > 0 {
        let suffix = if step_count == 1 {
            " (1 step)".to_string()
        } else {
            format!(" ({step_count} steps)")
        };
        spans.push(Span::styled(
            suffix,
            Style::default().fg(palette::QUIET).patch(bg),
        ));
    }
    Line::from(spans)
}

fn card_path_line(path: &Path) -> Line<'static> {
    let bg = card_background();
    Line::from(vec![Span::styled(
        path.display().to_string(),
        Style::default().fg(palette::QUIET).patch(bg),
    )])
}

fn blank_card_line() -> Line<'static> {
    Line::from(vec![Span::styled(String::new(), card_background())])
}

fn apply_card_background(line: Line<'static>) -> Line<'static> {
    let bg = card_background();
    let spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|span| {
            let style = span.style.patch(bg);
            Span::styled(span.content, style)
        })
        .collect();
    Line::from(spans)
}

fn diff_header_line(parent_id: &str) -> Line<'static> {
    let bg = card_background();
    Line::from(vec![
        Span::styled("diff vs ", Style::default().fg(palette::QUIET).patch(bg)),
        Span::styled(
            parent_id.to_string(),
            Style::default()
                .fg(palette::MODE_PURPLE)
                .add_modifier(Modifier::ITALIC)
                .patch(bg),
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
    let bg = card_background();
    let diff = TextDiff::from_lines(parent, current);
    let mut out = Vec::new();
    for change in diff.iter_all_changes() {
        let (sigil, fg) = match change.tag() {
            similar::ChangeTag::Equal => (' ', palette::QUIET),
            similar::ChangeTag::Insert => ('+', palette::DIFF_ADD_FG),
            similar::ChangeTag::Delete => ('-', palette::DIFF_DEL_FG),
        };
        let text = change.to_string();
        let text = text.trim_end_matches('\n').to_string();
        let span_text = format!("  {sigil} {text}");
        out.push(Line::from(vec![Span::styled(
            span_text,
            Style::default().fg(fg).patch(bg),
        )]));
    }
    out
}

#[cfg(test)]
#[path = "plan_card_tests.rs"]
mod tests;
