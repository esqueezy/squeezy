# Adversarial diagnoses on remaining mini-vs-codex losses

Three independent subagent diagnoses on the LOSS cells (java 2.06×, scala 1.76×,
js 1.59× over codex) where I deliberately stated a single-cause hypothesis to
the subagent and asked it to disprove it. All three were refuted, each
surfacing a different root cause. Useful as a counter to biased single-fix
thinking.

## Java — 2.06× codex on gson realworld

Hypothesis: planner picks `TypeAdapterFactory`, hierarchy returns huge subclass
packet, mini explores every subclass.

Verdict: **refuted**.
- Planner emitted only a truncated "s the file" query, never
  `TypeAdapterFactory`.
- Hierarchy appeared in 1 of 3 traces and returned modest 3–5 KB packets.
- Cost ratio actually closer to 2.8× on these traces.

Real driver: **structural verbosity in graph result packets**. One
26-result `decl_search` call returned 17,579 bytes (42% of the run's
total output). The equivalent grep scanned 684 KB of files but returned
only 1,590 bytes of paths — an 11× compression ratio.

The gap is the per-symbol packet wrapper (`spans`, `confidence`, `tool`)
plus the symbol body (`id`, `name`, `kind`, `path`, `signature`, `span`,
`confidence`). Every field is potentially load-bearing somewhere
downstream, so a blanket trim risks losing recall on different scenarios.

## Scala — 1.76× codex on akka mailbox audit

Hypothesis: graph packets bloated with sidecar fields (confidence, freshness,
attributes, spans) drives the gap.

Verdict: **refuted**.
- Across the 3 with-graph runs the subagent inspected,
  `definition_search` returned `graph_unavailable` with empty packets.
- The semantic graph never fired; `hierarchy` / `decl_search` never
  executed in those runs.

Real driver: **model bypasses graph and over-issues grep**. With-graph
runs had 18 grep calls at ~2,078 bytes/call; no-graph runs had 23 grep
calls at ~1,006 bytes/call. With-graph used uncapped grep + full file
reads (72.7 KB read_file bytes) while no-graph had byte caps. The
deep-chain finding flagged 23 consecutive grep / read_slice calls in one
turn — a runaway exploration the planner did not constrain.

Note: more recent scala mini runs (e.g.
`graph-vs-nograph-scala-realworld-with-graph-1780394389471`) show graph
*is* available (5 graph_available events, 27 definition_search calls).
The subagent's "graph never fired" finding is from earlier traces — the
recent traces still show grep-heavy exploration but with graph available
alongside. The over-grepping pattern persists; the graph-unavailable
state may be an indexer transient.

## JS — 1.59× codex on lodash fp wrapper alias resolution

Hypothesis: redundant tool calls chasing alias chains.

Verdict: **refuted**.
- Zero `reference_search` calls across all 3 traces (the very tool that
  would resolve chains).
- Run-to-run variance is the headline: Run 1 = 94 calls / 495 K input;
  Run 2 = 0 delegate calls / 87 K input; Run 3 = 185 calls / 841 K input.
  Run 3 was 8.6× Run 1.

Real driver: **Haiku delegate triggered a brute-force file enumeration**.
Run 3 logs read 113.5 KB across 2,365 enumerated files of which only 10
were actually touched. Delegate prompt: "Let me continue reading the
files in batches. I'll try to optimize by reading multiple files at once
and filtering as I go." The delegation heuristic spawns a subagent that
rediscovers task scope without inheriting constraint.

## Cross-cutting takeaways

Each LOSS cell has a distinct root cause; no single fix addresses all
three. The pattern across diagnoses:

1. **Single-hypothesis framing biases analysis.** Each adversarial check
   surfaced a different mechanism than the one I led with. Spawning the
   subagent with the explicit task of disproving the hypothesis was
   load-bearing.
2. **Mini's cost gap vs codex is not one problem.** Java is wire format;
   scala is over-grepping; js is delegation runaway. A universal trim
   helps everywhere but solves nothing fully.
3. **Run-to-run variance is large.** JS Run 3 was 8.6× Run 1 on the same
   scenario with the same binary. Median across n=3 still hides the
   outliers a sweep optimizer needs to see.

## What this rules out

- Per-language scenario fixes (the goal forbids this).
- A single universal trim being decisive (java packet bloat is real but
  ~22% wire reduction; not enough to flip 1.6×+ ratios).
- Sidecar-field removal from `evidence_packet` (medium-risk — 8+
  `assert_uniform_evidence_packet` test sites would need updating, and
  several non-symbol packet types still need `spans` as their only
  location signal).

## Open questions

- Is the scala "graph_unavailable" state a flaky indexer or a parser
  regression? Recent traces show it firing, but the previous 3 the
  subagent inspected did not.
- Should haiku's delegate-spawn heuristic be tuned to suppress
  brute-force read_file enumeration once a count threshold is hit?
- Is mini systematically ignoring the planner's recommendations on
  scala/php/csharp? Earlier evidence suggested yes; worth re-checking
  after #1 + #6.
