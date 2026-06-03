# 12 — Implemented cost-saving idea batch (2026-06)

Five ideas from the cost-reduction review were implemented in this batch, each
anchored to a verified call site. They target the tool-output and request layers
the rest of this guide covers. All landed with crate tests green; none regress
the realworld graph-vs-nograph eval (the read-only, single-prompt audit doesn't
exercise the cap/shell-overflow/edit/subagent paths most of them touch, so the
"no regression" bar holds by construction — see the eval report under
`docs/internal/eval-findings/graph-cost-wins-report.md`).

| Idea | Layer | Mechanism | Primary files |
|---|---|---|---|
| **B1** True `signature_span` | Context selection (Ch.05) | signature reads now exclude the body | `squeezy-parse/src/lib.rs`, `squeezy-graph/src/lib.rs`, `squeezy-tools/src/graph_tools.rs` |
| **B4** Raw shell sidecar | Tool output (Ch.04) | recover pre-cap bytes the hard cap drops | `squeezy-tools/src/shell.rs`, `shell_spillover.rs` |
| **B6** Pressure governor | Request (Ch.10) | gate the next round at 80% of a configured cost cap | `squeezy-agent/src/cost_broker.rs` |
| **B2/M1** Per-role reasoning | Sub-agent (Ch.08) | Planner=High, Explorer/Reviewer=Low reasoning_effort | `squeezy-agent/src/roles.rs`, `lib.rs` |
| **M2** Expired-context masking | Conversation shape (Ch.02/03) | stub stale reads of a file after it's edited | `squeezy-agent/src/micro_compaction.rs`, `lib.rs` |

## B1 — true `signature_span` for `read_slice`

Chapter 05 claimed `read_slice {span_kind: "signature"}` returns "a declaration
without its body". That was **aspirational, not actual**: the code used the whole
symbol span (`start_byte..end_byte`), so every signature read silently shipped
the body — the `signature` field was text metadata, never a byte range. Parsers
now compute a real `ParsedSymbol::signature_span` (symbol start → body start) in
each language extractor that has a body node, threaded through `GraphSymbol`;
`read_slice` reads it for the Signature case and falls back to the full span
(`None`) only for bodyless symbols and the few boundaries that can't be anchored
safely (JS/TS arrow/expression bodies, Go bare type aliases). Now the saving the
chapter describes is real: ~10–20% off signature reads of small symbols, at zero
quality cost (the header is exactly what was asked for).

## B4 — raw shell sidecar before truncation

Chapter 04 says spillover stores the unshaped raw result, but `read_limited_pipe`
discarded bytes past the hard cap *before* spillover ran, so `read_tool_output`
could never recover them. The full pre-cap (redacted) stream is now mirrored to a
`{call_id}-raw.txt` sidecar (written only on overflow; zero cost under cap),
bounded by the existing 100 MB session budget. A long build log / stack trace the
cap truncated is now fully recoverable on demand.

## B6 — adaptive pressure governor (gate-at-80% variant)

All budget knobs were snapshotted at broker construction and only gated *at*
cap-hit. `CostBroker::pressure_gate()` now refuses to start the next provider
round once spend reaches 80% of a configured session cost cap, surfacing a clear
cost-pressure status instead of silently shrinking per-turn budgets (the
false-economy variant was deliberately not built). No behaviour change when no
cap is set.

## B2/M1 — per-role reasoning budgets (phase 1)

`RoleConfig.reasoning_effort` was cataloged (Planner=High, Explorer/Reviewer=Low)
but never read. It's now threaded into each spawned sub-agent's request through
the existing provider-capability gate, so the high-volume Explore/Review roles
run at Low reasoning while Planner keeps High. The main agent is unchanged
(phase 2 — per-turn budgets from the `turn_router` signal — is left for later).

## M2 — expired-context masking by file-mutation lineage

After a *successful* `apply_patch`/`write_file`, the changed spans are spliced out
of the earlier `read_file`/`read_slice`/grep snapshots of the same file, in place,
into the existing `MICRO_COMPACT_CLEARED_PREFIX` recovery stub — zero extra model
call. Scoped to the changed span (surrounding context survives), gated on
`ToolStatus::Success`, honors `micro_compaction_keep_recent`, and only splices on
a net byte win. Attacks the quadratic N(N+1)/2 input term that threshold
compaction and SHA-dedup miss. (Folds into `mid_turn_compacted` so the rewrite
reaches the provider even on a `previous_response_id` chain.)
