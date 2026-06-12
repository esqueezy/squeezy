//! Actionable Tool Outputs (§12.3.1): scan a single tool result's output text
//! for *actionable elements* — file paths, URLs, error lines, diff hunks, and
//! shell commands — and surface each as a row that offers safe run/open/copy/jump
//! affordances.
//!
//! ## What this module owns
//!
//! Like the other §12 leaf modules (`action_palette`, `error_lens`,
//! `change_summary`), this file is a **pure model**: it owns the *vocabulary*
//! ([`ActionableKind`], [`WorkflowAction`], [`ActionableItem`]) and the *detection
//! rule* ([`detect_actionable_items`]) and nothing about geometry, rendering, or
//! input. `lib.rs` feeds it the focused tool result's bounded output text plus the
//! source entry id, gets back a list of detected items, and routes each item's
//! chosen action through the *same* handlers the keyboard already drives — copy
//! through the one `deliver_copy` funnel, jump through `jump_to_entry_id`. So
//! keyboard and mouse reach identical behaviour by construction and nothing here
//! mutates the transcript or spawns a process.
//!
//! ## Safety: copy/jump, never an unguarded run
//!
//! The spec flags retry/run as safety-sensitive ("Retry never bypasses sandbox or
//! approval") and says to "degrade to copy/open-output when uncertain". This model
//! therefore offers only the two affordances that can never bypass a sandbox or an
//! approval gate:
//!
//!   - **Copy** — put the matched element (a path, a URL, an error line, a command
//!     line, a diff hunk header) on the clipboard so the user can paste it into a
//!     terminal, a browser, or a new prompt. Always available.
//!   - **Jump** — scroll the main view to the tool result the element came from,
//!     the "jump to file/location" verb realized against transcript-resident
//!     output. Available for every item (they all live in one entry).
//!
//! "Open a URL" and "run the command" degrade to *copy the URL* / *copy the
//! command*: the safe, portable affordance that works on every terminal and never
//! launches anything behind the user's back. A future build can layer an OSC 8
//! open or an approval-gated retry on top without changing this vocabulary.
//!
//! ## Conservative detection
//!
//! Path/URL/command/diff/error detection can false-positive (the spec warns of
//! this), so the detectors are deliberately strict and the *copy* fallback is
//! always safe even when a match is loose. Each output line yields at most one
//! item (the first, most-specific match wins), and the total is bounded by
//! [`ITEMS_CAP`] so a pathological wall of paths can never produce thousands of
//! rows. An empty / whitespace-only output yields no items and the overlay paints
//! an honest empty state.

/// The semantic kind of one detected actionable element. A small, fixed set —
/// one per element family the spec calls out (file paths, URLs, errors, diffs,
/// commands). Ordered so [`ActionableKind::ALL`] reads the way the overlay groups
/// them (the navigable locations first, then the copyables).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ActionableKind {
    /// A file path, optionally with a `:line[:col]` suffix (a compiler/test/grep
    /// location). Copy puts the path on the clipboard; jump scrolls to the source
    /// entry.
    Path,
    /// A URL (`http`/`https`/`file` scheme). Copy puts the URL on the clipboard
    /// (the safe "open" degrade — never launches a browser).
    Url,
    /// An error / failure line (rustc, cargo, test, panic, permission, …). Copy
    /// puts the line on the clipboard for a follow-up prompt or a search.
    Error,
    /// A unified-diff hunk or file header (`+++ b/...`, `--- a/...`, `@@ ... @@`,
    /// `diff --git ...`). Copy puts the header on the clipboard.
    Diff,
    /// A shell command line (a `$ cmd ...` prompt echo, or a `cmd: ...` / run
    /// banner). Copy puts the command on the clipboard (the safe "run" degrade —
    /// never executes it).
    Command,
}

