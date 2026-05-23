pub(crate) fn collect_accuracy(root: &Path, graph: &SemanticGraph, ra_lsp_probes: usize) -> AccuracyReport {
    let squeezy_symbols = collect_squeezy_symbol_scan(graph);
    let started = Instant::now();
    let (rust_analyzer_symbols, status) = collect_rust_analyzer_symbol_scan(graph);
    let rust_analyzer_symbols_ms = if status.starts_with("rust-analyzer symbols succeeded") {
        Some(started.elapsed().as_millis())
    } else {
        None
    };
    let symbols = compare_symbol_sets(&squeezy_symbols, &rust_analyzer_symbols);
    let navigation = collect_navigation_accuracy(root, graph, ra_lsp_probes);

    AccuracyReport {
        rust_analyzer_symbols_ms,
        rust_analyzer_symbol_status: status,
        symbols,
        navigation,
        limitations: vec![
            "Symbol TP/FP/FN compares declaration families both engines expose; raw rust-analyzer locals and fields are counted as excluded, not silently compared.".to_string(),
            "Navigation TP/FP/FN is sampled through rust-analyzer LSP definition/reference requests; it is a realistic loss tracker, not an exhaustive proof.".to_string(),
            "Macro-generated items, proc macros, cfg matrices, trait dispatch, deref/autoref method resolution, and external crate/stdlib references remain documented lower-confidence areas.".to_string(),
        ],
    }
}

pub(crate) fn empty_accuracy(status: &str) -> AccuracyReport {
    AccuracyReport {
        rust_analyzer_symbols_ms: None,
        rust_analyzer_symbol_status: status.to_string(),
        symbols: compare_symbol_sets(&SymbolScan::default(), &SymbolScan::default()),
        navigation: NavigationAccuracyReport {
            rust_analyzer_lsp_ms: None,
            rust_analyzer_lsp_status: status.to_string(),
            requested_probe_limit: 0,
            definitions: DefinitionAccuracyReport::default(),
            references: ReferenceAccuracyReport::default(),
            limitations: vec![status.to_string()],
        },
        limitations: vec![status.to_string()],
    }
}

pub(crate) fn collect_c_family_accuracy(
    root: &Path,
    graph: &SemanticGraph,
    language: BenchmarkLanguage,
    oracle_file_limit: usize,
) -> Result<AccuracyReport> {
    let language_kind = language.source_language();
    let started = Instant::now();
    let oracle = collect_clang_ast_symbol_scan(root, language_kind, oracle_file_limit)?;
    let oracle_ms = started.elapsed().as_millis();
    let excluded_files = oracle
        .excluded_files
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let squeezy_symbols =
        collect_c_family_squeezy_symbol_scan(graph, language_kind, &excluded_files);
    let symbols = compare_symbol_sets(&squeezy_symbols, &oracle.symbols);
    let skipped = oracle.unparseable_files.len();
    let status = if skipped == 0 {
        format!(
            "clang AST JSON oracle succeeded for {}/{} selected {} files ({} candidates)",
            oracle.parsed_files,
            oracle.selected_files,
            language.as_str(),
            oracle.candidate_files
        )
    } else {
        format!(
            "clang AST JSON oracle parsed {}/{} selected {} files ({} candidates); skipped {} unparseable files excluded from Squeezy FP accounting: {}",
            oracle.parsed_files,
            oracle.selected_files,
            language.as_str(),
            oracle.candidate_files,
            skipped,
            oracle
                .unparseable_files
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    Ok(AccuracyReport {
        rust_analyzer_symbols_ms: Some(oracle_ms),
        rust_analyzer_symbol_status: status.clone(),
        symbols,
        navigation: NavigationAccuracyReport {
            rust_analyzer_lsp_ms: None,
            rust_analyzer_lsp_status: "clang AST navigation oracle not implemented".to_string(),
            requested_probe_limit: 0,
            definitions: DefinitionAccuracyReport::default(),
            references: ReferenceAccuracyReport::default(),
            limitations: vec![
                "C/C++ navigation FP/FN is not sampled against clangd yet; this benchmark currently compares declaration symbols against clang AST JSON.".to_string(),
            ],
        },
        limitations: vec![
            "C/C++ symbol TP/FP/FN compares file/name/kind declaration families exposed by Squeezy and clang AST JSON.".to_string(),
            "Headers and source files are parsed as standalone translation units with conservative include-dir heuristics; files requiring project-specific generated headers, compile flags, or compile_commands are reported as unparseable and excluded from Squeezy FP accounting.".to_string(),
            format!(
                "Oracle file limit is {}; use --oracle-files 0 for exhaustive local runs.",
                if oracle_file_limit == 0 {
                    "unlimited".to_string()
                } else {
                    oracle_file_limit.to_string()
                }
            ),
        ],
    })
}

pub(crate) fn collect_python_oracle_accuracy(
    root: &Path,
    graph: &SemanticGraph,
) -> Result<PythonOracleReport> {
    let started = Instant::now();
    let oracle = collect_python_ast_symbol_scan(root)?;
    let oracle_ms = started.elapsed().as_millis();
    let unparseable_files = oracle
        .unparseable_files
        .into_iter()
        .collect::<BTreeSet<_>>();
    let squeezy_symbols = collect_squeezy_symbol_scan_excluding_files(graph, &unparseable_files);
    let symbols = compare_symbol_sets(&squeezy_symbols, &oracle.symbols);
    let oracle_unparseable_examples = unparseable_files
        .iter()
        .take(10)
        .cloned()
        .collect::<Vec<_>>();
    let oracle_unparseable_files = unparseable_files.len();

    Ok(PythonOracleReport {
        oracle_ms,
        status: if oracle_unparseable_files == 0 {
            "CPython ast oracle succeeded".to_string()
        } else {
            format!(
                "CPython ast oracle succeeded with {oracle_unparseable_files} unparseable files excluded from symbol FP accounting"
            )
        },
        oracle_unparseable_files,
        oracle_unparseable_examples,
        symbols,
        limitations: vec![
            "The Python oracle uses CPython ast for declarations and does not execute imports, infer dynamic attributes, or model metaclass-generated members.".to_string(),
            "Symbol comparison is file/name/kind based so it tracks declaration loss without pretending to prove runtime dispatch.".to_string(),
            "Python files that CPython ast cannot parse are reported as oracle_unparseable and excluded from Squeezy false-positive accounting; tree-sitter recovery remains useful for production editing workflows.".to_string(),
        ],
    })
}

#[derive(Debug, Deserialize)]
pub(crate) struct JsTsOracleSymbol {
    file: String,
    kind: String,
    name: String,
}

pub(crate) fn collect_js_ts_oracle_accuracy(root: &Path, graph: &SemanticGraph) -> JsTsOracleReport {
    let started = Instant::now();
    match collect_js_ts_symbol_scan(root) {
        Ok(oracle) => JsTsOracleReport {
            oracle_ms: started.elapsed().as_millis(),
            status: "TypeScript compiler API symbol oracle succeeded".to_string(),
            symbols: compare_symbol_sets(&collect_squeezy_symbol_scan(graph), &oracle),
            limitations: js_ts_oracle_limitations(),
        },
        Err(err) => JsTsOracleReport {
            oracle_ms: started.elapsed().as_millis(),
            status: format!("TypeScript compiler API oracle unavailable: {err}"),
            symbols: compare_symbol_sets(&SymbolScan::default(), &SymbolScan::default()),
            limitations: js_ts_oracle_limitations(),
        },
    }
}

pub(crate) fn collect_js_ts_symbol_scan(root: &Path) -> Result<SymbolScan> {
    let script = r#"
const fs = require("fs");
const path = require("path");
const ts = require(process.env.SQUEEZY_TYPESCRIPT_PATH || "typescript");
const root = process.argv[1];
const out = [];
const GENERATED_MARKERS = [
  "@generated",
  "auto-generated",
  "automatically generated",
  "code generated",
  "do not edit",
];
const GENERATED_PREFIX_BYTES = 4096;
const SKIP_DIR_NAMES = new Set([
  ".git",
  "node_modules",
  "dist",
  "build",
  "coverage",
  "out",
  "vendor",
  "third_party",
]);
function walk(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (SKIP_DIR_NAMES.has(entry.name)) continue;
    if (entry.name.startsWith(".") && entry.name !== "." && entry.name !== "..") continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      walk(full);
    } else if (/\.[cm]?[jt]sx?$/.test(entry.name)) {
      scan(full);
    }
  }
}
function rel(file) { return path.relative(root, file).split(path.sep).join("/"); }
function emit(file, kind, name) {
  if (name && /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name)) out.push({ file: rel(file), kind, name });
}
// Tracks loop/catch variable declaration list nodes so locals introduced by
// `for (const x of ...)`, `for (let x = ...; ...; ...)` and `catch (e)` are not
// counted against Squeezy's declaration set. Squeezy's graph anchors these on
// dedicated AST nodes and only synthesizes a binding symbol when the binding
// is a simple identifier, so this matches the local heuristic on both sides.
function isLoopOrCatchLocal(node) {
  const parent = node.parent;
  if (!parent) return false;
  if (ts.isCatchClause(parent)) return true;
  if (parent.kind === ts.SyntaxKind.VariableDeclarationList) {
    const grand = parent.parent;
    if (grand && (
      ts.isForInStatement(grand) ||
      ts.isForOfStatement(grand) ||
      ts.isForStatement(grand)
    )) {
      // For-statement initializer can still be a top-level lexical declaration,
      // but `for (let i = 0; ...)` is a loop local that the oracle should skip
      // because Squeezy does not promote it to a graph symbol either.
      return true;
    }
  }
  return false;
}
function scan(file) {
  const source = fs.readFileSync(file, "utf8");
  const head = source.slice(0, GENERATED_PREFIX_BYTES).toLowerCase();
  if (GENERATED_MARKERS.some((marker) => head.includes(marker))) return;
  const sf = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, file.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  function visit(node) {
    if ((ts.isFunctionDeclaration(node) || ts.isFunctionExpression(node)) && node.name) emit(file, "Function", node.name.text);
    else if (ts.isClassDeclaration(node) && node.name) emit(file, "Class", node.name.text);
    else if (ts.isInterfaceDeclaration(node)) emit(file, "Interface", node.name.text);
    else if (ts.isModuleDeclaration(node) && ts.isIdentifier(node.name)) emit(file, "Module", node.name.text);
    else if (ts.isTypeAliasDeclaration(node)) emit(file, "TypeAlias", node.name.text);
    else if (ts.isEnumDeclaration(node)) emit(file, "Enum", node.name.text);
    else if ((ts.isMethodDeclaration(node) || ts.isMethodSignature(node)) && node.name && ts.isIdentifier(node.name)) emit(file, "Method", node.name.text);
    else if (ts.isPropertyDeclaration(node) && node.name && ts.isIdentifier(node.name)) {
      const init = node.initializer;
      if (init && (ts.isArrowFunction(init) || ts.isFunctionExpression(init))) emit(file, "Method", node.name.text);
    }
    else if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && !isLoopOrCatchLocal(node)) {
      const init = node.initializer;
      emit(file, init && (ts.isArrowFunction(init) || ts.isFunctionExpression(init)) ? "Function" : "Const", node.name.text);
    }
    ts.forEachChild(node, visit);
  }
  visit(sf);
}
walk(root);
console.log(JSON.stringify(out));
"#;
    let output = Command::new("node")
        .arg("-e")
        .arg(script)
        .arg(root)
        .output()
        .map_err(|err| SqueezyError::Graph(format!("node unavailable: {err}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = if stderr.contains("Cannot find module 'typescript'") {
            "node package 'typescript' is not installed".to_string()
        } else {
            stderr
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("node TypeScript oracle failed")
                .trim()
                .to_string()
        };
        return Err(SqueezyError::Graph(message));
    }
    let symbols: Vec<JsTsOracleSymbol> = serde_json::from_slice(&output.stdout)
        .map_err(|err| SqueezyError::Graph(format!("invalid JS/TS oracle JSON: {err}")))?;
    let mut scan = SymbolScan::default();
    for symbol in symbols {
        scan.raw_total += 1;
        increment_symbol(
            &mut scan.counts,
            SymbolKey {
                file: symbol.file,
                kind: symbol.kind,
                name: normalize_symbol_name(&symbol.name),
            },
        );
    }
    Ok(scan)
}

pub(crate) fn time_js_ts_oracle(fixture: &Path) -> Result<u128> {
    let started = Instant::now();
    let _ = collect_js_ts_symbol_scan(fixture)?;
    Ok(started.elapsed().as_millis())
}

pub(crate) fn js_ts_oracle_limitations() -> Vec<String> {
    vec![
        "The JS/TS oracle uses the TypeScript compiler API only in benchmark tooling; production navigation remains tree-sitter-only.".to_string(),
        "Symbol comparison is file/name/kind based and does not prove dynamic JavaScript dispatch, bundler aliases, or runtime module loading.".to_string(),
        "When node or the typescript package is unavailable, benchmark reports keep the oracle status explicit instead of blocking production parser tests.".to_string(),
    ]
}

