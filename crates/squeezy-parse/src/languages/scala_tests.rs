use super::*;

use crate::scala_language;

fn scala_record(relative_path: &str, source: &str) -> FileRecord {
    FileRecord {
        id: FileId::new(relative_path),
        path: std::path::PathBuf::from(relative_path),
        relative_path: relative_path.to_string(),
        hash: ContentHash::new("0"),
        size_bytes: source.len() as u64,
        modified_unix_millis: 0,
        language: LanguageKind::Scala,
        freshness: Freshness::Fresh,
    }
}

fn parse_scala(source: &str) -> ParsedFile {
    let mut parser = Parser::new();
    parser
        .set_language(&scala_language())
        .expect("load scala grammar");
    let tree = parser.parse(source, None).expect("parse scala source");
    extract_scala(scala_record("Main.scala", source), source, &tree)
}

#[test]
fn keeps_nested_qualified_path_prefix_reference() {
    // A 3+ segment qualified selection `a.b.c` parses as nested
    // `field_expression` nodes whose left-nested prefix `a.b` shares the outer
    // node's start byte. Deduping references on (start_byte, kind) alone
    // collapses the distinct prefix; the text-aware key keeps both.
    let parsed = parse_scala("object Main { val x = a.b.c }");

    let field_refs: Vec<&str> = parsed
        .references
        .iter()
        .filter(|reference| reference.kind == ReferenceKind::Field)
        .map(|reference| reference.text.as_str())
        .collect();
    assert!(
        field_refs.contains(&"a.b.c"),
        "expected full qualified selection reference, got {field_refs:?}"
    );
    assert!(
        field_refs.contains(&"a.b"),
        "expected nested qualified-prefix reference, got {field_refs:?}"
    );

    let path_hits: Vec<&str> = parsed
        .body_hits
        .iter()
        .filter(|hit| hit.kind == BodyHitKind::Path)
        .map(|hit| hit.text.as_str())
        .collect();
    assert!(
        path_hits.contains(&"a.b.c"),
        "expected full qualified selection path body hit, got {path_hits:?}"
    );
    assert!(
        path_hits.contains(&"a.b"),
        "expected nested qualified-prefix path body hit, got {path_hits:?}"
    );
}