impl ActionableKind {
    /// Every kind, in overlay grouping order. Exhaustive on purpose: a new variant
    /// must be added here or it never appears in the summary / coverage tests.
    pub(crate) const ALL: &'static [ActionableKind] = &[
        ActionableKind::Path,
        ActionableKind::Url,
        ActionableKind::Error,
        ActionableKind::Diff,
        ActionableKind::Command,
    ];

    /// Short, screen-reader-friendly label for the kind tag. ASCII only (no
    /// glyphs) so meaning never depends on color or a private-use codepoint.
    pub(crate) fn label(self) -> &'static str {
        match self {
            ActionableKind::Path => "path",
            ActionableKind::Url => "url",
            ActionableKind::Error => "error",
            ActionableKind::Diff => "diff",
            ActionableKind::Command => "command",
        }
    }

    /// Whether an item of this kind offers a *jump* affordance in addition to
    /// copy. Every kind lives inside one tool result, so jumping to that entry is
    /// always meaningful — the affordance is total today, but kept as a method so a
    /// future kind that is purely external (e.g. a bare URL with no transcript
    /// anchor) can opt out without touching the dispatch.
    pub(crate) fn supports_jump(self) -> bool {
        true
    }
}

/// A safe affordance offered for a detected item. Each maps 1:1 to an existing,
/// already-tested handler the keyboard already reaches (copy through the single
/// `deliver_copy` funnel, jump through `jump_to_entry_id`), so invoking it from
/// the overlay is the same behaviour as the chord — never a new mutation and never
/// a process spawn. Ordered so [`WorkflowAction::ALL`] reads the way a menu flows
/// (the primary copy first, then the navigation jump).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum WorkflowAction {
    /// Copy the matched element's text (path / URL / line / command / hunk) to the
    /// clipboard. The primary, always-available affordance.
    Copy,
    /// Jump the main view to the tool result the element came from.
    Jump,
}

impl WorkflowAction {
    /// Every action, in menu order. Exhaustive on purpose: a new variant must be
    /// added here or it never appears in a row's offered set.
    pub(crate) const ALL: &'static [WorkflowAction] = &[WorkflowAction::Copy, WorkflowAction::Jump];

    /// Short ASCII label for the affordance, used in the row's hint.
    pub(crate) fn label(self) -> &'static str {
        match self {
            WorkflowAction::Copy => "copy",
            WorkflowAction::Jump => "jump",
        }
    }
}

/// One detected actionable element inside a tool result's output (§12.3.1).
///
/// `entry_id` is the stable `TranscriptEntry::id` of the tool result it lives in
/// (the jump target); `kind` classifies it; `text` is the row *label* — the matched
/// element (path, URL, line, command, or hunk header) bounded by [`TEXT_CAP`] — while
/// `copy_text` holds the same match *untruncated* so the clipboard never receives a
/// capped, ellipsized payload; `line_index` is the 0-based line offset within the
/// output for stable ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionableItem {
    pub(crate) entry_id: u64,
    pub(crate) line_index: usize,
    pub(crate) kind: ActionableKind,
    /// The display label shown on the overlay row: the matched slice capped to
    /// [`TEXT_CAP`] chars with a trailing ellipsis when it was cut, so a giant
    /// path/URL can never blow up the row width.
    pub(crate) text: String,
    /// The full, untruncated matched slice delivered to the clipboard when the row
    /// is copied. Kept separate from `text` so the display cap never corrupts the
    /// payload (a 200-char URL copies whole, not capped + `\u{2026}`).
    pub(crate) copy_text: String,
}

impl ActionableItem {
    /// The action that runs when the row is activated with Enter / clicked: copy,
    /// the safe, always-available primary. Jump is the secondary verb the overlay
    /// exposes on a separate key.
    pub(crate) fn primary_action(&self) -> WorkflowAction {
        WorkflowAction::Copy
    }

    /// The set of affordances offered for this item, in [`WorkflowAction::ALL`]
    /// order. Copy is always offered; jump only when the kind supports it.
    pub(crate) fn actions(&self) -> Vec<WorkflowAction> {
        WorkflowAction::ALL
            .iter()
            .copied()
            .filter(|action| match action {
                WorkflowAction::Copy => true,
                WorkflowAction::Jump => self.kind.supports_jump(),
            })
            .collect()
    }
}