pub(crate) fn collect_go_oracle_accuracy(root: &Path, graph: &SemanticGraph) -> Result<GoOracleReport> {
    let started = Instant::now();
    let oracle = collect_go_ast_symbol_scan(root)?;
    let oracle_ms = started.elapsed().as_millis();
    let unparseable_files = oracle
        .unparseable_files
        .into_iter()
        .collect::<BTreeSet<_>>();
    let squeezy_symbols = collect_squeezy_symbol_scan_excluding_files(graph, &unparseable_files);
    let symbols = compare_symbol_sets(&squeezy_symbols, &oracle.symbols);
    let oracle_unparseable_examples = unparseable_files
        .iter()
        .take(10)
        .cloned()
        .collect::<Vec<_>>();
    let oracle_unparseable_files = unparseable_files.len();

    Ok(GoOracleReport {
        oracle_ms,
        status: if oracle_unparseable_files == 0 {
            "Go AST oracle succeeded".to_string()
        } else {
            format!(
                "Go AST oracle succeeded with {oracle_unparseable_files} unparseable files excluded from symbol FP accounting"
            )
        },
        oracle_unparseable_files,
        oracle_unparseable_examples,
        symbols,
        limitations: vec![
            "The Go oracle uses the Go parser/AST for declaration discovery and does not execute package code.".to_string(),
            "Symbol comparison is file/name/kind based; receiver dispatch, interface satisfaction, build tags, generated files, and external modules remain heuristic or excluded.".to_string(),
            "Heuristic changes should be accepted by FP/FN deltas on smoke plus external corpora, with rejected broad matches documented in the report.".to_string(),
        ],
    })
}

pub(crate) fn heuristic_iteration_reports(
    language: BenchmarkLanguage,
    go_oracle: &Option<GoOracleReport>,
) -> Vec<HeuristicIterationReport> {
    if language != BenchmarkLanguage::Go {
        return Vec::new();
    }
    if go_oracle.is_none() {
        return Vec::new();
    }
    vec![
        HeuristicIterationReport {
            name: "baseline-tree-sitter".to_string(),
            status: "accepted".to_string(),
            notes: vec![
                "Package/import/declaration extraction is the baseline for Go heuristic comparisons.".to_string(),
            ],
        },
        HeuristicIterationReport {
            name: "top-level-declaration-scope".to_string(),
            status: "accepted".to_string(),
            notes: vec![
                "Function-local var/const/type declarations, blank identifiers, and declarations inside top-level function literals are excluded from top-level symbol accuracy.".to_string(),
            ],
        },
        HeuristicIterationReport {
            name: "go-alias-and-declaration-lists".to_string(),
            status: "accepted".to_string(),
            notes: vec![
                "Grouped var/const specs and tree-sitter-go type_alias nodes are expanded so multi-name declarations and aliases count as symbols.".to_string(),
            ],
        },
        HeuristicIterationReport {
            name: "go-test-method-normalization".to_string(),
            status: "accepted".to_string(),
            notes: vec![
                "Suite-style _test.go methods with Test/Benchmark/Fuzz names are normalized to test functions for oracle comparison.".to_string(),
            ],
        },
        HeuristicIterationReport {
            name: "go-external-package-examples".to_string(),
            status: "targeted-next".to_string(),
            notes: vec![
                "Remaining etcd FNs are concentrated in external-package example test files; keep them visible instead of broad lexical matching.".to_string(),
            ],
        },
        HeuristicIterationReport {
            name: "go-lazy-reference-materialization".to_string(),
            status: "targeted-next".to_string(),
            notes: vec![
                "Prometheus and etcd are slower than the declaration-only Go oracle because cold build materializes references, body hits, calls, and edges eagerly.".to_string(),
            ],
        },
        HeuristicIterationReport {
            name: "broad-lexical-reference-binding".to_string(),
            status: "rejected-default".to_string(),
            notes: vec![
                "Broad same-name binding is not enabled by default; Go navigation favors exact package/import/receiver evidence before recall-only expansion.".to_string(),
            ],
        },
    ]
}

pub(crate) fn collect_squeezy_symbol_scan(graph: &SemanticGraph) -> SymbolScan {
    collect_squeezy_symbol_scan_excluding_files(graph, &BTreeSet::new())
}

pub(crate) fn collect_squeezy_symbol_scan_excluding_files(
    graph: &SemanticGraph,
    excluded_files: &BTreeSet<String>,
) -> SymbolScan {
    let mut scan = SymbolScan::default();
    for symbol in graph.symbols.values() {
        scan.raw_total += 1;
        match normalize_squeezy_kind(symbol.kind) {
            Some(kind) => {
                let Some(file) = graph.files.get(&symbol.file_id) else {
                    increment(&mut scan.excluded_by_kind, "MissingFile");
                    continue;
                };
                if excluded_files.contains(&file.relative_path) {
                    increment(&mut scan.excluded_by_kind, "OracleUnparseableFile");
                    continue;
                }
                increment_symbol(
                    &mut scan.counts,
                    SymbolKey {
                        file: file.relative_path.clone(),
                        kind,
                        name: normalize_symbol_name(&symbol.name),
                    },
                );
            }
            None => increment(&mut scan.excluded_by_kind, &format!("{:?}", symbol.kind)),
        }
    }
    scan
}

pub(crate) fn collect_c_family_squeezy_symbol_scan(
    graph: &SemanticGraph,
    language: LanguageKind,
    excluded_files: &BTreeSet<String>,
) -> SymbolScan {
    let mut scan = SymbolScan::default();
    for symbol in graph.symbols.values() {
        let Some(file) = graph.files.get(&symbol.file_id) else {
            increment(&mut scan.excluded_by_kind, "MissingFile");
            continue;
        };
        if file.language != language {
            continue;
        }
        scan.raw_total += 1;
        if excluded_files.contains(&file.relative_path) {
            increment(&mut scan.excluded_by_kind, "OracleUnparseableFile");
            continue;
        }
        if !clang_symbol_name_is_comparable(&symbol.name) {
            increment(&mut scan.excluded_by_kind, "UnnamedOrOperator");
            continue;
        }
        // Clang's AST oracle emits `ClassTemplateSpecializationDecl` (not
        // `CXXRecordDecl`) for `template<> class Foo<int> {}` style
        // declarations, and our `clang_symbol_kind` mapping intentionally
        // skips that kind. Squeezy treats the same node as a Class symbol
        // tagged with `c++:template-specialization`; counting it here would
        // appear as a false positive against the oracle. Exclude these
        // symbols symmetrically.
        if symbol
            .attributes
            .iter()
            .any(|attribute| attribute == "c++:template-specialization")
        {
            increment(&mut scan.excluded_by_kind, "TemplateSpecialization");
            continue;
        }
        match normalize_c_family_squeezy_kind(symbol.kind) {
            Some(kind) => {
                increment_unique_symbol(
                    &mut scan.counts,
                    SymbolKey {
                        file: file.relative_path.clone(),
                        kind,
                        name: normalize_symbol_name(&symbol.name),
                    },
                );
            }
            None => increment(&mut scan.excluded_by_kind, &format!("{:?}", symbol.kind)),
        }
    }
    scan
}

