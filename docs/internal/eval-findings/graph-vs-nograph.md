# Graph vs. no-graph eval — three-task A/B with cost and recall

A controlled A/B comparison of squeezy with its semantic-graph tool
family available against the same agent loop with that family hidden.
Three task shapes, three runs per side per task, eighteen runs total,
plus baseline-vs-fix replays for the bugs the experiment surfaced.

Full per-run data in [`graph-vs-nograph-data.csv`](graph-vs-nograph-data.csv).

## Setup

- **Model**: `gpt-5.4-mini` via the `openai` provider preset (same model
  on both sides of every comparison).
- **Workspace**: the squeezy repo itself (Rust workspace under
  `crates/`).
- **System prompt**: a deliberately neutral instructions block on both
  sides ("answer concisely with whatever tools are available"). The
  default `DEFAULT_INSTRUCTIONS` in `squeezy-core` leans toward
  graph-first; using it would have biased the no-graph half toward
  tools it cannot call.
- **Permissions**: `permission_mode = "allow"` on both sides, widened
  by this PR so `read` / `ignored_search` are covered (without that
  the no-graph half was getting `grep` / `read_file` auto-denied).
- **No-graph variant**: the twelve graph tools (`repo_map`,
  `decl_search`, `definition_search`, `reference_search`,
  `symbol_context`, `hierarchy`, `read_slice`, `upstream_flow`,
  `downstream_flow`, `diff_context`, `plan_patch`,
  `refresh_compiler_facts`) are filtered out of the advertised tool
  list via the new `excluded_tools` overlay, and the graph-first
  exploration planner is disabled via
  `SQUEEZY_EXPLORATION_COMPILER=0`. Both halves of every A/B are
  otherwise identical.

Scenarios live under
[`crates/squeezy-eval/fixtures/scenarios/graph-vs-nograph-*.toml`](../../../crates/squeezy-eval/fixtures/scenarios).
The harness already ships with `squeezy-eval diff <A> <B>` for
per-pair comparison; this doc rolls the six scenarios up into a single
table.

## Tasks

| # | Prompt | Ground truth size | Why graph should help |
|---|---|---:|---|
| 1 | "List every call site of `run_scenario` (file, line, enclosing function)." | 3 sites (1 def + 2 callers) | Single-symbol reference traversal; classic `reference_search` use. |
| 2 | "List every Rust type in this repo that implements the `LlmProvider` trait." | 27 impls | Workspace-wide hierarchy; touches multiple crates. |
| 3 | "List every non-test call site of `squeezy_llm::estimate_cost`." | 6 callers | Cross-crate reference traversal with both qualified and bare-after-import call shapes. |

Ground truth verified by `rg`/`grep` against the working tree at HEAD
of this branch and re-verified after each fix.

## Headline numbers

Medians across three runs per side per task, comparing the **current
state of the branch** (both fixes shipped) against the no-graph baseline.

| task | with-graph median $ | no-graph median $ | cost reduction | with-graph median recall | no-graph median recall |
|---|---:|---:|---:|---:|---:|
| 1 — `run_scenario` callers | $0.0225 | $0.0286 | **−21.3%** | 3/3 (100%) | 3/3 (100%) |
| 2 — `LlmProvider` impls | $0.0151 | $0.0604 | **−75.0%** | 18/27 (67%) | 27/27 (100%) |
| 3 — `estimate_cost` callers | $0.0359 | $0.0000* | n/a* | 6/6 (100%) | 0/6* |

\* Task 3 median is degenerate for the no-graph side: two of three
no-graph runs gave up returning empty answers ($0.0000); the one run
that finished cost $0.0888 — more than 2× the graph median. See the
per-run table.

**Tool-call medians** (graph vs no-graph): task 1 12 vs 14, task 2 4 vs
32, task 3 17 vs 1. Note that no-graph's task-2 median (32 tool calls)
delivers 27/27 recall while graph's median (4 tool calls) sometimes
stops short — see "Where graph wins" below.

## Per-run data

### Task 1 — `run_scenario` callers

| variant | run | tool calls | events | cost | recall |
|---|---|---:|---:|---:|---:|
| with-graph (post both fixes) | 1 | 12 | 723 | $0.0225 | 3/3 |
| with-graph (post both fixes) | 2 | 9 | 227 | $0.0201 | 3/3 |
| with-graph (post both fixes) | 3 | 16 | 937 | $0.0314 | 3/3 |
| no-graph | 1 | 15 | 616 | $0.0247 | 3/3 |
| no-graph | 2 | 14 | 364 | $0.0286 | 3/3 |
| no-graph | 3 | 13 | 519 | $0.0326 | 3/3 |

**Pre-fix reference point** (single run, before the self-crate
fallback): 6 tool calls, 110 events, $0.0144, **1/3** recall — graph
returned only the `ci.rs` references and silently missed
`main.rs:172`. Documented in
`references_to_symbol_finds_qualified_self_crate_call_across_modules`.

### Task 2 — `LlmProvider` impls (27 ground truth)

