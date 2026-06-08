use std::collections::HashSet;

use squeezy_core::{ContentHash, FileId, Freshness, LanguageFamily, LanguageKind};
use squeezy_parse::backend;
use squeezy_workspace::{FileRecord, stable_content_hash};

#[test]
fn every_language_family_has_one_backend() {
    let mut families = HashSet::new();
    for backend in backend::inventory() {
        assert!(
            families.insert(backend.family()),
            "duplicate parse backend for {:?}",
            backend.family()
        );
    }

    for family in LanguageFamily::all() {
        assert!(
            families.contains(family),
            "missing parse backend for {family:?}"
        );
    }
}

#[test]
fn every_supported_language_kind_maps_to_a_backend() {
    // Iterate every family (and every kind it advertises) so a newly-added
    // family or LanguageKind is automatically covered without editing this test.
    for family in LanguageFamily::all() {
        for &kind in family.kinds() {
            let backend = backend::backend_for_kind(kind)
                .unwrap_or_else(|| panic!("missing parse backend for {kind:?}"));
            assert_eq!(
                backend.family(),
                *family,
                "kind {kind:?} routed to the wrong backend family"
            );
            assert!(
                backend.kinds().contains(&kind),
                "backend {:?} does not advertise {kind:?}",
                backend.family()
            );
            assert!(
                backend.tree_sitter_language(kind).is_some(),
                "backend {:?} does not expose a tree-sitter language for {kind:?}",
                backend.family()
            );
            let mut parser = backend.parser(kind).unwrap_or_else(|err| {
                panic!(
                    "backend {:?} could not instantiate parser for {kind:?}: {err}",
                    backend.family()
                )
            });
            let source = minimal_source(kind);
            let tree = parser.parse(source, None).unwrap_or_else(|| {
                panic!(
                    "backend {:?} returned no tree for {kind:?}",
                    backend.family()
                )
            });
            assert!(
                !tree.root_node().has_error(),
                "minimal {kind:?} fixture should parse without tree-sitter errors"
            );
            let parsed = backend.extract(fixture_record(kind, source), source, &tree);
            assert!(
                parsed.unsupported.is_none(),
                "backend {:?} should extract supported {kind:?} fixtures",
                backend.family()
            );
        }
    }

    assert!(backend::backend_for_kind(LanguageKind::Unsupported).is_none());
    assert!(backend::backend_for_kind(LanguageKind::Unknown).is_none());
}

#[test]
fn every_family_file_extension_classifies_to_that_family() {
    // Iterate `LanguageFamily::all()` so a newly-registered extension is
    // automatically exercised, guarding against extension-classification drift.
    for family in LanguageFamily::all() {
        for &extension in family.file_extensions() {
            let kind = LanguageKind::from_extension(extension);
            assert_eq!(
                kind.family(),
                Some(*family),
                "extension {extension:?} of {family:?} classified as {kind:?}"
            );
            assert!(
                backend::backend_for_kind(kind).is_some(),
                "no backend for {kind:?} (extension {extension:?})"
            );
        }
    }
}

fn minimal_source(kind: LanguageKind) -> &'static str {
    match kind {
        LanguageKind::C => "int main(void) { return 0; }\n",
        LanguageKind::CSharp => "class Main { void Run() {} }\n",
        LanguageKind::Cpp => "int main() { return 0; }\n",
        LanguageKind::Dart => "void main() {}\n",
        LanguageKind::Go => "package main\nfunc main() {}\n",
        LanguageKind::Java => "class Main { void run() {} }\n",
        LanguageKind::JavaScript => "function run() {}\n",
        LanguageKind::Jsx => "const element = <div />;\n",
        LanguageKind::Kotlin => "fun main() {}\n",
        LanguageKind::Php => "<?php function run() {}\n",
        LanguageKind::Python => "def run():\n    pass\n",
        LanguageKind::Ruby => "def run\nend\n",
        LanguageKind::Rust => "fn main() {}\n",
        LanguageKind::Scala => "object Main { def run(): Unit = {} }\n",
        LanguageKind::Swift => "func run() {}\n",
        LanguageKind::TypeScript => "function run(): void {}\n",
        LanguageKind::Tsx => "const element = <div />;\n",
        LanguageKind::Unsupported | LanguageKind::Unknown => {
            unreachable!("Unsupported and Unknown are not in any LanguageFamily::kinds() iteration")
        }
    }
}

fn fixture_record(kind: LanguageKind, source: &str) -> FileRecord {
    let relative_path = format!("fixture.{}", extension_for_kind(kind));
    FileRecord {
        id: FileId::new(&relative_path),
        path: relative_path.clone().into(),
        relative_path,
        hash: ContentHash::new(stable_content_hash(source.as_bytes())),
        size_bytes: source.len() as u64,
        modified_unix_millis: 0,
        language: kind,
        freshness: Freshness::Fresh,
    }
}

fn extension_for_kind(kind: LanguageKind) -> &'static str {
    match kind {
        LanguageKind::C => "c",
        LanguageKind::CSharp => "cs",
        LanguageKind::Cpp => "cpp",
        LanguageKind::Dart => "dart",
        LanguageKind::Go => "go",
        LanguageKind::Java => "java",
        LanguageKind::JavaScript => "js",
        LanguageKind::Jsx => "jsx",
        LanguageKind::Kotlin => "kt",
        LanguageKind::Php => "php",
        LanguageKind::Python => "py",
        LanguageKind::Ruby => "rb",
        LanguageKind::Rust => "rs",
        LanguageKind::Scala => "scala",
        LanguageKind::Swift => "swift",
        LanguageKind::TypeScript => "ts",
        LanguageKind::Tsx => "tsx",
        LanguageKind::Unsupported | LanguageKind::Unknown => {
            unreachable!("Unsupported and Unknown are not in any LanguageFamily::kinds() iteration")
        }
    }
}