/// Largest number of characters retained in an item's `text`. A giant single-line
/// path or a base64 blob masquerading as a command would otherwise blow up the
/// overlay row; we keep a generous-but-bounded prefix.
const TEXT_CAP: usize = 160;

/// Largest number of items kept for one tool result. A pathological output that
/// prints thousands of paths should not produce thousands of overlay rows; we keep
/// the actionable head.
pub(crate) const ITEMS_CAP: usize = 24;

/// Strip ANSI/VT escape sequences (CSI `\x1b[...m` and a bare two-byte escape) so
/// color/cursor control does not defeat the detectors or pollute the copy payload.
/// Self-contained so the pure module has no dependency on the renderer's stripper
/// (mirrors `error_lens::strip_ansi`).
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Truncate `s` to at most [`TEXT_CAP`] chars (on a char boundary), appending an
/// ellipsis when it was cut.
fn cap_text(s: &str) -> String {
    if s.chars().count() <= TEXT_CAP {
        return s.to_string();
    }
    let prefix: String = s.chars().take(TEXT_CAP).collect();
    format!("{prefix}\u{2026}")
}

/// Classify and extract the most-specific actionable element on one already
/// ANSI-stripped, non-empty line, or `None` when the line carries nothing
/// actionable. The order of the checks is deliberate, most-specific first:
///
///   1. A **diff** marker (`diff --git`, `+++`/`---` file headers, `@@` hunk) — a
///      structured line whose shape is unambiguous.
///   2. A **URL** anywhere on the line — `find_links` already detects URL runs
///      conservatively, and a URL is a strong, unambiguous signal.
///   3. A **command** echo — a `$ ` / `> ` shell prompt, or a `$ `-less line that
///      a `command:` / `Running` banner introduces.
///   4. An **error** line — reuse the same case-insensitive failure substrings the
///      error-lens detector uses, kept small here so the module stays standalone.
///   5. A **path** run — `find_links` also detects absolute-path runs; a bare path
///      is the loosest signal so it is tried last.
///
/// Whatever matches, the stored `text` is the *most useful copyable slice*: the
/// whole trimmed line for diff/command/error (the user wants the line), or the
/// matched URL / path run for url/path (the user wants the address, not the prose
/// around it).
fn classify_line(line: &str) -> Option<(ActionableKind, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();

    // 1. Diff markers. The hunk header / file headers / git diff banner are the
    //    unambiguous structured shapes; a bare `+`/`-` body line is NOT treated as
    //    a diff (too noisy — it would match ordinary prose bullets).
    if trimmed.starts_with("diff --git ")
        || trimmed.starts_with("@@ ")
        || trimmed.starts_with("+++ ")
        || trimmed.starts_with("--- ")
        || trimmed.starts_with("index ") && trimmed.contains("..")
    {
        return Some((ActionableKind::Diff, trimmed.to_string()));
    }

    // 2. URL anywhere on the line. `find_links` returns conservative URL runs;
    //    take the first whose scheme is a web/file URL (not a bare file path,
    //    which we classify as a Path below so the kind tag reads honestly).
    if let Some(uri) = first_url(trimmed) {
        return Some((ActionableKind::Url, uri));
    }

    // 3. Command echo: a `$ ` or `> ` shell prompt, or a `command:`/`running:`
    //    banner. The copyable slice is the command itself (prompt/banner stripped).
    if let Some(cmd) = command_after_prompt(trimmed) {
        return Some((ActionableKind::Command, cmd));
    }

    // 4. Error / failure line. Case-insensitive substrings mirroring the
    //    error-lens families, kept compact so this module stays standalone.
    if is_error_line(&lower) {
        return Some((ActionableKind::Error, trimmed.to_string()));
    }

    // 5. Absolute-path run (the loosest signal, tried last). `find_links` detects
    //    `file://`-prefixed absolute paths; strip the scheme back off so the
    //    copyable text is the bare path the user expects.
    if let Some(path) = first_path(trimmed) {
        return Some((ActionableKind::Path, path));
    }

    None
}