#[derive(Debug, Deserialize)]
pub(crate) struct PythonAstOracleOutput {
    rows: Vec<[String; 3]>,
    unparseable_files: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct PythonAstSymbolScan {
    symbols: SymbolScan,
    unparseable_files: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct CFamilyClangSymbolScan {
    symbols: SymbolScan,
    parsed_files: usize,
    candidate_files: usize,
    selected_files: usize,
    unparseable_files: Vec<String>,
    excluded_files: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GoAstOracleOutput {
    #[serde(default, deserialize_with = "null_default")]
    rows: Vec<[String; 3]>,
    #[serde(default, deserialize_with = "null_default")]
    unparseable_files: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct GoAstSymbolScan {
    symbols: SymbolScan,
    unparseable_files: Vec<String>,
}

pub(crate) fn null_default<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

pub(crate) fn collect_python_ast_symbol_scan(root: &Path) -> Result<PythonAstSymbolScan> {
    let output = Command::new("python3")
        .arg("-c")
        .arg(PYTHON_AST_ORACLE)
        .arg(root)
        .output()
        .map_err(|err| SqueezyError::Graph(format!("failed to run Python AST oracle: {err}")))?;
    if !output.status.success() {
        return Err(SqueezyError::Graph(format!(
            "Python AST oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let output: PythonAstOracleOutput = serde_json::from_slice(&output.stdout)
        .map_err(|err| SqueezyError::Graph(format!("invalid Python AST oracle JSON: {err}")))?;
    let mut scan = SymbolScan::default();
    for [file, kind, name] in output.rows {
        scan.raw_total += 1;
        increment_symbol(
            &mut scan.counts,
            SymbolKey {
                file,
                kind,
                name: normalize_symbol_name(&name),
            },
        );
    }
    Ok(PythonAstSymbolScan {
        symbols: scan,
        unparseable_files: output.unparseable_files,
    })
}

pub(crate) fn collect_csharp_oracle_accuracy(
    root: &Path,
    graph: &SemanticGraph,
) -> Result<CsharpOracleReport> {
    let started = Instant::now();
    let oracle = collect_csharp_oracle_symbol_scan(root)?;
    let oracle_ms = started.elapsed().as_millis();
    let unparseable_files = oracle
        .unparseable_files
        .into_iter()
        .collect::<BTreeSet<_>>();
    let squeezy_symbols = collect_squeezy_symbol_scan_excluding_files(graph, &unparseable_files);
    let symbols = compare_symbol_sets(&squeezy_symbols, &oracle.symbols);
    let oracle_unparseable_examples = unparseable_files
        .iter()
        .take(10)
        .cloned()
        .collect::<Vec<_>>();
    let oracle_unparseable_files = unparseable_files.len();

    let status_text = if oracle_unparseable_files == 0 {
        "Roslyn C# oracle succeeded".to_string()
    } else {
        format!(
            "Roslyn C# oracle succeeded with {oracle_unparseable_files} unparseable files excluded from symbol FP accounting"
        )
    };

    Ok(CsharpOracleReport {
        oracle_ms,
        oracle_build_ms: oracle.build_ms,
        status: status_text,
        oracle_unparseable_files,
        oracle_unparseable_examples,
        symbols,
        limitations: vec![
            "The C# oracle uses Roslyn's CSharpSyntaxTree (syntactic, not semantic), so it counts declarations but does not resolve members inherited from referenced assemblies.".to_string(),
            "Symbol comparison is file/name/kind based; the oracle reports partial declarations once per source file, mirroring squeezy's own behavior.".to_string(),
            "C# files that Roslyn cannot parse (e.g. invalid syntax) are reported as oracle_unparseable and excluded from Squeezy false-positive accounting.".to_string(),
        ],
    })
}

pub(crate) fn csharp_oracle_to_accuracy(report: &CsharpOracleReport) -> AccuracyReport {
    AccuracyReport {
        rust_analyzer_symbols_ms: Some(report.oracle_ms),
        rust_analyzer_symbol_status: report.status.clone(),
        symbols: report.symbols.clone(),
        navigation: NavigationAccuracyReport {
            rust_analyzer_lsp_ms: None,
            rust_analyzer_lsp_status: "C# LSP navigation oracle not used".to_string(),
            requested_probe_limit: 0,
            definitions: DefinitionAccuracyReport::default(),
            references: ReferenceAccuracyReport::default(),
            limitations: vec![
                "C# accuracy currently compares symbol declarations against Roslyn; LSP-style go-to-definition probes are not exercised yet.".to_string(),
            ],
        },
        limitations: report.limitations.clone(),
    }
}

pub(crate) fn go_oracle_to_accuracy(report: &GoOracleReport) -> AccuracyReport {
    AccuracyReport {
        rust_analyzer_symbols_ms: Some(report.oracle_ms),
        rust_analyzer_symbol_status: report.status.clone(),
        symbols: report.symbols.clone(),
        navigation: NavigationAccuracyReport {
            rust_analyzer_lsp_ms: None,
            rust_analyzer_lsp_status: "Go LSP navigation oracle not used".to_string(),
            requested_probe_limit: 0,
            definitions: DefinitionAccuracyReport::default(),
            references: ReferenceAccuracyReport::default(),
            limitations: vec![
                "Go accuracy currently compares symbol declarations against the Go parser/type oracle; LSP-style go-to-definition probes are not exercised yet.".to_string(),
            ],
        },
        limitations: report.limitations.clone(),
    }
}

/// Combine symbol oracle + TypeScript Language Service navigation probes into a
/// single `AccuracyReport`. When `probe_limit == 0` the navigation half is
/// skipped (same semantics as `--ra-lsp-probes 0` for Rust).
pub(crate) fn collect_js_ts_accuracy(
    root: &Path,
    graph: &SemanticGraph,
    probe_limit: usize,
) -> AccuracyReport {
    let oracle = collect_js_ts_oracle_accuracy(root, graph);
    let navigation = collect_js_ts_navigation_accuracy(root, graph, probe_limit);
    AccuracyReport {
        rust_analyzer_symbols_ms: Some(oracle.oracle_ms),
        rust_analyzer_symbol_status: oracle.status.clone(),
        symbols: oracle.symbols.clone(),
        navigation,
        limitations: oracle.limitations.clone(),
    }
}

// ─── JS/TS TypeScript Language Service navigation oracle ────────────────────

/// Probes built from resolved Squeezy call edges in JS/TS files.
pub(crate) struct TsDefProbe {
    label: String,
    relative_file: String,
    byte_offset: u32,
    squeezy_target: Option<SymbolId>,
}

/// Probes built from JS/TS declaration symbols for reference comparison.
pub(crate) struct TsRefProbe {
    label: String,
    relative_file: String,
    byte_offset: u32,
    symbol_id: SymbolId,
    name: String,
}

pub(crate) fn js_ts_language(language: LanguageKind) -> bool {
    matches!(
        language,
        LanguageKind::JavaScript | LanguageKind::Jsx | LanguageKind::TypeScript | LanguageKind::Tsx
    )
}

pub(crate) fn build_ts_definition_probes(
    graph: &SemanticGraph,
    limit: usize,
) -> Result<(usize, Vec<TsDefProbe>)> {
    let mut edges: Vec<_> = graph
        .edges()
        .iter()
        .filter(|edge| edge.kind == EdgeKind::Calls)
        .filter_map(|edge| {
            let span = edge.span?;
            let from = graph.symbols.get(&edge.from)?;
            let file = graph.files.get(&from.file_id)?;
            if !js_ts_language(file.language) {
                return None;
            }
            Some((file.relative_path.clone(), span.start_byte, edge, file))
        })
        .collect();
    edges.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.target_text.cmp(&b.2.target_text))
    });
    let available = edges.len();
    let selected = select_scenarios(available, limit);

    let mut probes = Vec::new();
    for index in selected {
        let (_, _, edge, file) = edges[index];
        let Some(span) = edge.span else {
            continue;
        };
        let source = match fs::read_to_string(&file.path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let byte = probe_byte_for_edge(
            &source,
            span.start_byte as usize,
            span.end_byte as usize,
            &edge.target_text,
        );
        let pos = byte_to_lsp_position(&source, byte);
        probes.push(TsDefProbe {
            label: format!(
                "{}:{}:{} {}",
                file.relative_path,
                pos.line + 1,
                pos.character + 1,
                edge.target_text
            ),
            relative_file: file.relative_path.clone(),
            byte_offset: byte as u32,
            squeezy_target: edge.to.clone(),
        });
    }
    Ok((available, probes))
}

pub(crate) fn build_ts_reference_probes(
    graph: &SemanticGraph,
    limit: usize,
) -> Result<(usize, Vec<TsRefProbe>)> {
    let mut symbols: Vec<_> = graph
        .symbols
        .values()
        .filter(|sym| {
            matches!(
                sym.kind,
                SymbolKind::Function
                    | SymbolKind::Class
                    | SymbolKind::Interface
                    | SymbolKind::TypeAlias
                    | SymbolKind::Method
                    | SymbolKind::Enum
            ) && sym.name.len() >= 3
        })
        .filter(|sym| {
            graph
                .files
                .get(&sym.file_id)
                .map(|f| js_ts_language(f.language))
                .unwrap_or(false)
        })
        .collect();
    symbols.sort_by(|a, b| {
        a.file_id
            .0
            .cmp(&b.file_id.0)
            .then(a.span.start_byte.cmp(&b.span.start_byte))
            .then(a.name.cmp(&b.name))
    });
    let available = symbols.len();
    let selected = select_scenarios(available, limit);

    let mut probes = Vec::new();
    for index in selected {
        let sym = symbols[index];
        let Some(file) = graph.files.get(&sym.file_id) else {
            continue;
        };
        let source = match fs::read_to_string(&file.path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let byte = probe_byte_for_symbol(
            &source,
            sym.span.start_byte as usize,
            sym.span.end_byte as usize,
            &sym.name,
        );
        let pos = byte_to_lsp_position(&source, byte);
        probes.push(TsRefProbe {
            label: format!(
                "{}:{}:{} {}",
                file.relative_path,
                pos.line + 1,
                pos.character + 1,
                sym.name
            ),
            relative_file: file.relative_path.clone(),
            byte_offset: byte as u32,
            symbol_id: sym.id.clone(),
            name: sym.name.clone(),
        });
    }
    Ok((available, probes))
}

/// Embedded Node.js script that drives the TypeScript Language Service.
/// Reads a JSON document from stdin:
///   `{ root, def_probes: [{file, byte_offset, label}], ref_probes: [{file, byte_offset, label}] }`
/// Writes a JSON document to stdout:
///   `{ def_results: [{..., ts_locs: [{file, line, character}]}],
///      ref_results: [{..., ts_refs: [{file, line, character}]}] }`
const TS_NAV_ORACLE_SCRIPT: &str = r#"
(function() {
  'use strict';
  const fs = require('fs');
  const path = require('path');
  const ts = require(process.env.SQUEEZY_TYPESCRIPT_PATH || 'typescript');

  let buf = '';
  process.stdin.on('data', function(d) { buf += d; });
  process.stdin.on('end', function() {
    let parsed;
    try { parsed = JSON.parse(buf); } catch (e) {
      process.stdout.write(JSON.stringify({ error: 'parse input: ' + e }));
      return;
    }
    const root = parsed.root;
    const defProbes = parsed.def_probes || [];
    const refProbes = parsed.ref_probes || [];

    // Discover all JS/TS source files (skip generated/hidden/vendor)
    const SKIP = new Set(['.git','node_modules','dist','build','out','coverage',
      '__pycache__','vendor','.next','.nuxt','.svelte-kit']);
    const GEN_MARKERS = ['@generated','auto-generated','automatically generated',
      'code generated','do not edit'];
    const GEN_BYTES = 4096;
    const fileSet = new Set();
    function walk(dir) {
      let entries;
      try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch { return; }
      for (const e of entries) {
        if (SKIP.has(e.name) || e.name.startsWith('.')) continue;
        const full = path.join(dir, e.name);
        if (e.isDirectory()) { walk(full); continue; }
        if (!/\.[cm]?[jt]sx?$/.test(e.name)) continue;
        if (e.name.endsWith('.d.ts') || e.name.endsWith('.d.cts') || e.name.endsWith('.d.mts')) {
          // skip declaration files -- TypeScript treats them differently for findReferences
          continue;
        }
        try {
          const head = Buffer.allocUnsafe(GEN_BYTES);
          const fd = fs.openSync(full, 'r');
          const bytesRead = fs.readSync(fd, head, 0, GEN_BYTES, 0);
          fs.closeSync(fd);
          const preview = head.slice(0, bytesRead).toString('utf8').toLowerCase();
          if (GEN_MARKERS.some(function(m) { return preview.includes(m); })) continue;
        } catch {}
        fileSet.add(full);
      }
    }
    walk(root);
    const files = Array.from(fileSet);

    // TypeScript Language Service host
    const host = {
      getScriptFileNames: function() { return files; },
      getScriptVersion: function() { return '1'; },
      getScriptSnapshot: function(f) {
        try { return ts.ScriptSnapshot.fromString(fs.readFileSync(f, 'utf8')); } catch { return undefined; }
      },
      getCurrentDirectory: function() { return root; },
      getCompilationSettings: function() {
        return {
          target: ts.ScriptTarget.Latest,
          allowJs: true, checkJs: false,
          jsx: ts.JsxEmit.Preserve,
          moduleResolution: ts.ModuleResolutionKind.Node10,
          noEmit: true, skipLibCheck: true,
        };
      },
      getDefaultLibFileName: function(opts) { return ts.getDefaultLibFilePath(opts); },
      fileExists: ts.sys.fileExists,
      readFile: ts.sys.readFile,
      readDirectory: ts.sys.readDirectory,
      directoryExists: ts.sys.directoryExists,
      getDirectories: ts.sys.getDirectories,
      useCaseSensitiveFileNames: function() { return true; },
    };

    let ls;
    try {
      ls = ts.createLanguageService(host, ts.createDocumentRegistry());
    } catch (e) {
      process.stdout.write(JSON.stringify({ error: 'LanguageService: ' + e }));
      return;
    }

    function absPath(file) {
      return path.isAbsolute(file) ? file : path.join(root, file);
    }
    function relPath(file) {
      return path.relative(root, file).replace(/\\/g, '/');
    }
    function getLineChar(sf, offset) {
      try { return ts.getLineAndCharacterOfPosition(sf, offset); } catch { return { line: 0, character: 0 }; }
    }

    // Definition probes
    const defResults = defProbes.map(function(probe) {
      const absFile = absPath(probe.file);
      try {
        const prog = ls.getProgram();
        const defs = ls.getDefinitionAtPosition(absFile, probe.byte_offset) || [];
        const tsLocs = defs.reduce(function(acc, d) {
          const sf = prog ? prog.getSourceFile(d.fileName) : null;
          if (!sf) return acc;
          const lc = getLineChar(sf, d.textSpan.start);
          acc.push({ file: relPath(d.fileName), line: lc.line, character: lc.character });
          return acc;
        }, []);
        return { file: probe.file, byte_offset: probe.byte_offset, label: probe.label, ts_locs: tsLocs };
      } catch (e) {
        return { file: probe.file, byte_offset: probe.byte_offset, label: probe.label, ts_locs: [], error: String(e) };
      }
    });

    // Reference probes
    const refResults = refProbes.map(function(probe) {
      const absFile = absPath(probe.file);
      try {
        const prog = ls.getProgram();
        const groups = ls.findReferences(absFile, probe.byte_offset) || [];
        const tsRefs = [];
        for (const group of groups) {
          for (const ref of group.references) {
            if (ref.isDefinition) continue;  // exclude the declaration itself
            const sf = prog ? prog.getSourceFile(ref.fileName) : null;
            if (!sf) continue;
            const lc = getLineChar(sf, ref.textSpan.start);
            tsRefs.push({ file: relPath(ref.fileName), line: lc.line, character: lc.character });
          }
        }
        return { file: probe.file, byte_offset: probe.byte_offset, label: probe.label, ts_refs: tsRefs };
      } catch (e) {
        return { file: probe.file, byte_offset: probe.byte_offset, label: probe.label, ts_refs: [], error: String(e) };
      }
    });

    process.stdout.write(JSON.stringify({ def_results: defResults, ref_results: refResults }));
  });
})();
"#;

pub(crate) fn run_ts_navigation_node(
    root: &Path,
    def_probes: &[TsDefProbe],
    ref_probes: &[TsRefProbe],
) -> Result<Value> {
    let input = json!({
        "root": root.to_string_lossy(),
        "def_probes": def_probes.iter().map(|p| json!({
            "file": p.relative_file,
            "byte_offset": p.byte_offset,
            "label": p.label,
        })).collect::<Vec<_>>(),
        "ref_probes": ref_probes.iter().map(|p| json!({
            "file": p.relative_file,
            "byte_offset": p.byte_offset,
            "label": p.label,
        })).collect::<Vec<_>>(),
    });
    let input_bytes = input.to_string().into_bytes();

    let mut child = Command::new("node")
        .arg("-e")
        .arg(TS_NAV_ORACLE_SCRIPT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SqueezyError::Graph(format!("node unavailable: {e}")))?;

    // Write all probe data to stdin then close it so node sees EOF.
    {
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SqueezyError::Graph("node stdin unavailable".to_string()))?;
        let mut stdin = stdin;
        stdin
            .write_all(&input_bytes)
            .map_err(|e| SqueezyError::Graph(format!("node stdin write: {e}")))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| SqueezyError::Graph(format!("node wait: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first_line = stderr.lines().next().unwrap_or("(no stderr)");
        return Err(SqueezyError::Graph(format!(
            "node TS navigation oracle failed: {first_line}"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result: Value = serde_json::from_str(&stdout)
        .map_err(|e| SqueezyError::Graph(format!("TS navigation oracle JSON: {e}")))?;

    if let Some(err) = result.get("error").and_then(|v| v.as_str()) {
        return Err(SqueezyError::Graph(format!(
            "TS navigation oracle error: {err}"
        )));
    }
    Ok(result)
}

pub(crate) fn score_ts_definition_results(
    root: &Path,
    graph: &SemanticGraph,
    probes: &[TsDefProbe],
    available: usize,
    raw_results: &[Value],
) -> DefinitionAccuracyReport {
    let mut report = DefinitionAccuracyReport {
        available_probes: available,
        probes: probes.len(),
        ..DefinitionAccuracyReport::default()
    };
    for (probe, result) in probes.iter().zip(raw_results.iter()) {
        let ts_locs: Vec<LocationKey> = result["ts_locs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                let file = v["file"].as_str()?.to_string();
                let line = v["line"].as_u64()? as u32;
                let character = v["character"].as_u64()? as u32;
                Some(LocationKey {
                    file,
                    line,
                    character,
                })
            })
            .collect();

        let squeezy_has_target = probe.squeezy_target.is_some();
        let squeezy_matches = probe
            .squeezy_target
            .as_ref()
            .and_then(|id| graph.symbols.get(id))
            .map(|symbol| {
                ts_locs
                    .iter()
                    .any(|loc| location_matches_symbol(root, graph, loc, symbol))
            })
            .unwrap_or(false);

        match (ts_locs.is_empty(), squeezy_has_target, squeezy_matches) {
            (false, true, true) => report.true_positive += 1,
            (false, false, _) => {
                report.false_negative += 1;
                push_example(
                    &mut report.examples,
                    format!(
                        "FN definition {}: TS -> {}, Squeezy unresolved",
                        probe.label,
                        render_locations(&ts_locs)
                    ),
                );
            }
            (false, true, false) => {
                report.false_positive += 1;
                report.false_negative += 1;
                report.wrong_target += 1;
                push_example(
                    &mut report.examples,
                    format!(
                        "Wrong definition {}: TS -> {}, Squeezy -> {}",
                        probe.label,
                        render_locations(&ts_locs),
                        probe
                            .squeezy_target
                            .as_ref()
                            .map(|id| id.0.as_str())
                            .unwrap_or("<none>")
                    ),
                );
            }
            (true, true, false) => {
                report.false_positive += 1;
                report.squeezy_only += 1;
                push_example(
                    &mut report.examples,
                    format!(
                        "Squeezy-only definition {}: TS unresolved, Squeezy -> {}",
                        probe.label,
                        probe
                            .squeezy_target
                            .as_ref()
                            .map(|id| id.0.as_str())
                            .unwrap_or("<none>")
                    ),
                );
            }
            (true, false, _) => report.unresolved_agreement += 1,
            (true, true, true) => unreachable!("matched target requires a TS location"),
        }
    }
    report.precision = ratio(
        report.true_positive,
        report.true_positive + report.false_positive,
    );
    report.recall = ratio(
        report.true_positive,
        report.true_positive + report.false_negative,
    );
    report
}

pub(crate) fn score_ts_reference_results(
    graph: &SemanticGraph,
    probes: &[TsRefProbe],
    available: usize,
    raw_results: &[Value],
) -> ReferenceAccuracyReport {
    let mut report = ReferenceAccuracyReport {
        available_symbols: available,
        symbols_sampled: probes.len(),
        ..ReferenceAccuracyReport::default()
    };
    for (probe, result) in probes.iter().zip(raw_results.iter()) {
        let ts_refs: BTreeSet<LocationKey> = result["ts_refs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                let file = v["file"].as_str()?.to_string();
                let line = v["line"].as_u64()? as u32;
                let character = v["character"].as_u64()? as u32;
                Some(LocationKey {
                    file,
                    line,
                    character,
                })
            })
            .collect();

        let squeezy: BTreeSet<LocationKey> = graph
            .references_to_symbol(&probe.symbol_id)
            .into_iter()
            .filter_map(|hit| location_key_for_reference_hit(graph, &hit, &probe.name))
            .collect();

        let tp = squeezy.intersection(&ts_refs).count();
        let fp: Vec<_> = squeezy.difference(&ts_refs).cloned().collect();
        let fn_: Vec<_> = ts_refs.difference(&squeezy).cloned().collect();
        report.true_positive += tp;
        report.false_positive += fp.len();
        report.false_negative += fn_.len();

        if !fp.is_empty() {
            push_example(
                &mut report.false_positive_examples,
                format!(
                    "{} FP refs: {}",
                    probe.label,
                    fp.iter()
                        .take(5)
                        .map(LocationKey::render)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
        if !fn_.is_empty() {
            push_example(
                &mut report.false_negative_examples,
                format!(
                    "{} FN refs: {}",
                    probe.label,
                    fn_.iter()
                        .take(5)
                        .map(LocationKey::render)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
    }
    report.precision = ratio(
        report.true_positive,
        report.true_positive + report.false_positive,
    );
    report.recall = ratio(
        report.true_positive,
        report.true_positive + report.false_negative,
    );
    report
}

pub(crate) fn collect_js_ts_navigation_accuracy(
    root: &Path,
    graph: &SemanticGraph,
    probe_limit: usize,
) -> NavigationAccuracyReport {
    if probe_limit == 0 {
        return NavigationAccuracyReport {
            rust_analyzer_lsp_ms: None,
            rust_analyzer_lsp_status: "disabled by --ra-lsp-probes 0".to_string(),
            requested_probe_limit: 0,
            definitions: DefinitionAccuracyReport::default(),
            references: ReferenceAccuracyReport::default(),
            limitations: js_ts_nav_limitations(),
        };
    }

    let (def_available, def_probes) = match build_ts_definition_probes(graph, probe_limit) {
        Ok(p) => p,
        Err(e) => {
            return nav_report_error(
                &format!("definition probe build: {e}"),
                probe_limit,
                js_ts_nav_limitations(),
            );
        }
    };
    let (ref_available, ref_probes) = match build_ts_reference_probes(graph, probe_limit) {
        Ok(p) => p,
        Err(e) => {
            return nav_report_error(
                &format!("reference probe build: {e}"),
                probe_limit,
                js_ts_nav_limitations(),
            );
        }
    };

    if def_probes.is_empty() && ref_probes.is_empty() {
        return NavigationAccuracyReport {
            rust_analyzer_lsp_ms: None,
            rust_analyzer_lsp_status: "no JS/TS call edges or symbols found for navigation probes"
                .to_string(),
            requested_probe_limit: probe_limit,
            definitions: DefinitionAccuracyReport {
                available_probes: def_available,
                ..DefinitionAccuracyReport::default()
            },
            references: ReferenceAccuracyReport {
                available_symbols: ref_available,
                ..ReferenceAccuracyReport::default()
            },
            limitations: js_ts_nav_limitations(),
        };
    }

    let started = Instant::now();
    let oracle_result = match run_ts_navigation_node(root, &def_probes, &ref_probes) {
        Ok(r) => r,
        Err(e) => {
            return nav_report_error(
                &format!("TS Language Service oracle: {e}"),
                probe_limit,
                js_ts_nav_limitations(),
            );
        }
    };
    let elapsed = started.elapsed().as_millis();

    let raw_defs = oracle_result["def_results"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let raw_refs = oracle_result["ref_results"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let definitions =
        score_ts_definition_results(root, graph, &def_probes, def_available, &raw_defs);
    let references = score_ts_reference_results(graph, &ref_probes, ref_available, &raw_refs);

    NavigationAccuracyReport {
        rust_analyzer_lsp_ms: Some(elapsed),
        rust_analyzer_lsp_status:
            "TypeScript Language Service definition/reference probes succeeded".to_string(),
        requested_probe_limit: probe_limit,
        definitions,
        references,
        limitations: js_ts_nav_limitations(),
    }
}

pub(crate) fn nav_report_error(msg: &str, limit: usize, limitations: Vec<String>) -> NavigationAccuracyReport {
    NavigationAccuracyReport {
        rust_analyzer_lsp_ms: None,
        rust_analyzer_lsp_status: msg.to_string(),
        requested_probe_limit: limit,
        definitions: DefinitionAccuracyReport::default(),
        references: ReferenceAccuracyReport::default(),
        limitations,
    }
}

pub(crate) fn js_ts_nav_limitations() -> Vec<String> {
    vec![
        "Definition probes compare Squeezy resolved JS/TS call edge targets with TypeScript Language Service getDefinitionAtPosition for sampled call sites.".to_string(),
        "Reference probes compare Squeezy references_to_symbol results with TypeScript Language Service findReferences for sampled declarations; the declaration position itself is excluded from the reference set.".to_string(),
        "The TypeScript Language Service is not type-checking at full depth (skipLibCheck, noEmit); probes are accurate for within-workspace calls. External library definitions show as FN for Squeezy.".to_string(),
        "Byte offsets are used as character offsets; for ASCII-dominant TypeScript source this is exact. Multi-byte unicode in the same line as a call site can shift the probe by a few characters.".to_string(),
        "Samples are deterministic and capped by --ra-lsp-probes; increase for deeper local audits.".to_string(),
    ]
}

#[derive(Debug, Deserialize)]
pub(crate) struct CsharpOracleOutput {
    rows: Vec<[String; 3]>,
    unparseable_files: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct CsharpOracleSymbolScan {
    symbols: SymbolScan,
    unparseable_files: Vec<String>,
    build_ms: Option<u128>,
}

pub(crate) fn collect_csharp_oracle_symbol_scan(root: &Path) -> Result<CsharpOracleSymbolScan> {
    let (dll, build_ms) = ensure_csharp_oracle_built()?;
    let output = Command::new("dotnet")
        .arg(&dll)
        .arg(root)
        .output()
        .map_err(|err| SqueezyError::Graph(format!("failed to run Roslyn C# oracle: {err}")))?;
    if !output.status.success() {
        return Err(SqueezyError::Graph(format!(
            "Roslyn C# oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let output: CsharpOracleOutput = serde_json::from_slice(&output.stdout)
        .map_err(|err| SqueezyError::Graph(format!("invalid Roslyn C# oracle JSON: {err}")))?;
    let mut scan = SymbolScan::default();
    for [file, kind, name] in output.rows {
        scan.raw_total += 1;
        increment_symbol(
            &mut scan.counts,
            SymbolKey {
                file,
                kind,
                name: normalize_symbol_name(&name),
            },
        );
    }
    Ok(CsharpOracleSymbolScan {
        symbols: scan,
        unparseable_files: output.unparseable_files,
        build_ms,
    })
}

pub(crate) fn ensure_csharp_oracle_built() -> Result<(PathBuf, Option<u128>)> {
    let project = csharp_oracle_project_dir()?;
    let dll = project
        .join("bin")
        .join("Release")
        .join("net8.0")
        .join("CsharpOracle.dll");
    if dll.exists() {
        return Ok((dll, None));
    }
    let started = Instant::now();
    let status = Command::new("dotnet")
        .arg("build")
        .arg(&project)
        .arg("-c")
        .arg("Release")
        .arg("--nologo")
        .arg("-v")
        .arg("minimal")
        .status()
        .map_err(|err| SqueezyError::Graph(format!("failed to build Roslyn C# oracle: {err}")))?;
    let build_ms = started.elapsed().as_millis();
    if !status.success() {
        return Err(SqueezyError::Graph(format!(
            "Roslyn C# oracle build failed with {status}"
        )));
    }
    if !dll.exists() {
        return Err(SqueezyError::Graph(format!(
            "Roslyn C# oracle build did not produce {}",
            dll.display()
        )));
    }
    Ok((dll, Some(build_ms)))
}

pub(crate) fn csharp_oracle_project_dir() -> Result<PathBuf> {
    if let Ok(value) = std::env::var("SQUEEZY_CSHARP_ORACLE_DIR")
        && !value.is_empty()
    {
        let path = PathBuf::from(value);
        if path.exists() {
            return Ok(path);
        }
    }
    let candidates: [PathBuf; 3] = [
        PathBuf::from("benchmarks/oracle/csharp"),
        PathBuf::from("../oracle/csharp"),
        PathBuf::from("../../benchmarks/oracle/csharp"),
    ];
    for candidate in candidates {
        if candidate.join("CsharpOracle.csproj").exists() {
            return Ok(candidate);
        }
    }
    Err(SqueezyError::Graph(
        "could not locate benchmarks/oracle/csharp; set SQUEEZY_CSHARP_ORACLE_DIR".to_string(),
    ))
}

const PYTHON_AST_ORACLE: &str = r#"
import ast
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1]).resolve()
rows = []
unparseable_files = []

class Visitor(ast.NodeVisitor):
    def __init__(self, rel):
        self.rel = rel
        self.parents = []

    def visit_ClassDef(self, node):
        rows.append([self.rel, "Class", node.name])
        self.parents.append("Class")
        self.generic_visit(node)
        self.parents.pop()

    def visit_FunctionDef(self, node):
        kind = "Method" if self.parents and self.parents[-1] == "Class" else "Function"
        rows.append([self.rel, kind, node.name])
        self.parents.append(kind)
        self.generic_visit(node)
        self.parents.pop()

    visit_AsyncFunctionDef = visit_FunctionDef

for path in sorted(root.rglob("*.py")):
    rel = path.relative_to(root).as_posix()
    try:
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    except (SyntaxError, UnicodeDecodeError):
        unparseable_files.append(rel)
        continue
    Visitor(rel).visit(tree)

print(json.dumps({"rows": rows, "unparseable_files": unparseable_files}))
"#;

pub(crate) fn time_java_oracle_optional(root: &Path) -> (u128, String) {
    if !command_exists("java") {
        return (0, "skipped: java not found".to_string());
    }
    let started = Instant::now();
    match collect_java_compiler_tree_symbol_scan(root) {
        Ok((_, status)) if status.starts_with("JDK compiler tree oracle succeeded") => {
            (started.elapsed().as_millis(), status)
        }
        Ok((_, status)) => (0, format!("skipped: {status}")),
        Err(err) => (0, format!("skipped: Java oracle failed: {err}")),
    }
}

pub(crate) fn collect_java_oracle_accuracy(
    root: &Path,
    graph: &SemanticGraph,
    queries: &[QueryReport],
) -> Result<JavaOracleReport> {
    if !command_exists("java") {
        return Ok(JavaOracleReport {
            oracle_ms: None,
            status: "skipped: java not found".to_string(),
            symbols: compare_symbol_sets(
                &collect_squeezy_symbol_scan(graph),
                &SymbolScan::default(),
            ),
            navigation: collect_query_oracle_accuracy(queries),
            limitations: java_oracle_limitations(),
        });
    }
    let started = Instant::now();
    match collect_java_compiler_tree_symbol_scan(root) {
        Ok((oracle, status)) if status.starts_with("JDK compiler tree oracle succeeded") => {
            let oracle_ms = started.elapsed().as_millis();
            let squeezy_symbols = collect_squeezy_symbol_scan(graph);
            Ok(JavaOracleReport {
                oracle_ms: Some(oracle_ms),
                status,
                symbols: compare_symbol_sets(&squeezy_symbols, &oracle),
                navigation: collect_query_oracle_accuracy(queries),
                limitations: java_oracle_limitations(),
            })
        }
        Ok((_, status)) => Ok(JavaOracleReport {
            oracle_ms: None,
            status: format!("skipped: {status}"),
            symbols: compare_symbol_sets(
                &collect_squeezy_symbol_scan(graph),
                &SymbolScan::default(),
            ),
            navigation: collect_query_oracle_accuracy(queries),
            limitations: java_oracle_limitations(),
        }),
        Err(err) => Ok(JavaOracleReport {
            oracle_ms: None,
            status: format!("skipped: Java oracle failed: {err}"),
            symbols: compare_symbol_sets(
                &collect_squeezy_symbol_scan(graph),
                &SymbolScan::default(),
            ),
            navigation: collect_query_oracle_accuracy(queries),
            limitations: java_oracle_limitations(),
        }),
    }
}

pub(crate) fn collect_query_oracle_accuracy(queries: &[QueryReport]) -> QueryOracleReport {
    let true_positive = queries
        .iter()
        .map(|query| {
            query
                .expected_contains
                .iter()
                .filter(|expected| query.actual.contains(expected))
                .count()
        })
        .sum::<usize>();
    let false_negative = queries
        .iter()
        .map(|query| query.missing.len())
        .sum::<usize>();
    // Query specs use expected_contains, not an exhaustive expected set, so
    // extra results stay visible on each query but are not counted as oracle FP.
    let false_positive = 0;
    QueryOracleReport {
        status: "fixture query truth (minimum expected_contains oracle)".to_string(),
        query_count: queries.len(),
        true_positive,
        false_positive,
        false_negative,
        precision: ratio(true_positive, true_positive + false_positive),
        recall: ratio(true_positive, true_positive + false_negative),
    }
}

pub(crate) fn java_oracle_limitations() -> Vec<String> {
    vec![
        "The Java oracle uses the JDK compiler tree API for declarations only and does not require successful type attribution.".to_string(),
        "Symbol comparison is file/name/kind based; overload resolution, dispatch, generated sources, annotation processors, and external libraries remain separate navigation-loss areas.".to_string(),
        "If java or a JDK compiler is unavailable, the oracle is skipped while fixture query gates still run.".to_string(),
    ]
}

#[derive(Debug, Deserialize)]
pub(crate) struct JavaOracleOutput {
    rows: Vec<[String; 3]>,
}

pub(crate) fn collect_java_compiler_tree_symbol_scan(root: &Path) -> Result<(SymbolScan, String)> {
    let temp = temp_dir("squeezy-java-oracle")?;
    let oracle_path = temp.join("JavaOracle.java");
    fs::write(&oracle_path, JAVA_COMPILER_TREE_ORACLE)?;
    let output = Command::new("java")
        .arg(&oracle_path)
        .arg(root)
        .output()
        .map_err(|err| SqueezyError::Graph(format!("failed to run Java oracle: {err}")))?;
    if !output.status.success() {
        return Ok((
            SymbolScan::default(),
            format!(
                "Java oracle unavailable: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    let output: JavaOracleOutput = serde_json::from_slice(&output.stdout)
        .map_err(|err| SqueezyError::Graph(format!("invalid Java oracle JSON: {err}")))?;
    let mut scan = SymbolScan::default();
    for [file, kind, name] in output.rows {
        scan.raw_total += 1;
        increment_symbol(
            &mut scan.counts,
            SymbolKey {
                file,
                kind,
                name: normalize_symbol_name(&name),
            },
        );
    }
    Ok((
        scan.clone(),
        format!(
            "JDK compiler tree oracle succeeded with {} declaration symbols",
            symbol_count(&scan.counts)
        ),
    ))
}

const JAVA_COMPILER_TREE_ORACLE: &str = r#"
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import javax.tools.JavaCompiler;
import javax.tools.StandardJavaFileManager;
import javax.tools.ToolProvider;
import com.sun.source.tree.ClassTree;
import com.sun.source.tree.CompilationUnitTree;
import com.sun.source.tree.MethodTree;
import com.sun.source.tree.Tree;
import com.sun.source.util.JavacTask;
import com.sun.source.util.TreeScanner;

public class JavaOracle {
  record Row(String file, String kind, String name) {}

  public static void main(String[] args) throws Exception {
    JavaCompiler compiler = ToolProvider.getSystemJavaCompiler();
    if (compiler == null) {
      System.err.println("JDK compiler is not available");
      System.exit(2);
    }
    Path root = Path.of(args[0]).toAbsolutePath().normalize();
    List<Path> files = Files.walk(root)
      .filter(path -> path.toString().endsWith(".java"))
      .sorted()
      .toList();
    List<Row> rows = new ArrayList<>();
    try (StandardJavaFileManager manager = compiler.getStandardFileManager(null, null, StandardCharsets.UTF_8)) {
      Iterable units = manager.getJavaFileObjectsFromPaths(files);
      JavacTask task = (JavacTask) compiler.getTask(null, manager, null, List.of("-proc:none"), null, units);
      for (CompilationUnitTree unit : task.parse()) {
        String rel = root.relativize(Path.of(unit.getSourceFile().toUri()).toAbsolutePath().normalize()).toString().replace('\\', '/');
        new Scanner(rel, rows).scan(unit, null);
      }
    }
    rows.sort(Comparator.comparing(Row::file).thenComparing(Row::kind).thenComparing(Row::name));
    StringBuilder out = new StringBuilder();
    out.append("{\"rows\":[");
    for (int i = 0; i < rows.size(); i++) {
      Row row = rows.get(i);
      if (i > 0) out.append(',');
      out.append("[\"").append(escape(row.file())).append("\",\"")
        .append(escape(row.kind())).append("\",\"")
        .append(escape(row.name())).append("\"]");
    }
    out.append("]}");
    System.out.println(out);
  }

  static class Scanner extends TreeScanner<Void, Void> {
    private final String file;
    private final List<Row> rows;
    private final ArrayDeque<String> classes = new ArrayDeque<>();

    Scanner(String file, List<Row> rows) {
      this.file = file;
      this.rows = rows;
    }

    @Override
    public Void visitClass(ClassTree node, Void unused) {
      String kind = switch (node.getKind()) {
        case CLASS -> "Class";
        case INTERFACE, ANNOTATION_TYPE -> "Trait";
        case ENUM -> "Enum";
        case RECORD -> "Struct";
        default -> "Class";
      };
      String name = node.getSimpleName().toString();
      if (name.isEmpty()) {
        return super.visitClass(node, unused);
      }
      rows.add(new Row(file, kind, name));
      classes.push(name);
      super.visitClass(node, unused);
      classes.pop();
      return null;
    }

    @Override
    public Void visitMethod(MethodTree node, Void unused) {
      String name = node.getName().toString();
      if ("<init>".equals(name) && !classes.isEmpty()) {
        name = classes.peek();
      }
      rows.add(new Row(file, "Method", name));
      return super.visitMethod(node, unused);
    }
  }

  static String escape(String value) {
    return value.replace("\\", "\\\\").replace("\"", "\\\"");
  }
}
"#;

pub(crate) fn collect_clang_ast_symbol_scan(
    root: &Path,
    language: LanguageKind,
    file_limit: usize,
) -> Result<CFamilyClangSymbolScan> {
    let snapshot = WorkspaceCrawler::new(CrawlOptions::default()).crawl(root)?;
    let root = fs::canonicalize(root)?;
    let mut records = snapshot
        .files
        .iter()
        .filter(|record| record.language == language)
        .cloned()
        .collect::<Vec<_>>();
    records.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let indexes = select_scenarios(records.len(), file_limit);
    let selected = indexes.iter().copied().collect::<BTreeSet<_>>();
    let include_dirs = clang_include_dirs(&root, &snapshot.files);
    let excluded_initial = records
        .iter()
        .enumerate()
        .filter(|(index, _)| !selected.contains(index))
        .map(|(_, record)| record.relative_path.clone())
        .collect::<Vec<_>>();

    let selected_records = indexes
        .iter()
        .copied()
        .map(|index| records[index].clone())
        .collect::<Vec<_>>();
    if selected_records.is_empty() {
        return Ok(CFamilyClangSymbolScan {
            symbols: SymbolScan::default(),
            parsed_files: 0,
            candidate_files: records.len(),
            selected_files: 0,
            unparseable_files: Vec::new(),
            excluded_files: excluded_initial,
        });
    }

    let worker_count = std::thread::available_parallelism()
        .map(|threads| threads.get())
        .unwrap_or(1)
        .min(selected_records.len())
        .max(1);
    let chunk_size = selected_records.len().div_ceil(worker_count);

    let outputs = std::thread::scope(|scope| -> Result<Vec<ClangAstFileOutput>> {
        let mut handles = Vec::new();
        for chunk in selected_records.chunks(chunk_size) {
            let chunk = chunk.to_vec();
            let include_dirs = include_dirs.clone();
            let root = root.clone();
            handles.push(scope.spawn(move || -> Result<Vec<ClangAstFileOutput>> {
                let mut out = Vec::with_capacity(chunk.len());
                for record in chunk {
                    let result =
                        clang_ast_symbols_for_file(&root, &record, language, &include_dirs);
                    out.push(ClangAstFileOutput {
                        relative_path: record.relative_path.clone(),
                        result,
                    });
                }
                Ok(out)
            }));
        }
        let mut outputs = Vec::with_capacity(selected_records.len());
        for handle in handles {
            match handle.join() {
                Ok(Ok(mut chunk_outputs)) => outputs.append(&mut chunk_outputs),
                Ok(Err(err)) => return Err(err),
                Err(_) => {
                    return Err(SqueezyError::Graph(
                        "clang AST oracle worker panicked".to_string(),
                    ));
                }
            }
        }
        Ok(outputs)
    })?;

    let mut scan = SymbolScan::default();
    let mut parsed_files = 0usize;
    let mut unparseable_files = Vec::new();
    let mut excluded_files = excluded_initial;
    for output in outputs {
        match output.result {
            Ok(file_scan) => {
                parsed_files += 1;
                merge_symbol_scan(&mut scan, file_scan);
            }
            Err(_) => {
                unparseable_files.push(output.relative_path.clone());
                excluded_files.push(output.relative_path);
            }
        }
    }

    Ok(CFamilyClangSymbolScan {
        symbols: scan,
        parsed_files,
        candidate_files: records.len(),
        selected_files: parsed_files + unparseable_files.len(),
        unparseable_files,
        excluded_files,
    })
}

pub(crate) struct ClangAstFileOutput {
    relative_path: String,
    result: Result<SymbolScan>,
}

pub(crate) fn clang_ast_symbols_for_file(
    root: &Path,
    record: &squeezy_workspace::FileRecord,
    language: LanguageKind,
    include_dirs: &[PathBuf],
) -> Result<SymbolScan> {
    let root = fs::canonicalize(root)?;
    let main_file = fs::canonicalize(&record.path)?;
    let compiler = match language {
        LanguageKind::C => "clang",
        LanguageKind::Cpp => "clang++",
        _ => {
            return Err(SqueezyError::Graph(format!(
                "clang AST oracle does not support {language:?}"
            )));
        }
    };
    let x_language = clang_x_language(record, language);
    let mut command = Command::new(compiler);
    command
        .current_dir(&root)
        .arg("-Xclang")
        .arg("-ast-dump=json")
        .arg("-fsyntax-only")
        .arg("-fno-color-diagnostics")
        .arg("-Wno-everything")
        .arg("-x")
        .arg(x_language);
    for include_dir in include_dirs {
        command.arg("-I").arg(include_dir);
    }
    command.arg(&main_file);

    let output = command.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default();
        return Err(SqueezyError::Graph(format!(
            "{compiler} AST failed for {} with {}{}",
            record.relative_path,
            output.status,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {}", truncate(detail, 240))
            }
        )));
    }

    let ast: Value = serde_json::from_slice(&output.stdout)
        .map_err(|err| SqueezyError::Graph(format!("invalid clang AST JSON: {err}")))?;
    let mut scan = SymbolScan::default();
    collect_clang_ast_symbols_from_value(&ast, &root, &main_file, &record.relative_path, &mut scan);
    Ok(scan)
}

pub(crate) fn collect_clang_ast_symbols_from_value(
    node: &Value,
    root: &Path,
    main_file: &Path,
    relative_path: &str,
    scan: &mut SymbolScan,
) {
    if node.get("isImplicit").and_then(Value::as_bool) == Some(true) {
        return;
    }

    if let Some(kind) = clang_symbol_kind(node)
        && clang_node_is_in_main_file(node, root, main_file)
    {
        scan.raw_total += 1;
        if let Some(name) = node
            .get("name")
            .and_then(Value::as_str)
            .map(normalize_clang_symbol_name)
            .filter(|name| clang_symbol_name_is_comparable(name))
        {
            increment_unique_symbol(
                &mut scan.counts,
                SymbolKey {
                    file: relative_path.to_string(),
                    kind,
                    name,
                },
            );
        } else {
            increment(&mut scan.excluded_by_kind, "UnnamedOrOperator");
        }
    }

    if let Some(children) = node.get("inner").and_then(Value::as_array) {
        for child in children {
            collect_clang_ast_symbols_from_value(child, root, main_file, relative_path, scan);
        }
    }
}

pub(crate) fn clang_symbol_kind(node: &Value) -> Option<String> {
    let kind = node.get("kind").and_then(Value::as_str)?;
    match kind {
        "NamespaceDecl" => Some("Module".to_string()),
        "RecordDecl" => match node.get("tagUsed").and_then(Value::as_str) {
            Some("struct") => Some("Struct".to_string()),
            Some("union") => Some("Union".to_string()),
            _ => None,
        },
        "CXXRecordDecl" => match node.get("tagUsed").and_then(Value::as_str) {
            Some("struct") => Some("Struct".to_string()),
            Some("union") => Some("Union".to_string()),
            Some("class") | None => Some("Class".to_string()),
            _ => None,
        },
        "EnumDecl" => Some("Enum".to_string()),
        "FunctionDecl" => Some("Function".to_string()),
        "CXXMethodDecl" => Some("Method".to_string()),
        "TypedefDecl" | "TypeAliasDecl" => Some("TypeAlias".to_string()),
        _ => None,
    }
}

pub(crate) fn clang_node_is_in_main_file(node: &Value, root: &Path, main_file: &Path) -> bool {
    if node.pointer("/loc/includedFrom").is_some()
        || node.pointer("/range/begin/includedFrom").is_some()
    {
        return false;
    }
    let Some(raw_file) = clang_node_file(node) else {
        return true;
    };
    let path = PathBuf::from(raw_file);
    let absolute = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    fs::canonicalize(&absolute)
        .map(|path| path == main_file)
        .unwrap_or(false)
}

pub(crate) fn clang_node_file(node: &Value) -> Option<&str> {
    node.pointer("/loc/file")
        .and_then(Value::as_str)
        .or_else(|| node.pointer("/range/begin/file").and_then(Value::as_str))
}

pub(crate) fn clang_x_language(
    record: &squeezy_workspace::FileRecord,
    language: LanguageKind,
) -> &'static str {
    let extension = record
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    match (language, extension) {
        (LanguageKind::C, "h") => "c-header",
        (LanguageKind::Cpp, "h" | "hh" | "hpp" | "hxx") => "c++-header",
        (LanguageKind::C, _) => "c",
        (LanguageKind::Cpp, _) => "c++",
        _ => "c",
    }
}

pub(crate) fn clang_include_dirs(root: &Path, files: &[squeezy_workspace::FileRecord]) -> Vec<PathBuf> {
    let mut dirs = BTreeSet::new();
    dirs.insert(root.to_path_buf());
    for name in ["include", "src", "lib", "deps"] {
        let path = root.join(name);
        if path.is_dir() {
            dirs.insert(path);
        }
    }
    for file in files
        .iter()
        .filter(|file| matches!(file.language, LanguageKind::C | LanguageKind::Cpp))
    {
        let Some(parent) = file.path.parent() else {
            continue;
        };
        let parent_name = parent
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if matches!(parent_name, "include" | "inc" | "src" | "lib" | "deps") {
            dirs.insert(parent.to_path_buf());
        }
    }
    let mut dirs = dirs.into_iter().collect::<Vec<_>>();
    dirs.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then(left.cmp(right))
    });
    dirs.truncate(128);
    dirs
}

pub(crate) fn normalize_clang_symbol_name(name: &str) -> String {
    name.trim()
        .trim_start_matches('~')
        .split('<')
        .next()
        .unwrap_or(name)
        .rsplit("::")
        .next()
        .unwrap_or(name)
        .to_string()
}

pub(crate) fn clang_symbol_name_is_comparable(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with("__")
        && !name.starts_with("operator")
        && !name.contains("(anonymous")
}

pub(crate) fn collect_go_ast_symbol_scan(root: &Path) -> Result<GoAstSymbolScan> {
    // The oracle Go program is written to a dedicated sub-directory of the
    // system temp directory and the whole sub-directory is removed when this
    // function returns. Tracking the sub-directory explicitly (instead of
    // relying on `script_path.parent()`) keeps the cleanup scoped even if a
    // future change ever co-locates additional files with the script.
    let oracle_dir = temp_dir("squeezy-go-oracle")?;
    let script_path = oracle_dir.join("oracle.go");
    let result = run_go_ast_oracle(&script_path, root);
    let _ = fs::remove_dir_all(&oracle_dir);
    result
}

pub(crate) fn run_go_ast_oracle(script_path: &Path, root: &Path) -> Result<GoAstSymbolScan> {
    fs::write(script_path, GO_AST_ORACLE)?;
    let output = Command::new("go")
        .arg("run")
        .arg(script_path)
        .arg(root)
        .output()
        .map_err(|err| SqueezyError::Graph(format!("failed to run Go AST oracle: {err}")))?;
    if !output.status.success() {
        return Err(SqueezyError::Graph(format!(
            "Go AST oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let output: GoAstOracleOutput = serde_json::from_slice(&output.stdout)
        .map_err(|err| SqueezyError::Graph(format!("invalid Go AST oracle JSON: {err}")))?;
    let mut scan = SymbolScan::default();
    for [file, kind, name] in output.rows {
        scan.raw_total += 1;
        increment_symbol(
            &mut scan.counts,
            SymbolKey {
                file,
                kind,
                name: normalize_symbol_name(&name),
            },
        );
    }
    Ok(GoAstSymbolScan {
        symbols: scan,
        unparseable_files: output.unparseable_files,
    })
}

const GO_AST_ORACLE: &str = r#"
package main

import (
	"encoding/json"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

type Output struct {
	Rows             [][3]string `json:"rows"`
	UnparseableFiles []string    `json:"unparseable_files"`
}

func main() {
	root, _ := filepath.Abs(os.Args[1])
	out := Output{}
	filepath.WalkDir(root, func(path string, entry os.DirEntry, err error) error {
		if err != nil || entry.IsDir() {
			if entry != nil && entry.IsDir() && (entry.Name() == "vendor" || strings.HasPrefix(entry.Name(), ".")) {
				return filepath.SkipDir
			}
			return nil
		}
		if !strings.HasSuffix(path, ".go") {
			return nil
		}
		rel, _ := filepath.Rel(root, path)
		rel = filepath.ToSlash(rel)
		fset := token.NewFileSet()
		file, err := parser.ParseFile(fset, path, nil, parser.ParseComments)
		if err != nil {
			out.UnparseableFiles = append(out.UnparseableFiles, rel)
			return nil
		}
		for _, decl := range file.Decls {
			switch decl := decl.(type) {
			case *ast.FuncDecl:
				kind := "Function"
				if decl.Recv != nil {
					kind = "Method"
				}
				if strings.HasSuffix(rel, "_test.go") && (strings.HasPrefix(decl.Name.Name, "Test") || strings.HasPrefix(decl.Name.Name, "Benchmark") || strings.HasPrefix(decl.Name.Name, "Fuzz")) {
					kind = "Function"
				}
				out.Rows = append(out.Rows, [3]string{rel, kind, decl.Name.Name})
			case *ast.GenDecl:
				for _, spec := range decl.Specs {
					switch spec := spec.(type) {
					case *ast.TypeSpec:
						kind := "TypeAlias"
						switch spec.Type.(type) {
						case *ast.StructType:
							kind = "Struct"
						case *ast.InterfaceType:
							kind = "Interface"
						}
						out.Rows = append(out.Rows, [3]string{rel, kind, spec.Name.Name})
					case *ast.ValueSpec:
						kind := "Static"
						if decl.Tok == token.CONST {
							kind = "Const"
						}
						for _, name := range spec.Names {
							if name.Name != "_" {
								out.Rows = append(out.Rows, [3]string{rel, kind, name.Name})
							}
						}
					}
				}
			}
		}
		return nil
	})
	sort.Slice(out.Rows, func(i, j int) bool {
		return strings.Join(out.Rows[i][:], "\x00") < strings.Join(out.Rows[j][:], "\x00")
	})
	_ = json.NewEncoder(os.Stdout).Encode(out)
}
"#;

pub(crate) fn collect_rust_analyzer_symbol_scan(graph: &SemanticGraph) -> (SymbolScan, String) {
    let Some(program) = rust_analyzer_program() else {
        return (SymbolScan::default(), "rust-analyzer not found".to_string());
    };

    let mut records = graph
        .files
        .values()
        .filter(|record| record.language == LanguageKind::Rust)
        .collect::<Vec<_>>();
    records.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

    let mut scan = SymbolScan::default();
    let mut failures = Vec::new();
    for record in &records {
        match rust_analyzer_symbols_for_file(&program, record) {
            Ok(Some(file_scan)) => {
                merge_symbol_scan(&mut scan, file_scan);
            }
            Ok(None) => {
                scan.skipped_non_utf8_files += 1;
            }
            Err(err) => {
                failures.push(format!("{}: {err}", record.relative_path));
            }
        }
    }

    if failures.is_empty() {
        (
            scan.clone(),
            format!(
                "rust-analyzer symbols succeeded for {} Rust files; skipped {} non-UTF-8 Rust files",
                records.len() - scan.skipped_non_utf8_files,
                scan.skipped_non_utf8_files
            ),
        )
    } else {
        (
            scan,
            format!(
                "rust-analyzer symbols partially failed for {}/{} Rust files: {}",
                failures.len(),
                records.len(),
                failures
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        )
    }
}

pub(crate) fn rust_analyzer_symbols_for_file(
    program: &str,
    record: &squeezy_workspace::FileRecord,
) -> Result<Option<SymbolScan>> {
    let source = match fs::read_to_string(&record.path) {
        Ok(source) => source,
        Err(err) if err.kind() == std::io::ErrorKind::InvalidData => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let mut child = Command::new(program)
        .arg("symbols")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| SqueezyError::Graph("failed to open rust-analyzer stdin".to_string()))?;
    stdin.write_all(source.as_bytes())?;
    drop(stdin);

    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SqueezyError::Graph(format!(
            "rust-analyzer symbols failed with {}: {}",
            output.status,
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut scan = SymbolScan::default();
    for line in stdout.lines() {
        let Some((raw_kind, key)) = parse_rust_analyzer_symbol_line(line, &record.relative_path)
        else {
            continue;
        };
        scan.raw_total += 1;
        if let Some(key) = key {
            increment_symbol(&mut scan.counts, key);
        } else {
            increment(&mut scan.excluded_by_kind, &raw_kind);
        }
    }
    Ok(Some(scan))
}

pub(crate) fn parse_rust_analyzer_symbol_line(line: &str, file: &str) -> Option<(String, Option<SymbolKey>)> {
    let label = extract_quoted_field(line, "label")?;
    let raw_kind = extract_symbol_kind(line)?;
    let key = normalize_rust_analyzer_kind(&raw_kind).map(|kind| SymbolKey {
        file: file.to_string(),
        kind,
        name: normalize_symbol_name(&label),
    });
    Some((raw_kind, key))
}

pub(crate) fn extract_quoted_field(line: &str, field: &str) -> Option<String> {
    let prefix = format!("{field}: \"");
    let start = line.find(&prefix)? + prefix.len();
    let mut escaped = false;
    let mut value = String::new();
    for ch in line[start..].chars() {
        if escaped {
            value.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(value);
        } else {
            value.push(ch);
        }
    }
    None
}

pub(crate) fn extract_symbol_kind(line: &str) -> Option<String> {
    let prefix = "kind: SymbolKind(";
    let start = line.find(prefix)? + prefix.len();
    let rest = &line[start..];
    let end = rest.find(')')?;
    Some(rest[..end].to_string())
}

pub(crate) fn normalize_rust_analyzer_kind(kind: &str) -> Option<String> {
    match kind {
        "Module" => Some("Module".to_string()),
        "Struct" => Some("Struct".to_string()),
        "Enum" => Some("Enum".to_string()),
        "Union" => Some("Union".to_string()),
        "Trait" => Some("Trait".to_string()),
        "Impl" => Some("Impl".to_string()),
        "Function" => Some("Function".to_string()),
        "Method" => Some("Method".to_string()),
        "Const" => Some("Const".to_string()),
        "Static" => Some("Static".to_string()),
        "TypeAlias" => Some("TypeAlias".to_string()),
        "Macro" => Some("Macro".to_string()),
        _ => None,
    }
}

pub(crate) fn normalize_squeezy_kind(kind: SymbolKind) -> Option<String> {
    match kind {
        SymbolKind::Class => Some("Class".to_string()),
        SymbolKind::Interface => Some("Interface".to_string()),
        SymbolKind::Module => Some("Module".to_string()),
        SymbolKind::Struct => Some("Struct".to_string()),
        SymbolKind::Enum => Some("Enum".to_string()),
        SymbolKind::Union => Some("Union".to_string()),
        SymbolKind::Trait => Some("Trait".to_string()),
        SymbolKind::Impl => Some("Impl".to_string()),
        SymbolKind::Function | SymbolKind::Test => Some("Function".to_string()),
        SymbolKind::Method => Some("Method".to_string()),
        SymbolKind::Const => Some("Const".to_string()),
        SymbolKind::Static => Some("Static".to_string()),
        SymbolKind::TypeAlias => Some("TypeAlias".to_string()),
        SymbolKind::Macro => Some("Macro".to_string()),
        SymbolKind::Crate
        | SymbolKind::File
        | SymbolKind::Field
        | SymbolKind::Variant
        | SymbolKind::Unknown => None,
    }
}

pub(crate) fn normalize_c_family_squeezy_kind(kind: SymbolKind) -> Option<String> {
    match kind {
        SymbolKind::Class => Some("Class".to_string()),
        SymbolKind::Module => Some("Module".to_string()),
        SymbolKind::Struct => Some("Struct".to_string()),
        SymbolKind::Enum => Some("Enum".to_string()),
        SymbolKind::Union => Some("Union".to_string()),
        SymbolKind::Function | SymbolKind::Test => Some("Function".to_string()),
        SymbolKind::Method => Some("Method".to_string()),
        SymbolKind::TypeAlias => Some("TypeAlias".to_string()),
        // `Interface` is a Go concept and never produced by the C/C++
        // parser path, but the type system still needs an arm for it.
        SymbolKind::Crate
        | SymbolKind::Trait
        | SymbolKind::Impl
        | SymbolKind::Interface
        | SymbolKind::Const
        | SymbolKind::Static
        | SymbolKind::Macro
        | SymbolKind::File
        | SymbolKind::Field
        | SymbolKind::Variant
        | SymbolKind::Unknown => None,
    }
}

pub(crate) fn normalize_symbol_name(name: &str) -> String {
    trim_impl_header(&name.split_whitespace().collect::<Vec<_>>().join(" "))
}

pub(crate) fn trim_impl_header(raw: &str) -> String {
    let trimmed = raw.trim();
    let trimmed = trimmed.strip_prefix("unsafe ").unwrap_or(trimmed);
    let Some(rest) = trimmed.strip_prefix("impl") else {
        return trimmed.to_string();
    };
    let Some(next) = rest.chars().next() else {
        return trimmed.to_string();
    };
    if !next.is_whitespace() && next != '<' {
        return trimmed.to_string();
    }

    let mut rest = rest.trim_start();
    if rest.starts_with('<') {
        let mut depth = 0usize;
        let mut close_index = None;
        let mut previous = None;
        for (index, ch) in rest.char_indices() {
            match ch {
                '<' => depth += 1,
                '>' if previous != Some('-') => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        close_index = Some(index + ch.len_utf8());
                        break;
                    }
                }
                _ => {}
            }
            previous = Some(ch);
        }
        if let Some(index) = close_index {
            rest = rest[index..].trim_start();
        }
    }
    rest.split_once(" where ")
        .map(|(before, _)| before)
        .unwrap_or(rest)
        .trim_end_matches(',')
        .to_string()
}

pub(crate) fn compare_symbol_sets(squeezy: &SymbolScan, rust_analyzer: &SymbolScan) -> AccuracySetReport {
    let true_positive = squeezy
        .counts
        .iter()
        .map(|(key, count)| count.min(rust_analyzer.counts.get(key).unwrap_or(&0)))
        .sum::<usize>();
    let false_positive = count_difference(&squeezy.counts, &rust_analyzer.counts);
    let false_negative = count_difference(&rust_analyzer.counts, &squeezy.counts);
    let precision = ratio(true_positive, true_positive + false_positive);
    let recall = ratio(true_positive, true_positive + false_negative);

    AccuracySetReport {
        compared_kinds: vec![
            "Class".to_string(),
            "Interface".to_string(),
            "Module".to_string(),
            "Struct".to_string(),
            "Enum".to_string(),
            "Union".to_string(),
            "Trait".to_string(),
            "Impl".to_string(),
            "Function".to_string(),
            "Method".to_string(),
            "Const".to_string(),
            "Static".to_string(),
            "TypeAlias".to_string(),
            "Macro".to_string(),
        ],
        rust_analyzer_raw_total: rust_analyzer.raw_total,
        rust_analyzer_total: symbol_count(&rust_analyzer.counts),
        rust_analyzer_unique: rust_analyzer.counts.len(),
        rust_analyzer_excluded_by_kind: rust_analyzer.excluded_by_kind.clone(),
        rust_analyzer_skipped_non_utf8_files: rust_analyzer.skipped_non_utf8_files,
        squeezy_raw_total: squeezy.raw_total,
        squeezy_total: symbol_count(&squeezy.counts),
        squeezy_unique: squeezy.counts.len(),
        squeezy_excluded_by_kind: squeezy.excluded_by_kind.clone(),
        true_positive,
        false_positive,
        false_negative,
        precision,
        recall,
        false_positive_examples: difference_examples(&squeezy.counts, &rust_analyzer.counts),
        false_negative_examples: difference_examples(&rust_analyzer.counts, &squeezy.counts),
    }
}

pub(crate) fn collect_navigation_accuracy(
    root: &Path,
    graph: &SemanticGraph,
    probe_limit: usize,
) -> NavigationAccuracyReport {
    if probe_limit == 0 {
        return NavigationAccuracyReport {
            rust_analyzer_lsp_ms: None,
            rust_analyzer_lsp_status: "disabled by --ra-lsp-probes 0".to_string(),
            requested_probe_limit: probe_limit,
            definitions: DefinitionAccuracyReport::default(),
            references: ReferenceAccuracyReport::default(),
            limitations: navigation_limitations(),
        };
    }

    let started = Instant::now();
    let mut client = match RustAnalyzerLsp::start(root) {
        Ok(client) => client,
        Err(err) => {
            return NavigationAccuracyReport {
                rust_analyzer_lsp_ms: None,
                rust_analyzer_lsp_status: format!("rust-analyzer LSP unavailable: {err}"),
                requested_probe_limit: probe_limit,
                definitions: DefinitionAccuracyReport::default(),
                references: ReferenceAccuracyReport::default(),
                limitations: navigation_limitations(),
            };
        }
    };

    let definitions = compare_definition_probes(root, graph, &mut client, probe_limit);
    let references = compare_reference_probes(root, graph, &mut client, probe_limit);
    let elapsed = started.elapsed().as_millis();
    let status = match (&definitions, &references) {
        (Ok(_), Ok(_)) => "rust-analyzer LSP definition/reference probes succeeded".to_string(),
        (Err(err), _) => format!("rust-analyzer LSP definition probes failed: {err}"),
        (_, Err(err)) => format!("rust-analyzer LSP reference probes failed: {err}"),
    };

    NavigationAccuracyReport {
        rust_analyzer_lsp_ms: (definitions.is_ok() && references.is_ok()).then_some(elapsed),
        rust_analyzer_lsp_status: status,
        requested_probe_limit: probe_limit,
        definitions: definitions.unwrap_or_default(),
        references: references.unwrap_or_default(),
        limitations: navigation_limitations(),
    }
}

pub(crate) fn navigation_limitations() -> Vec<String> {
    vec![
        "Definition probes compare Squeezy resolved call and macro edge targets with rust-analyzer LSP definitions for sampled call sites.".to_string(),
        "Reference probes compare Squeezy references_to_symbol results with rust-analyzer LSP references for sampled declarations, excluding declarations because the selected symbol already supplies the definition span.".to_string(),
        "Samples are deterministic and capped; increase --ra-lsp-probes for deeper local audits.".to_string(),
        "External dependency definitions are counted as rust-analyzer-only misses because Squeezy currently indexes workspace files only.".to_string(),
    ]
}

fn compare_definition_probes(
    root: &Path,
    graph: &SemanticGraph,
    client: &mut RustAnalyzerLsp,
    probe_limit: usize,
) -> Result<DefinitionAccuracyReport> {
    let (available_probes, probes) = build_definition_probes(graph, probe_limit)?;
    let mut report = DefinitionAccuracyReport {
        available_probes,
        probes: probes.len(),
        ..DefinitionAccuracyReport::default()
    };

    for probe in probes {
        client.did_open(&probe.uri, &probe.path)?;
        let ra_locations = client.definition(&probe.uri, probe.position)?;
        let squeezy_has_target = probe.squeezy_target.is_some();
        let squeezy_matches = probe
            .squeezy_target
            .as_ref()
            .and_then(|id| graph.symbols.get(id))
            .map(|symbol| {
                ra_locations
                    .iter()
                    .any(|location| location_matches_symbol(root, graph, location, symbol))
            })
            .unwrap_or(false);

        match (ra_locations.is_empty(), squeezy_has_target, squeezy_matches) {
            (false, true, true) => report.true_positive += 1,
            (false, false, _) => {
                report.false_negative += 1;
                push_example(
                    &mut report.examples,
                    format!(
                        "FN definition {}: RA -> {}, Squeezy unresolved",
                        probe.label,
                        render_locations(&ra_locations)
                    ),
                );
            }
            (false, true, false) => {
                report.false_positive += 1;
                report.false_negative += 1;
                report.wrong_target += 1;
                push_example(
                    &mut report.examples,
                    format!(
                        "Wrong definition {}: RA -> {}, Squeezy -> {}",
                        probe.label,
                        render_locations(&ra_locations),
                        probe
                            .squeezy_target
                            .as_ref()
                            .map(|id| id.0.as_str())
                            .unwrap_or("<none>")
                    ),
                );
            }
            (true, true, false) => {
                report.false_positive += 1;
                report.squeezy_only += 1;
                push_example(
                    &mut report.examples,
                    format!(
                        "Squeezy-only definition {}: RA unresolved, Squeezy -> {}",
                        probe.label,
                        probe
                            .squeezy_target
                            .as_ref()
                            .map(|id| id.0.as_str())
                            .unwrap_or("<none>")
                    ),
                );
            }
            (true, false, _) => report.unresolved_agreement += 1,
            (true, true, true) => unreachable!("matched target requires an RA location"),
        }
    }

    report.precision = ratio(
        report.true_positive,
        report.true_positive + report.false_positive,
    );
    report.recall = ratio(
        report.true_positive,
        report.true_positive + report.false_negative,
    );
    Ok(report)
}

fn compare_reference_probes(
    root: &Path,
    graph: &SemanticGraph,
    client: &mut RustAnalyzerLsp,
    probe_limit: usize,
) -> Result<ReferenceAccuracyReport> {
    let (available_symbols, probes) = build_reference_probes(root, graph, probe_limit)?;
    let mut report = ReferenceAccuracyReport {
        available_symbols,
        symbols_sampled: probes.len(),
        ..ReferenceAccuracyReport::default()
    };

    for probe in probes {
        client.did_open(&probe.uri, &probe.path)?;
        let ra = client
            .references(&probe.uri, probe.position)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        let squeezy = graph
            .references_to_symbol(&probe.symbol_id)
            .into_iter()
            .filter_map(|hit| location_key_for_reference_hit(graph, &hit, &probe.name))
            .collect::<BTreeSet<_>>();

        let tp = squeezy.intersection(&ra).count();
        let fp = squeezy.difference(&ra).cloned().collect::<Vec<_>>();
        let fn_ = ra.difference(&squeezy).cloned().collect::<Vec<_>>();
        report.true_positive += tp;
        report.false_positive += fp.len();
        report.false_negative += fn_.len();

        if !fp.is_empty() {
            push_example(
                &mut report.false_positive_examples,
                format!(
                    "{} FP refs: {}",
                    probe.label,
                    fp.iter()
                        .take(5)
                        .map(LocationKey::render)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
        if !fn_.is_empty() {
            push_example(
                &mut report.false_negative_examples,
                format!(
                    "{} FN refs: {}",
                    probe.label,
                    fn_.iter()
                        .take(5)
                        .map(LocationKey::render)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
    }

    report.precision = ratio(
        report.true_positive,
        report.true_positive + report.false_positive,
    );
    report.recall = ratio(
        report.true_positive,
        report.true_positive + report.false_negative,
    );
    Ok(report)
}

pub(crate) fn build_definition_probes(
    graph: &SemanticGraph,
    limit: usize,
) -> Result<(usize, Vec<DefinitionProbe>)> {
    let mut probes = Vec::new();
    let mut edges = graph
        .edges()
        .iter()
        .filter(|edge| matches!(edge.kind, EdgeKind::Calls | EdgeKind::InvokesMacro))
        .filter_map(|edge| {
            let span = edge.span?;
            let from = graph.symbols.get(&edge.from)?;
            let file = graph.files.get(&from.file_id)?;
            Some((file.relative_path.clone(), span.start_byte, edge))
        })
        .collect::<Vec<_>>();
    edges.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.1.cmp(&right.1))
            .then(left.2.target_text.cmp(&right.2.target_text))
    });
    let available = edges.len();
    let selected = select_scenarios(available, limit);

    for index in selected {
        let (_, _, edge) = edges[index];
        let Some(span) = edge.span else {
            continue;
        };
        let Some(from) = graph.symbols.get(&edge.from) else {
            continue;
        };
        let Some(file) = graph.files.get(&from.file_id) else {
            continue;
        };
        let source = fs::read_to_string(&file.path)?;
        let byte = probe_byte_for_edge(
            &source,
            span.start_byte as usize,
            span.end_byte as usize,
            &edge.target_text,
        );
        let position = byte_to_lsp_position(&source, byte);
        probes.push(DefinitionProbe {
            label: format!(
                "{}:{}:{} {}",
                file.relative_path,
                position.line + 1,
                position.character + 1,
                edge.target_text
            ),
            uri: path_to_file_uri(&file.path)?,
            path: file.path.clone(),
            position,
            squeezy_target: edge.to.clone(),
        });
    }

    Ok((available, probes))
}

pub(crate) fn build_reference_probes(
    _root: &Path,
    graph: &SemanticGraph,
    limit: usize,
) -> Result<(usize, Vec<ReferenceProbe>)> {
    let mut symbols = graph
        .symbols
        .values()
        .filter(|symbol| {
            matches!(
                symbol.kind,
                SymbolKind::Struct
                    | SymbolKind::Enum
                    | SymbolKind::Union
                    | SymbolKind::Trait
                    | SymbolKind::Function
                    | SymbolKind::Method
                    | SymbolKind::TypeAlias
                    | SymbolKind::Const
                    | SymbolKind::Static
                    | SymbolKind::Macro
            ) && symbol.name.len() >= 3
        })
        .collect::<Vec<_>>();
    symbols.sort_by(|left, right| {
        left.file_id
            .0
            .cmp(&right.file_id.0)
            .then(left.span.start_byte.cmp(&right.span.start_byte))
            .then(left.name.cmp(&right.name))
    });
    let available = symbols.len();
    let selected = select_scenarios(available, limit);

    let mut probes = Vec::new();
    for index in selected {
        let symbol = symbols[index];
        let Some(file) = graph.files.get(&symbol.file_id) else {
            continue;
        };
        let source = fs::read_to_string(&file.path)?;
        let byte = probe_byte_for_symbol(
            &source,
            symbol.span.start_byte as usize,
            symbol.span.end_byte as usize,
            &symbol.name,
        );
        let position = byte_to_lsp_position(&source, byte);
        probes.push(ReferenceProbe {
            label: format!(
                "{}:{}:{} {}",
                file.relative_path,
                position.line + 1,
                position.character + 1,
                symbol.name
            ),
            uri: path_to_file_uri(&file.path)?,
            path: file.path.clone(),
            position,
            symbol_id: symbol.id.clone(),
            name: symbol.name.clone(),
        });
    }

    Ok((available, probes))
}

pub(crate) fn location_key_for_reference_hit(
    graph: &SemanticGraph,
    hit: &squeezy_graph::ReferenceHit,
    name: &str,
) -> Option<LocationKey> {
    let file = graph.files.get(&hit.reference.file_id)?;
    let source = fs::read_to_string(&file.path).ok()?;
    let start = hit.reference.span.start_byte as usize;
    let end = (hit.reference.span.end_byte as usize).min(source.len());
    let slice = source.get(start.min(end)..end).unwrap_or_default();
    let byte = slice
        .find(name)
        .map(|index| start + index)
        .unwrap_or(hit.reference.span.start_byte as usize);
    let position = byte_to_lsp_position(&source, byte);
    Some(LocationKey {
        file: file.relative_path.clone(),
        line: position.line,
        character: position.character,
    })
}

pub(crate) fn probe_byte_for_edge(source: &str, start: usize, end: usize, target_text: &str) -> usize {
    let end = end.min(source.len());
    let start = start.min(end);
    let slice = source.get(start..end).unwrap_or_default();
    let needle = target_identifier(target_text);
    slice
        .rfind(&needle)
        .map(|index| start + index)
        .unwrap_or(start)
}

pub(crate) fn probe_byte_for_symbol(source: &str, start: usize, end: usize, name: &str) -> usize {
    let end = end.min(source.len());
    let start = start.min(end);
    let slice = source.get(start..end).unwrap_or_default();
    let needle = target_identifier(name);
    slice
        .find(&needle)
        .map(|index| start + index)
        .unwrap_or(start)
}

pub(crate) fn target_identifier(text: &str) -> String {
    let before_bang = text.split('!').next().unwrap_or(text);
    let before_call = before_bang.split('(').next().unwrap_or(before_bang);
    before_call
        .rsplit(|ch| ['.', ':', '<', '>', '&', ' ', '\t', '\n'].contains(&ch))
        .find(|part| !part.is_empty())
        .unwrap_or(before_call)
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .to_string()
}

pub(crate) fn location_matches_symbol(
    root: &Path,
    graph: &SemanticGraph,
    location: &LocationKey,
    symbol: &squeezy_graph::GraphSymbol,
) -> bool {
    let Some(file) = graph.files.get(&symbol.file_id) else {
        return false;
    };
    if location.file != file.relative_path {
        return false;
    }
    let Ok(source) = fs::read_to_string(root.join(&file.relative_path)) else {
        return false;
    };
    line_char_to_byte(&source, location.line, location.character)
        .map(|byte| symbol.span.contains_byte(byte as u32))
        .unwrap_or(false)
}

pub(crate) fn render_locations(locations: &[LocationKey]) -> String {
    locations
        .iter()
        .take(5)
        .map(LocationKey::render)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn push_example(examples: &mut Vec<String>, example: String) {
    if examples.len() < 20 {
        examples.push(example);
    }
}

pub(crate) fn merge_symbol_scan(target: &mut SymbolScan, source: SymbolScan) {
    target.raw_total += source.raw_total;
    target.skipped_non_utf8_files += source.skipped_non_utf8_files;
    for (key, count) in source.counts {
        *target.counts.entry(key).or_default() += count;
    }
    for (kind, count) in source.excluded_by_kind {
        *target.excluded_by_kind.entry(kind).or_default() += count;
    }
}

pub(crate) fn increment_symbol(counts: &mut BTreeMap<SymbolKey, usize>, key: SymbolKey) {
    *counts.entry(key).or_default() += 1;
}

pub(crate) fn increment_unique_symbol(counts: &mut BTreeMap<SymbolKey, usize>, key: SymbolKey) {
    counts.entry(key).or_insert(1);
}

pub(crate) fn symbol_count(counts: &BTreeMap<SymbolKey, usize>) -> usize {
    counts.values().sum()
}

pub(crate) fn count_difference(
    left: &BTreeMap<SymbolKey, usize>,
    right: &BTreeMap<SymbolKey, usize>,
) -> usize {
    left.iter()
        .map(|(key, count)| count.saturating_sub(*right.get(key).unwrap_or(&0)))
        .sum()
}

pub(crate) fn difference_examples(
    left: &BTreeMap<SymbolKey, usize>,
    right: &BTreeMap<SymbolKey, usize>,
) -> Vec<String> {
    left.iter()
        .filter_map(|(key, count)| {
            let extra = count.saturating_sub(*right.get(key).unwrap_or(&0));
            match extra {
                0 => None,
                1 => Some(key.render()),
                _ => Some(format!("{} x{}", key.render(), extra)),
            }
        })
        .take(20)
        .collect()
}

pub(crate) fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        1.0
    } else {
        ((numerator as f64 / denominator as f64) * 10_000.0).round() / 10_000.0
    }
}

#[derive(Debug, Clone)]
pub(crate) enum MixedScenario {
    HierarchyAll {
        depth: usize,
    },
    HierarchyRoot {
        root: SymbolId,
        depth: usize,
    },
    SymbolLookup {
        name: String,
    },
    SignatureSearch {
        text: String,
        kind: Option<SymbolKind>,
    },
    BodySearch {
        text: String,
        hit_kind: Option<BodyHitKind>,
    },
    ReferenceSearch {
        text: String,
    },
    ReferencesToSymbol {
        symbol: SymbolId,
    },
    Callees {
        symbol: SymbolId,
    },
    Callers {
        symbol: SymbolId,
    },
    CallChain {
        from: SymbolId,
        to: SymbolId,
    },
}

impl MixedScenario {
    pub(crate) fn tool(&self) -> &'static str {
        match self {
            MixedScenario::HierarchyAll { .. } | MixedScenario::HierarchyRoot { .. } => "hierarchy",
            MixedScenario::SymbolLookup { .. } => "symbol_lookup",
            MixedScenario::SignatureSearch { .. } => "signature_search",
            MixedScenario::BodySearch { .. } => "body_search",
            MixedScenario::ReferenceSearch { .. } => "reference_search",
            MixedScenario::ReferencesToSymbol { .. } => "references_to_symbol",
            MixedScenario::Callees { .. } => "callees",
            MixedScenario::Callers { .. } => "callers",
            MixedScenario::CallChain { .. } => "call_chain",
        }
    }
}

pub(crate) fn build_mixed_scenarios(graph: &SemanticGraph) -> Vec<MixedScenario> {
    let mut symbols = graph
        .symbols
        .values()
        .filter(|symbol| !symbol.name.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    symbols.sort_by(|left, right| {
        format!("{:?}", left.kind)
            .cmp(&format!("{:?}", right.kind))
            .then(left.name.cmp(&right.name))
            .then(left.file_id.0.cmp(&right.file_id.0))
            .then(left.span.start_byte.cmp(&right.span.start_byte))
    });

    let names = symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| symbol.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let mut scenarios = Vec::new();
    for depth in [1, 2, 4, 8, 16] {
        scenarios.push(MixedScenario::HierarchyAll { depth });
    }

    for symbol in &symbols {
        if symbol.kind == SymbolKind::File {
            scenarios.push(MixedScenario::HierarchyRoot {
                root: symbol.id.clone(),
                depth: 4,
            });
            continue;
        }

        scenarios.push(MixedScenario::SymbolLookup {
            name: symbol.name.clone(),
        });
        scenarios.push(MixedScenario::SignatureSearch {
            text: symbol.name.clone(),
            kind: None,
        });
        scenarios.push(MixedScenario::SignatureSearch {
            text: symbol.name.clone(),
            kind: Some(symbol.kind),
        });
        scenarios.push(MixedScenario::Callees {
            symbol: symbol.id.clone(),
        });
        scenarios.push(MixedScenario::Callers {
            symbol: symbol.id.clone(),
        });
        scenarios.push(MixedScenario::ReferencesToSymbol {
            symbol: symbol.id.clone(),
        });
    }

    for name in &names {
        scenarios.push(MixedScenario::ReferenceSearch { text: name.clone() });
        scenarios.push(MixedScenario::BodySearch {
            text: name.clone(),
            hit_kind: None,
        });
        for hit_kind in [
            BodyHitKind::Identifier,
            BodyHitKind::Type,
            BodyHitKind::Path,
            BodyHitKind::Call,
            BodyHitKind::Macro,
        ] {
            scenarios.push(MixedScenario::BodySearch {
                text: name.clone(),
                hit_kind: Some(hit_kind),
            });
        }
    }

    for edge in graph.edges() {
        if matches!(edge.kind, EdgeKind::Calls | EdgeKind::InvokesMacro)
            && let Some(to) = &edge.to
        {
            scenarios.push(MixedScenario::CallChain {
                from: edge.from.clone(),
                to: to.clone(),
            });
        }
    }

    scenarios
}

pub(crate) fn select_scenarios(available: usize, requested: usize) -> Vec<usize> {
    if requested == 0 || requested >= available {
        return (0..available).collect();
    }

    let mut rng = DeterministicRng::new(0x5eed_5eed_51ee_ee55_u64);
    let mut selected = BTreeSet::new();
    while selected.len() < requested {
        selected.insert(rng.next_usize(available));
    }
    selected.into_iter().collect()
}

pub(crate) fn run_mixed_scenario(graph: &SemanticGraph, scenario: &MixedScenario) -> usize {
    match scenario {
        MixedScenario::HierarchyAll { depth } => graph.hierarchy(None, *depth).len(),
        MixedScenario::HierarchyRoot { root, depth } => graph.hierarchy(Some(root), *depth).len(),
        MixedScenario::SymbolLookup { name } => graph.find_symbol_by_name(name).len(),
        MixedScenario::SignatureSearch { text, kind } => graph
            .signature_search(&SignatureQuery {
                text: text.clone(),
                kind: *kind,
                visibility: None,
                attribute: None,
            })
            .len(),
        MixedScenario::BodySearch { text, hit_kind } => graph
            .body_search(&BodySearchQuery {
                text: text.clone(),
                owner_kind: None,
                hit_kind: *hit_kind,
            })
            .len(),
        MixedScenario::ReferenceSearch { text } => graph.reference_search(text).len(),
        MixedScenario::ReferencesToSymbol { symbol } => graph.references_to_symbol(symbol).len(),
        MixedScenario::Callees { symbol } => graph.callees(symbol).len(),
        MixedScenario::Callers { symbol } => graph.callers(symbol).len(),
        MixedScenario::CallChain { from, to } => graph
            .call_chain(from, to, 8)
            .map(|chain| chain.len())
            .unwrap_or_default(),
    }
}

pub(crate) fn run_refresh_probe(repo: &Path, language: BenchmarkLanguage) -> Result<RefreshProbeReport> {
    let source_snapshot = WorkspaceCrawler::new(CrawlOptions::default()).crawl(repo)?;
    let temp_root = temp_dir("squeezy-refresh-probe")?;
    let mut copied = Vec::new();
    for record in source_snapshot
        .files
        .iter()
        .filter(|record| record.language == language.language_kind())
        .take(250)
    {
        let dest = temp_root.join(&record.relative_path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&record.path, &dest)?;
        copied.push(dest);
    }

    // The probe creates a synthetic tree of just-source files, so the
    // workspace indexing-signal check can fail (no Cargo.toml/pom.xml/etc.
    // gets copied alongside the source files). Disable the signal
    // requirement for the probe so refresh always walks the temp tree.
    let crawl_options = CrawlOptions {
        require_indexing_signal: false,
        ..CrawlOptions::default()
    };
    let mut manager = GraphManager::open_with_crawl_options(
        &temp_root,
        RefreshConfig {
            debounce: std::time::Duration::from_millis(0),
            idle_refresh_interval: std::time::Duration::from_millis(0),
            per_tool_refresh_budget: std::time::Duration::from_secs(10),
        },
        crawl_options,
    )?;

    let edits = copied.iter().take(2).cloned().collect::<Vec<_>>();
    for path in &edits {
        let mut text = fs::read_to_string(path)?;
        text.push_str(language.comment_text());
        fs::write(path, text)?;
        manager.record_changed_path(path.clone());
    }

    let refresh_started = Instant::now();
    let report = manager.refresh_before_query()?;
    let refresh_ms = refresh_started.elapsed().as_millis();
    fs::remove_dir_all(&temp_root)?;

    Ok(RefreshProbeReport {
        language: language.as_str().to_string(),
        copied_source_files: copied.len(),
        edited_files: edits.len(),
        refresh_ms,
        reparsed_files: report.reparsed_files,
        changed_files: report.changed_files.len(),
        changed_paths_from_events: report.changed_paths_from_events,
        changed_paths_from_polling: report.changed_paths_from_polling,
        unchanged_event_paths: report.unchanged_event_paths,
        budget_exhausted: report.budget_exhausted,
    })
}

