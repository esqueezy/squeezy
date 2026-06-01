# Haiku w/g vs CC realworld scoreboard

Squeezy Haiku 4.5 with-graph (w/g) and no-graph (n/g) vs Claude Code `--bare` on the same Haiku tier. Medians across 3 reps. Verdict: w/g WINS iff recall ≥ 95% AND cost ≤ 0.95× CC.

| Lang | sqz w/g recall | sqz w/g cost | CC cost | w/g vs CC | sqz n/g recall | sqz n/g cost | Verdict |
|------|--------------:|-------------:|--------:|----------:|--------------:|-------------:|:-------:|
| swift | 100.0% | $0.0225 | $0.0337 | 0.67× | 100.0% | $0.0233 | **WIN** |
| go | 100.0% | $0.0181 | $0.1781 | 0.10× | 100.0% | $0.0828 | **WIN** |
| cpp | 100.0% | $0.2110 | $0.2382 | 0.89× | 100.0% | $0.1499 | **WIN** |
| csharp | 100.0% | $0.1300 | $0.1818 | 0.72× | 69.0% | $0.1605 | **WIN** |
| java | 100.0% | $0.1821 | $0.2441 | 0.75× | 77.8% | $0.1418 | **WIN** |
| js | 95.5% | $0.0280 | $0.0491 | 0.57× | 59.1% | $0.0231 | **WIN** |
| ts | 100.0% | $0.1868 | $0.1742 | 1.07× | 100.0% | $0.1960 | LOSS |
| python | 100.0% | $0.0455 | $0.0819 | 0.56× | 95.0% | $0.0309 | **WIN** |
| ruby | 100.0% | $0.1524 | $0.2426 | 0.63× | 100.0% | $0.0736 | **WIN** |
| php | 100.0% | $0.0949 | $0.1875 | 0.51× | 100.0% | $0.1646 | **WIN** |
| kotlin | 100.0% | $0.1722 | $0.1324 | 1.30× | 0.0% | $0.0000 | LOSS |
| scala | 100.0% | $0.2656 | $0.3541 | 0.75× | 100.0% | $0.1959 | **WIN** |
| dart | 63.9% | $0.1893 | $0.3029 | 0.63× | 77.8% | $0.2366 | LOSS |
| c | 100.0% | $0.0785 | $0.2288 | 0.34× | 100.0% | $0.2176 | **WIN** |
| rust | 100.0% | $0.1028 | $0.1734 | 0.59× | 100.0% | $0.1593 | **WIN** |

**Tally:** 12 WIN / 0 TIE / 3 LOSS over 15 langs.

**Remaining losses:** ts (cost), kotlin (cost), dart (recall).

## Session deltas
Pre-session: 8 WIN / 2 TIE / 5 LOSS. Post-session: 12 WIN / 0 TIE / 3 LOSS. Net +4 WINs.

**Flipped LOSS → WIN this session:** swift (cat-n line numbers fixed 79pp recall floor + universal gate dropped wasted planner round), csharp (universal gate + hierarchy intent on "concrete class"), php (hierarchy intent on "concrete class" replaced wasteful definition_search with hierarchy), java (cat-n recall improvement).

**Remaining losses notes:** ts (1.07× cost — gate fires but residual planner overhead); kotlin (1.30× — planner_hierarchy fires but cat-n inflated read_file payloads dominate); dart (recall stochastic 50-94% — planner_hierarchy not consistently firing on Flutter SDK, likely graph_indexing wait).