/// The first web/file *URL* run on the line (not a bare file path), or `None`.
/// Reuses `hyperlinks::find_links`, keeping a single conservative URL detector for
/// the whole crate; a `file://` path that `find_links` returns is filtered out
/// here because the bare-path branch classifies it as a [`ActionableKind::Path`].
fn first_url(line: &str) -> Option<String> {
    for span in crate::hyperlinks::find_links(line) {
        let uri = &line[span.start..span.end];
        if uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("file://") {
            return Some(uri.to_string());
        }
    }
    None
}

/// The first absolute-path run on the line, with any `file://` scheme `find_links`
/// would prepend stripped back off, or `None`. The visible run from `find_links`
/// is already the bare path (its `uri` is what gets the scheme), so we read the
/// span's text directly.
fn first_path(line: &str) -> Option<String> {
    for span in crate::hyperlinks::find_links(line) {
        let run = &line[span.start..span.end];
        // `find_links` returns both URL runs and bare absolute-path runs; a path
        // run is the one that is NOT a scheme URL.
        if !(run.starts_with("http://") || run.starts_with("https://"))
            && run.starts_with('/')
            && run.len() > 1
        {
            return Some(run.to_string());
        }
    }
    None
}

/// If the line begins with a shell prompt (`$ `, `> `) or a run banner
/// (`command:`, `running:`, `+ ` from `set -x`), return the command text after
/// it; else `None`. The returned command is trimmed and must be non-empty.
fn command_after_prompt(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let candidate = if let Some(rest) = line.strip_prefix("$ ") {
        rest
    } else if let Some(rest) = line.strip_prefix("> ") {
        rest
    } else if let Some(rest) = line.strip_prefix("+ ") {
        // `set -x` trace lines. Require it to look like a command (a word), not a
        // bare `+ 1` arithmetic trace.
        rest
    } else if let Some(rest) = lower
        .strip_prefix("command:")
        .or_else(|| lower.strip_prefix("running:"))
    {
        // Use the original-case slice at the same byte offset so casing is kept.
        let offset = line.len() - rest.len();
        &line[offset..]
    } else {
        return None;
    };
    let cmd = candidate.trim();
    // Must start with a command-like token (a letter, `/`, `.`, or `_`) so a bare
    // `$ ` or a `+ 1` arithmetic trace does not become a phantom command.
    let first = cmd.chars().next()?;
    if !(first.is_ascii_alphabetic() || matches!(first, '/' | '.' | '_')) {
        return None;
    }
    Some(cmd.to_string())
}

/// True when an already-lowercased line carries a failure/error signal. A compact
/// subset of the error-lens families (rustc/cargo/test/panic/permission/network)
/// — enough to surface the actionable error lines a tool output prints, while the
/// `copy` affordance keeps even a loose match safe.
fn is_error_line(lower: &str) -> bool {
    lower.starts_with("error:")
        || lower.starts_with("error[")
        || lower.contains("panicked at")
        || lower.contains("fatal:")
        || lower.contains("fatal error")
        || lower.contains("assertion failed")
        || lower.contains("test result: failed")
        || lower.ends_with("... failed")
        || lower.contains("permission denied")
        || lower.contains("connection refused")
        || lower.contains("could not compile")
        || lower.contains("traceback (most recent call last)")
        || lower.contains("exception:")
}

/// Scan one tool result's output `text` into actionable items (§12.3.1). Pure and
/// standalone so it is the unit-testable heart of the feature. Walks line by line,
/// strips ANSI, classifies each line into its most-specific actionable kind, and
/// emits one capped item per matching line, stamped with `entry_id` so the jump
/// knows where to land. Capped at [`ITEMS_CAP`]; non-actionable lines are skipped.
pub(crate) fn detect_actionable_items(entry_id: u64, text: &str) -> Vec<ActionableItem> {
    let mut items = Vec::new();
    for (line_index, raw) in text.lines().enumerate() {
        let clean = strip_ansi(raw);
        let Some((kind, matched)) = classify_line(&clean) else {
            continue;
        };
        let trimmed = matched.trim();
        items.push(ActionableItem {
            entry_id,
            line_index,
            kind,
            text: cap_text(trimmed),
            copy_text: trimmed.to_string(),
        });
        if items.len() >= ITEMS_CAP {
            break;
        }
    }
    items
}

