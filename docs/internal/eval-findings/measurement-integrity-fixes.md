# Measurement-integrity fixes (graph-vs-nograph realworld A/B)

While pushing toward "15/15 vs both Codex and Claude Code", two measurement
bugs were found that had been producing **wrong** python/java/rust verdicts on
at least one tier. Both are now fixed. This documents what was wrong, the fix,
and the corrected same-task numbers — so the scoreboard is trustworthy.

## Bug 1 — stale Haiku tomls (python + java measured on a *different task*)

The Haiku-tier scenarios are generated copies under `/tmp/haiku-baseline-toml/`
(committed scenario with the `[squeezy]` provider/model swapped to Anthropic
Haiku). They were generated **May 31**. Commit #201 (Jun 2) then **revised the
python and java prompts**. Result: for python and java, Haiku ran the *old*
prompt while Mini ran the *new* committed prompt — the two tiers were scoring
**different tasks**.

- python Haiku ran the old "Session intra-class call-graph" task; python Mini
  ran the new "subclass-surface inventory" task.
- This is why python "scored 100 on Haiku" (old prompt agreed with the old
  grader) but "0 on Mini" (new prompt vs old grader).

Diff across all 15 langs × 2 variants showed **only python and java** drifted;
the other 13 have byte-identical prompts across tiers and are unaffected.

**Fix:** regenerated the python + java Haiku tomls from the current committed
scenarios (prompt now identical to Mini; only provider/model differ). The
stale originals are kept as `*.stale-bak`.

## Bug 2 — stale graders (wrong ground truth for python + rust)

The external correctness grader (`/tmp/codex-runs/realworld/grade.py`, ground
truth in `ground_truth.json`) had drifted from the committed prompts:

- **python** — `grade_python` graded the *old* call-graph task
  (`<method>: <callees>`), but the committed prompt asks for a subclass-surface
  inventory (`<path>::<Class>: <methods>`). Both models answered the prompt
  correctly and scored **0** against the wrong ground truth.
  **Fix:** recomputed ground truth from psf/requests @ `cd90742…` with `ast`
  (12 subclasses across 7 files), rewrote `grade_python` to parse
  `<path>::<C>: <methods>` and exact-set-match the method list.
- **rust** — the task answers against the **local working tree**
  (`workspace.local = "."`), so the `impl LlmProvider` line numbers drift
  between checkouts; the grader required a `line ±2` match and so failed
  correct answers (recall ~6%). **Fix:** recomputed the current 15 production
  `impl LlmProvider for X` sites and made `grade_rust` **drift-proof** — match
  each by its unique implementor TYPE name (the same presence approach
  `grade_java` already uses). `java`'s grader was already correct.

**Fairness check (critical):** a grader fix is only valid if it lifts *both*
sides, not just squeezy. After the fix, **codex** recall went 0→**100%**
(python) and ~6→**100%** (rust) — confirming the bug was bad ground truth, not
a model failure.

## Corrected same-task numbers (only same-task runs, recall ≥95, equal rates)

| lang | tier | squeezy | baseline | verdict |
|---|---|---|---|---|
| python | Mini | $0.1038 (r≈83–100) | codex $0.0288 | **LOSS — 3.6× pricier** |
| rust | Mini | $0.0422 (r≈100) | codex $0.0415 | TIE |
| java | Mini | $0.1056 (r≈94) | codex $0.1175 | **WIN 10%** |
| rust | Haiku | $0.1103 (r≈100) | CC $0.1734 | **WIN 36%** |
| python | Haiku | re-running (regenerated toml) | CC re-running (old-task) | pending |
| java | Haiku | re-running (regenerated toml) | CC $0.2222 | pending |

The other 13 languages were measured on consistent prompts and correct graders;
their verdicts stand.

## The python finding (a real product signal, not a measurement artifact)

After both fixes, python is a **genuine cost loss on Mini**: squeezy pays 3.6×
codex. Cause: #201 revised python from a graph-favorable call-graph task into a
**grep-friendly** "find every class whose direct base is in B" inventory — one
`grep 'class .*(…Base…)'` across `src/` + `tests/` enumerates the answer.
Codex greps it for ~$0.03; squeezy navigates it with the graph (hierarchy +
many `read_slice`) for ~$0.10. This is the "the graph tax needs a big enough
task to justify it" principle (cost-saving Ch.13 §13.4): squeezy should
recognize a grep-shaped scan and **use grep instead of the graph**, rather than
paying for cross-file navigation the task doesn't need. The fix is a squeezy
efficiency/routing improvement (option a), not a scenario change.
