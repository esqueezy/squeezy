use super::*;

#[test]
fn script_run_detection_matches_runner_plus_extension() {
    let tokens = vec![
        "python3".to_string(),
        "-u".to_string(),
        "scripts/fetch_comments.py".to_string(),
    ];
    assert_eq!(script_run_token(&tokens), Some("scripts/fetch_comments.py"));
}

#[test]
fn script_run_detection_excludes_python_c() {
    let tokens = vec![
        "python3".to_string(),
        "-c".to_string(),
        "print(1)".to_string(),
    ];
    assert_eq!(script_run_token(&tokens), None);
}

#[test]
fn tokenizer_preserves_quoted_paths() {
    let tokens = tokenize_command("python3 \"scripts/my tool.py\"");
    assert_eq!(tokens, vec!["python3", "scripts/my tool.py"]);
}

#[test]
fn doc_prefilter_rejects_unrelated_reader_tokens() {
    let mut doc_filenames = BTreeSet::new();
    doc_filenames.insert("skill.md".to_string());

    assert!(!doc_token_may_match_indexed_path("a.rs", &doc_filenames));
    assert!(!doc_token_may_match_indexed_path(
        "README.md",
        &doc_filenames
    ));
}

#[test]
fn doc_prefilter_keeps_plausible_skill_doc_tokens() {
    let mut doc_filenames = BTreeSet::new();
    doc_filenames.insert("skill.md".to_string());

    // SKILL.md always matches via the early-return fast path.
    assert!(doc_token_may_match_indexed_path("SKILL.md", &doc_filenames));
    assert!(doc_token_may_match_indexed_path(
        ".squeezy/skills/nav/SKILL.md",
        &doc_filenames
    ));
}

#[test]
fn doc_prefilter_keeps_skill_doc_tokens_when_canonical_target_differs() {
    // Even when the indexed path uses a different name, SKILL.md tokens
    // should still pass via the fast-path early return.
    let doc_filenames = BTreeSet::new();

    assert!(doc_token_may_match_indexed_path(
        ".squeezy/skills/nav/SKILL.md",
        &doc_filenames
    ));
}

#[test]
fn doc_prefilter_matches_non_skill_doc_by_filename() {
    let mut doc_filenames = BTreeSet::new();
    doc_filenames.insert("guide.md".to_string());

    assert!(doc_token_may_match_indexed_path("guide.md", &doc_filenames));
    assert!(doc_token_may_match_indexed_path(
        "skills/guide.md",
        &doc_filenames
    ));
    // Case-insensitive matching.
    assert!(doc_token_may_match_indexed_path("GUIDE.MD", &doc_filenames));
    assert!(!doc_token_may_match_indexed_path(
        "other.md",
        &doc_filenames
    ));
}

#[test]
fn powershell_readers_trigger_doc_read_detection() {
    assert!(command_reads_file(&[
        "Get-Content".to_string(),
        "SKILL.md".to_string()
    ]));
    assert!(command_reads_file(&[
        "gc".to_string(),
        "SKILL.md".to_string()
    ]));
    assert!(command_reads_file(&[
        "type".to_string(),
        "SKILL.md".to_string()
    ]));
    // Non-reader should not match.
    assert!(!command_reads_file(&[
        "Invoke-WebRequest".to_string(),
        "SKILL.md".to_string()
    ]));
}

#[cfg(not(windows))]
#[test]
fn tokenizer_treats_backslash_as_escape_on_unix() {
    // On Unix, backslash escapes the next character.
    let tokens = tokenize_command("cat foo\\ bar.txt");
    assert_eq!(tokens, vec!["cat", "foo bar.txt"]);
}

#[cfg(windows)]
#[test]
fn tokenizer_preserves_windows_path_separators() {
    // On Windows, backslash is a path separator, not an escape character.
    let tokens = tokenize_command(r"pwsh -File .\.squeezy\skills\nav\scripts\init.ps1");
    assert_eq!(tokens.len(), 3);
    assert_eq!(tokens[0], "pwsh");
    assert_eq!(tokens[1], "-File");
    assert_eq!(tokens[2], r".\.squeezy\skills\nav\scripts\init.ps1");
}

#[cfg(windows)]
#[test]
fn tokenizer_preserves_absolute_windows_path() {
    let tokens = tokenize_command(r#"pwsh -File "C:\Users\alice\SKILL.md""#);
    assert_eq!(tokens.len(), 3);
    assert_eq!(tokens[2], r"C:\Users\alice\SKILL.md");
}
