# Scala SemanticDB oracle

Reads `.semanticdb` protobufs emitted by `scalac -Xsemanticdb` and prints
the `(file, kind, name)` rows the squeezy bench oracle compares against the
tree-sitter extractor.

Status: skeleton. The Rust-side driver in
`benchmarks/squeezy-graph-bench/src/oracles/scala_semanticdb.rs` skips the
oracle until scalac + scala-cli invocation is wired through. Once that lands
the runner will:

1. `scalac -Xsemanticdb -semanticdb-target:<tmp> -d <tmp>/classes <sources>`
2. `scala-cli run scala-oracle.sc -- <tmp> <root>`
3. Parse the resulting JSON into a `SymbolScan` and compare against the
   squeezy graph.

See `docs/internal/lang-specs/scala.md` §9 for the full plan, including the
limitation list that maps Scala-specific oracle gaps (implicit conversions,
given/using resolution, macro-expanded members, anonymous classes, locals)
back to the gate report.
