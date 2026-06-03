use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use squeezy_core::{ContentHash, FileId, LanguageKind};
use squeezy_parse::LanguageParser;
use squeezy_workspace::{FileRecord, stable_content_hash};

use super::*;

fn temp_root() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("squeezy-refcache-{pid}-{counter}-{nonce}"));
    fs::create_dir_all(&root).unwrap();
    root
}

fn rust_record(relative_path: &str, source: &str) -> FileRecord {
    let root = temp_root();
    let path = root.join(relative_path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, source).unwrap();
    FileRecord {
        id: FileId::new(relative_path),
        path,
        relative_path: relative_path.to_string(),
        hash: ContentHash::new(stable_content_hash(source.as_bytes())),
        size_bytes: source.len() as u64,
        modified_unix_millis: 0,
        language: LanguageKind::Rust,
        freshness: Freshness::Fresh,
    }
}

/// A symbol referenced many times in a single file must trigger at most one
/// `read_to_string` of that file during the binding pass, and the result must
/// be identical to the public `references_to_symbol` path.
#[test]
fn references_to_symbol_reads_each_file_once() {
    let source = r#"
fn run() {}

fn a() { run(); }
fn b() { run(); }
fn c() { run(); }
fn d() { run(); }
fn e() { run(); }
"#;
    let mut parser = LanguageParser::new().unwrap();
    let record = rust_record("src/lib.rs", source);
    let parsed = parser.parse_source(&record, source.to_string()).unwrap();
    let graph = SemanticGraph::from_parsed(vec![parsed]);

    let run = graph.find_symbol_by_name("run").pop().unwrap();

    let mut sources = SourceCache::default();
    let hits = graph.references_to_symbol_with_cache(&run.id, &mut sources);

    // The symbol is referenced five times in this one file, but the binding
    // pass must read the file at most once, not once per candidate.
    assert!(hits.len() >= 5, "expected the five call sites to bind");
    assert_eq!(
        sources.reads(),
        1,
        "single-file query must read source at most once, got {} reads",
        sources.reads()
    );

    // The cached path must agree with the public, fresh-cache path.
    let public = graph.references_to_symbol(&run.id);
    assert_eq!(hits, public);
}