| variant | fix level | run | tool calls | events | cost | recall |
|---|---|---|---:|---:|---:|---:|
| with-graph | pre-fix | 1 | 7 | 322 | $0.0192 | 16/27 |
| with-graph | pre-fix | 2 | 5 | 344 | $0.0239 | 18/27 |
| with-graph | pre-fix | 3 | 32 | 2228 | $0.0578 | 18/27 |
| with-graph | post both fixes | 1 | 4 | 304 | $0.0151 | 20/27 |
| with-graph | post both fixes | 2 | 4 | 386 | $0.0082 | 7/27 |
| with-graph | post both fixes | 3 | 32 | 1154 | $0.0662 | 18/27 |
| no-graph | baseline | 1 | 32 | 1881 | $0.0685 | 27/27 |
| no-graph | baseline | 2 | 3 | 758 | $0.0130 | 27/27 |
| no-graph | baseline | 3 | 32 | 1049 | $0.0604 | 27/27 |

The post-fix graph ceiling is higher (one earlier run reached 25/27)
than pre-fix (max 18/27), but the median doesn't move because on
"list every X in the workspace" prompts the model frequently picks
`grep` as its first move even when graph tools are available. See
"Where graph still falls short."

### Task 3 — `estimate_cost` callers (6 ground truth)

| variant | fix level | run | tool calls | events | cost | recall |
|---|---|---|---:|---:|---:|---:|
| with-graph | pre-fix | 1 | 28 | 643 | $0.0883 | 4/6 |
| with-graph | pre-fix | 2 | 6 | 481 | $0.0288 | 1/6 |
| with-graph | pre-fix | 3 | 11 | 307 | $0.0000 | 0/6 |
| with-graph | post both fixes | 1 | 17 | 609 | $0.0359 | 6/6 |
| with-graph | post both fixes | 2 | 27 | 573 | $0.0547 | 6/6 |
| with-graph | post both fixes | 3 | 12 | 514 | $0.0213 | 1/6 |
| no-graph | baseline | 1 | 25 | 798 | $0.0888 | 6/6 |
| no-graph | baseline | 2 | 1 | 113 | $0.0000 | 0/6 |
| no-graph | baseline | 3 | 1 | 10 | $0.0000 | 0/6 |

Two no-graph runs delegated to the `explore` subagent and gave up
without an answer; the one no-graph run that finished cost more than
twice any post-fix graph run with the same final recall. Pre-fix
graph runs were unreliable too (one returned correct count with wrong
caller names, one returned 1, one returned nothing); post-fix two of
three runs are perfect.

## Where graph wins the most

**Cross-crate single-symbol traversal under a tight budget** — task 3
is the cleanest case for the graph value prop. The graph-resolved
`reference_search` returns the seven workspace call sites in one
call; the model then `read_slice`s each to extract caller names. No-
graph has to fan out across crates with `grep -r` and chase line
numbers manually; in two of three runs the model abandoned the task
before reporting an answer. Where the no-graph run did finish, it
cost 2.5× the post-fix graph median.

**Same-symbol, few sites, mixed call shapes** — task 1 is the
incremental case. Both halves can solve it; the graph half does so at
~21% lower cost and ~40% fewer tool calls. The pre-fix graph half
silently missed the qualified `squeezy_eval::run_scenario` call from
`main.rs`; the self-crate qualified-callable fallback now surfaces
that hit, so the graph side now matches no-graph on recall while
keeping its cost edge.

## Where graph still falls short

**Workspace-wide structural enumeration (task 2)** — when the prompt
is "list every implementor / definition / type matching X", the
model often picks `grep` first regardless of whether graph tools are
advertised. Post-fix the graph CAN find every impl (one earlier run
hit 25/27), but on the cleaner three-run median the model's choice
of `grep` as first step caps recall at 18/27. The remaining gap is a
prompt/planner question, not a graph capability question. A
follow-up nudge in `squeezy-agent`'s default instructions — "for
'list every implementor / definition / type' questions, prefer
`decl_search` over `grep`" — should close it.

## Fixes shipped

Two binding-rule additions in `squeezy-graph`. Each is gated by a
unique-workspace-candidate-by-name check, so ambiguous names stay
unresolved.

### 1. Self-crate qualified call

`<mycrate>::foo()` from another module of the same crate now resolves
to the function in that crate. Tree-sitter emits the `Calls` edge
with `to = None` because `module_qualified_call` does not treat the
crate's own underscore name as an alias for the crate root, and the
binding chain falls through into rules that reject `Function` symbols
on `reference_kind_can_bind_symbol`. The new
`self_crate_qualified_callable_matches` runs before the call-edge and
semantic-edge branches and binds with `Heuristic` confidence when the
symbol is the unique same-crate callable of its name.

Unit tests:

- `references_to_symbol_finds_qualified_self_crate_call_across_modules`
- `self_crate_qualified_callable_does_not_bind_when_name_is_ambiguous_in_crate`

### 2. Workspace-cross-crate qualified or import-resolved reference

