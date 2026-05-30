use std::{path::Path, time::Instant};

use squeezy_core::Result;
use squeezy_graph::SemanticGraph;

use crate::{
    accuracy::compare_symbol_sets,
    oracles::common_scan::collect_squeezy_symbol_scan,
    report::{ScalaOracleReport, SymbolScan},
    util::command_exists,
};

/// Wrapper that lets the runner toggle the scala oracle without compiling
/// in scalac. When `scalac` and `scala-cli` are not on `$PATH`, the oracle
/// degrades to a scan-only comparison that still surfaces precision/recall
/// against an empty oracle set — letting fixture query gates keep running.
pub(crate) fn time_scala_oracle_optional(_root: &Path) -> (u128, String) {
    if !command_exists("scalac") {
        return (0, "skipped: scalac not found".to_string());
    }
    if !command_exists("scala-cli") {
        return (
            0,
            "skipped: scala-cli not found (SemanticDB reader unavailable)".to_string(),
        );
    }
    let started = Instant::now();
    // TODO(scala): wire scalac -Xsemanticdb invocation and a scala-cli helper
    // that walks the resulting .semanticdb protobufs into the SymbolScan
    // shape used by `compare_symbol_sets`. See
    // docs/internal/lang-specs/scala.md §9 for the full plan; until then
    // the oracle reports a placeholder status and the symbol comparison
    // proceeds against an empty oracle (precision=recall=0).
    let _ = started;
    (
        0,
        "skipped: SemanticDB scanner not yet implemented (see scala spec §9)".to_string(),
    )
}

pub(crate) fn collect_scala_oracle_accuracy(
    _root: &Path,
    graph: &SemanticGraph,
) -> Result<ScalaOracleReport> {
    let squeezy_symbols = collect_squeezy_symbol_scan(graph);
    let (_, status) = time_scala_oracle_optional(_root);
    Ok(ScalaOracleReport {
        oracle_ms: None,
        status,
        oracle_unparseable_files: 0,
        oracle_unparseable_examples: Vec::new(),
        symbols: compare_symbol_sets(&squeezy_symbols, &SymbolScan::default()),
        limitations: scala_oracle_limitations(),
    })
}

pub(crate) fn scala_oracle_limitations() -> Vec<String> {
    vec![
        "Scala oracle uses SemanticDB declarations; implicit-conversion injection at call sites, `given`/`using` resolution at call sites, and macro-expanded synthetic members are excluded from the symbol comparison.".to_string(),
        "Path-dependent type references (`a.B`) are emitted as references with no resolution edge; they are excluded from navigation accuracy.".to_string(),
        "Anonymous classes and lambda bodies are not compared; SemanticDB emits `<anon>` symbols that the tree-sitter extractor omits.".to_string(),
        "Local `val`/`var` (LOCAL kind in SemanticDB) are excluded — squeezy does not emit locals as symbols.".to_string(),
        "If `scalac` or `scala-cli` is unavailable, the oracle is skipped while fixture query gates still run.".to_string(),
    ]
}
