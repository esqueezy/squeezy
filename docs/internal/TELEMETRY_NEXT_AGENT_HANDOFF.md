# Telemetry Next-Agent Handoff

This branch currently implements the expanded event-based telemetry pass. The
next follow-up should migrate high-frequency telemetry into one bounded
`squeezy_session_summary` event built from a local durable telemetry ledger.

## Current Implementation

- `squeezy_startup_ready`: emitted on the first interactive TUI draw with
  startup route and elapsed startup time.
- `squeezy_session_ended`: emitted during session finish with duration, final
  status, turn count, tool result counts, budget denials, and subagent counts.
- `squeezy_graph_build_completed`: emitted from deferred graph open in
  `squeezy-tools` with build timing, file counts, language distribution,
  exclusions, persistence cache counts, symbols, and edges.
- `squeezy_slash_command_used`: emitted for TUI composer, TUI inline, and
  headless agent dispatch. Command names are sanitized tokens derived from the
  canonical slash head; unknown heads are reported as `unknown`.
- `squeezy_config_change_committed`: accumulated while `/config` is open and
  emitted only after the pane closes. Fields come from config schema metadata;
  values are bucketed, never raw.
- The telemetry client now buffers safely when called from sync code without a
  Tokio runtime.
- The Worker allowlist accepts the new properties and treats `slash_command` as
  a bounded token, not a manually maintained command enum.

## One-Event Summary Direction

Use local records as the source of truth and PostHog as the aggregate sink:

1. Persist a local telemetry record at the moment a safe fact happens.
2. Include `occurred_at_ms` and a monotonic local sequence on each record.
3. On session exit, sort records by `(occurred_at_ms, sequence)` and reduce them
   into one bounded `squeezy_session_summary` event.
4. Store the summary as pending before sending.
5. If sending fails, retry pending summaries on next session start before new
   telemetry is sent.
6. On startup, detect prior sessions that have a start record but no clean end
   record and synthesize a summary with an abnormal status.

The raw local ledger can be more detailed than PostHog, but the remote summary
must stay bounded and aggregate-first.

## Exclusions From The Single Summary

Keep these out of the one summary event:

- Explicit user-consented flows: `/feedback` and `/report` should keep their
  separate direct endpoints and preview/redaction behavior.
- Raw user content: prompts, model responses, file contents, snippets, shell
  commands, command output, URLs, environment values, API keys, and raw settings
  values.
- Raw paths, repository names, session titles, labels, custom model ids, template
  names, slash arguments, tool arguments, or opaque hashes that can fingerprint
  user content.
- Unbounded nested event arrays. If a section grows beyond a cap, summarize the
  top buckets and include `truncated = true` plus dropped counts.
- Full local diagnostic timelines by default. If needed, add opt-in diagnostic
  upload or low-rate sampling later.

Candidate direct-send exceptions besides feedback/report are only fatal errors
that cannot be durably recorded first. Prefer durable local recording and
next-start recovery whenever possible.

## Suggested Summary Sections

- Session: started/ended timestamps, duration, status, abnormal-exit flag.
- Startup: route, time to placeholder draw, agent build, first interactive draw.
- Graph: build/refresh counts, duration buckets, file/language/exclusion/cache
  counts, error counts.
- Slash usage: counts by command token, surface, outcome, alias kind, arg shape.
- Config: counts by scope, section, field id, apply tier, change kind, value
  bucket transition.
- Tools: counts by tool family/name/status, duration buckets, bytes/read/search
  buckets, output buckets.
- Failures: counts by coarse error kind and phase.
- Cost/context: aggregate token/cost/cache/budget counters already captured in
  turn/session metrics.

## Files Touched In This Branch

- `crates/squeezy-telemetry/src/lib.rs`
- `crates/squeezy-telemetry/src/lib_tests.rs`
- `crates/squeezy-agent/src/lib.rs`
- `crates/squeezy-tools/src/lib.rs`
- `crates/squeezy-tools/Cargo.toml`
- `crates/squeezy-tui/src/lib.rs`
- `crates/squeezy-tui/src/config_screen.rs`
- `crates/squeezy-tui/src/config_screen/keys.rs`
- `crates/squeezy-tui/src/config_screen/save.rs`
- `infra/telemetry-worker/src/worker.ts`
- `infra/telemetry-worker/tests/worker.test.ts`
- `crates/squeezy-skills/external-docs/TELEMETRY.md`

## Validation Run

- `cargo fmt --all`
- `cargo test -p squeezy-telemetry`
- `cargo check -p squeezy-agent -p squeezy-tools -p squeezy-tui`
- `cargo test -p squeezy-tui config_screen --lib`
- `bun test` in `infra/telemetry-worker`
- `bun run typecheck` in `infra/telemetry-worker`
- `git diff --check`