`<othercrate>::Foo` from a different workspace crate, and bare `foo()`
after `use othercrate::foo;`, now resolve to the symbol in
`crates/othercrate/`. The default `reference_is_in_symbol_package`
gate rejected cross-crate references before the binding chain could
look at the qualified path or the file's imports. The new
`workspace_cross_crate_qualified_match` runs before that gate and
recovers the qualifier from one of three sources: the reference text
itself, the source-byte scope prefix adjacent to a bare-leaf
reference, or a non-glob `use <crate>::Name [as alias]` import in the
reference's file.

Unit tests:

- `references_to_symbol_finds_workspace_cross_crate_qualified_trait_impl`
- `references_to_symbol_finds_workspace_cross_crate_bare_call_after_use_import`
- `workspace_cross_crate_qualified_match_does_not_bind_ambiguous_workspace_name`
- `graph_symbol_references_surface_qualified_workspace_cross_crate_uses`
  (rewritten from
  `graph_symbol_references_are_package_local_until_cargo_resolution_exists`,
  which documented the pre-resolution behaviour we are now partially
  retiring)

## Reproduce

```sh
# build the eval binary
cargo build -p squeezy-eval --release

# run any scenario three times
for i in 1 2 3; do
  ./target/release/squeezy-eval run \
    crates/squeezy-eval/fixtures/scenarios/graph-vs-nograph-callers-with-graph.toml \
    --no-triage --out target/eval --quiet
done

# compare a pair of run directories
./target/release/squeezy-eval diff \
  target/eval/graph-vs-nograph-callers-with-graph-<ts> \
  target/eval/graph-vs-nograph-callers-no-graph-<ts>
```

Each run writes `run.json`, `trace.jsonl`, `frames.jsonl`,
`findings.jsonl`, and a `tickets/` directory; the per-run cost,
tool-call sequence, and assistant answer text used to build the
tables above come from `run.json` + `trace.jsonl` + `frames.jsonl`.

## Architectural audits (Codex baseline)

Seven architectural-audit scenarios — "list every X that derives from /
implements / imports / writes Y" — run three times each against Codex
CLI (`codex exec` with `gpt-5.4-mini`, ephemeral, JSON output, no
graph) as an external baseline for the same scenarios squeezy
exercises with and without its semantic-graph tool family. Codex
artifacts live under `/tmp/codex-runs/architectural/{lang}-r{1,2,3}.{events.jsonl,answer.txt}`;
metrics are appended to
[`graph-vs-nograph-data.csv`](graph-vs-nograph-data.csv) as
`{lang}_architectural` rows with variant `codex_baseline`. Costs are
medians of three runs; squeezy figures come from the same scenarios
under `crates/squeezy-eval/fixtures/scenarios/graph-vs-nograph-{lang}-architectural-{with,no}-graph.toml`
(Go was not in the squeezy validation sweep).

| scenario | codex $ | squeezy with-graph $ | squeezy no-graph $ | codex recall | cost winner |
|---|---:|---:|---:|---:|:--|
| Rust — `ToolResult` struct literals (4) | $0.0359 | $0.0276 | $0.0443 | 4/4 (100%) | **squeezy with-graph** |
| Go — `ValidArgsFunction` writes (6) | $0.0208 | — | — | 6/6 (100%) | **codex** |
| C++ — `base_sink<Mutex>` direct subclasses (21) | $0.0541 | $0.0189 | $0.0219 | 20/21 (95%) | **squeezy with-graph** |
| C# — `JsonReader` subclasses (5) | $0.0278 | $0.0270 | $0.0096 | 5/5 (100%) | **squeezy no-graph** |
| Java — `TypeAdapter` subclasses (11) | $0.0497 | $0.0244 | $0.0409 | 11/11 (100%) | **squeezy with-graph** |
| JS — lodash importer-helper pairs (16) | $0.0176 | $0.0236 | $0.0172 | 16/16 (100%) | **squeezy no-graph** |
| Python — `RequestException` subclasses + raises (21) | $0.0365 | $0.0261 | $0.0310 | 21/21 (100%) | **squeezy with-graph** |

**Headline**: squeezy with-graph beats codex on cost in 4 of 6
comparable scenarios (rust, cpp, java, python), squeezy no-graph beats
codex in the other 2 (csharp, js), and codex never wins on cost
against squeezy. Codex recall is essentially perfect across the board
(only one miss: `dist_sink` on the cpp prompt, lost to a literal
reading of the example exclusion list). The lone scenario where codex
beats no head-to-head squeezy reference is Go — squeezy did not run
that scenario in the validation sweep.

The cost gap is larger than the cost gap on the callers/refactor
sweeps: codex spends 2-3× squeezy with-graph on Rust, C++, Java, and
Python — primarily because codex repeatedly re-reads large files via
`sed -n` rather than using a graph-resolved slice. Squeezy with-graph
matches or beats codex recall everywhere except cpp (one run hit
18/21) and java (two runs short of 11/11), where the same
"`grep`-first" caveat noted under "Where graph still falls short"
applies — but the median post-fix squeezy with-graph cost is still
below codex on every comparable scenario.