/// The open Actionable Tool Outputs overlay (§12.3.1): the detected items for one
/// focused tool result, plus a cursor into the list and the source entry id. Built
/// fresh each time the overlay opens via [`ToolActions::open`]; the resting state
/// is `None` on the app (the overlay closed), so a session that never opens it
/// costs nothing and an idle session pays nothing (no per-frame rebuild — the items
/// are a one-shot snapshot of the focused result).
#[derive(Debug, Clone)]
pub(crate) struct ToolActions {
    /// Stable `TranscriptEntry::id` of the focused tool result the items came from
    /// (the jump target and the header anchor).
    entry_id: u64,
    /// A short, deterministic, secret-free one-line label for the focused tool
    /// result (its tool name), painted in the header so the overlay shows *which*
    /// result it is acting on. Bounded by the caller.
    title: String,
    /// The detected items, in output order.
    items: Vec<ActionableItem>,
    /// Cursor into `items`. Clamped to the item count on every move.
    selected: usize,
}

impl ToolActions {
    /// Open the overlay over a focused tool result's detected `items` (which may be
    /// empty — the render path paints an honest empty state). The cursor parks on
    /// the first item.
    pub(crate) fn open(entry_id: u64, title: String, items: Vec<ActionableItem>) -> Self {
        Self {
            entry_id,
            title,
            items,
            selected: 0,
        }
    }

    /// The source tool result's stable entry id.
    pub(crate) fn entry_id(&self) -> u64 {
        self.entry_id
    }

    /// The header title (the focused tool result's name).
    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    /// The detected items in output order.
    pub(crate) fn items(&self) -> &[ActionableItem] {
        &self.items
    }

    /// Number of detected items.
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether no actionable item was detected (the empty-state case).
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The current cursor index, clamped to the last item.
    pub(crate) fn selected(&self) -> usize {
        self.selected.min(self.items.len().saturating_sub(1))
    }

    /// The item under the cursor, or `None` when the list is empty.
    pub(crate) fn selected_item(&self) -> Option<&ActionableItem> {
        self.items.get(self.selected())
    }

    /// Move the cursor to `index`, clamped to the item range. The mouse click path
    /// uses this so a click lands and stays on exactly the clicked row.
    pub(crate) fn select(&mut self, index: usize) {
        if self.items.is_empty() {
            self.selected = 0;
        } else {
            self.selected = index.min(self.items.len() - 1);
        }
    }

    /// Move the cursor up one (saturating at the top).
    pub(crate) fn move_up(&mut self) {
        self.selected = self.selected().saturating_sub(1);
    }

    /// Move the cursor down one (clamped to the last item).
    pub(crate) fn move_down(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected() + 1).min(self.items.len() - 1);
        }
    }

    /// A compact one-line summary of the detected items for the overlay header,
    /// e.g. `"5 items \u{00b7} 2 path \u{00b7} 1 url \u{00b7} 2 error"`. Empty
    /// string when nothing was detected.
    pub(crate) fn summary(&self) -> String {
        if self.items.is_empty() {
            return String::new();
        }
        let total = self.items.len();
        let total_word = if total == 1 { "item" } else { "items" };
        let mut parts = vec![format!("{total} {total_word}")];
        for kind in ActionableKind::ALL.iter().copied() {
            let n = self.items.iter().filter(|i| i.kind == kind).count();
            if n > 0 {
                parts.push(format!("{n} {}", kind.label()));
            }
        }
        parts.join(" \u{00b7} ")
    }
}

#[cfg(test)]
#[path = "tool_actions_tests.rs"]
mod tests;
