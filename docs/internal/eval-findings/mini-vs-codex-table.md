# Mini w/g vs Codex realworld scoreboard

Squeezy gpt-5.4-mini with-graph (w/g) and no-graph (n/g) vs Codex CLI baseline (also gpt-5.4-mini). Medians across 3 reps. Verdict: w/g WINS iff recall ≥ 95% AND cost ≤ 0.95× codex.

| Lang | sqz w/g recall | sqz w/g cost | codex cost | w/g vs codex | sqz n/g recall | sqz n/g cost | Verdict |
|------|--------------:|-------------:|-----------:|-------------:|--------------:|-------------:|:-------:|
| swift | 100.0% | $0.0167 | $0.0281 | 0.59× | 100.0% | $0.0188 | **WIN** |
| go | 100.0% | $0.0788 | $0.0250 | 3.15× | 100.0% | $0.0478 | LOSS |
| cpp | 100.0% | $0.0500 | $0.0541 | 0.92× | 100.0% | $0.0610 | **WIN** |
| csharp | 100.0% | $0.0719 | $0.0341 | 2.11× | 100.0% | $0.0629 | LOSS |
| java | 100.0% | $0.1026 | $0.0497 | 2.06× | 83.3% | $0.0564 | LOSS |
| js | 100.0% | $0.0337 | $0.0212 | 1.59× | 0.0% | $0.0000 | LOSS |
| python | 100.0% | $0.0286 | $0.0209 | 1.37× | 100.0% | $0.0157 | LOSS |
| ruby | 100.0% | $0.0500 | $0.0473 | 1.06× | 100.0% | $0.0746 | LOSS |
| php | 100.0% | $0.0424 | $0.0351 | 1.21× | 100.0% | $0.0429 | LOSS |
| kotlin | 100.0% | $0.0377 | $0.0248 | 1.52× | 100.0% | $0.0260 | LOSS |
| scala | 100.0% | $0.0329 | $0.0307 | 1.07× | 100.0% | $0.0495 | LOSS |
| dart | 100.0% | $0.1710 | $0.0233 | 7.34× | 94.4% | $0.1371 | LOSS |
| rust | 96.9% | $0.0309 | $0.0354 | 0.87× | 100.0% | $0.0159 | **WIN** |

**Tally:** 3 WIN / 0 TIE / 10 LOSS

**Note:** Most rows reflect PRE-session mini data (only go and csharp were re-swept after the cat-n + planner gate fixes). The remaining 11 langs need a fresh mini sweep to capture post-fix performance — the Haiku side showed big wins from those fixes, so the mini-vs-codex picture is expected to improve when re-swept.

**Mini regressions noticed:** go w/g jumped from $0.0475 (TIE pre-fix) to $0.0788 (LOSS) — cat-n line-number prefix may inflate mini's input tokens more than expected. Worth investigating before broader mini sweep.
