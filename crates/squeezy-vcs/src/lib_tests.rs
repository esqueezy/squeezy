use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use super::*;

static VCS_NONCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn parses_patch_hunks_as_zero_based_line_ranges() {
    let patch = "@@ -1,2 +1,3 @@\n-a\n+b\n+c\n@@ -10 +12,2 @@\n";
    let hunks = parse_patch_hunks(patch);
    assert_eq!(hunks.len(), 2);
    assert_eq!(hunks[0].start_line, 0);
    assert_eq!(hunks[0].end_line, 2);
    assert_eq!(hunks[1].start_line, 11);
    assert_eq!(hunks[1].end_line, 12);
}

#[test]
fn parses_numstat_with_binary_counts() {
    let parsed = parse_numstat(b"2\t3\tsrc/lib.rs\0-\t-\timage.png\0");
    assert_eq!(parsed["src/lib.rs"].additions, 2);
    assert_eq!(parsed["src/lib.rs"].deletions, 3);
    assert!(parsed["image.png"].binary);
}

#[test]
fn branch_mode_snapshot_reports_files_changed_since_default_branch() {
    let root = temp_repo("branch_mode");
    init_repo(&root);
    fs::write(root.join("base.txt"), "base\n").expect("write base");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "initial"]);
    git(&root, &["checkout", "-b", "feature"]);
    fs::write(root.join("feature.txt"), "feature\n").expect("write feature");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "feature work"]);

    let vcs = GitVcs::open(&root).expect("open vcs");
    let snapshot = vcs.snapshot(DiffMode::Branch, DiffOptions::default());

    assert_eq!(snapshot.mode, DiffMode::Branch);
    assert_eq!(snapshot.vcs.kind, VcsKind::Git);
    assert_eq!(snapshot.vcs.branch.as_deref(), Some("feature"));
    assert!(
        snapshot
            .vcs
            .default_branch
            .as_deref()
            .is_some_and(|name| name == "main" || name == "master"),
        "expected main or master, got {:?}",
        snapshot.vcs.default_branch
    );
    assert!(snapshot.vcs.merge_base.is_some());

    let paths = snapshot
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["feature.txt"]);
    assert_eq!(snapshot.files[0].status, DiffFileStatus::Added);
    assert_eq!(snapshot.summary.files_changed, 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn checkpoint_rollback_restores_modified_added_and_deleted_files() {
    let root = temp_repo("checkpoint_restore");
    fs::write(root.join("a.txt"), "A\n").expect("write a");
    fs::write(root.join("b.txt"), "B\n").expect("write b");
    let store = CheckpointStore::open(&root).expect("checkpoint store");
    let before = store.track_tree().expect("track before");

    fs::write(root.join("a.txt"), "A2\n").expect("modify a");
    fs::write(root.join("c.txt"), "C\n").expect("write c");
    fs::remove_file(root.join("b.txt")).expect("remove b");
    let record = store
        .create_checkpoint(&before, "shell", "call", "turn-1", "success")
        .expect("create checkpoint")
        .expect("checkpoint");

    assert_eq!(record.summary.files_changed, 3);
    let rollback = store
        .rollback(RollbackTarget::Latest)
        .expect("rollback latest");

    assert!(rollback.conflicts.is_empty());
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "A\n");
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "B\n");
    assert!(!root.join("c.txt").exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn checkpoint_rollback_reports_conflicts_without_overwriting_user_changes() {
    let root = temp_repo("checkpoint_conflict");
    fs::write(root.join("a.txt"), "A\n").expect("write a");
    let store = CheckpointStore::open(&root).expect("checkpoint store");
    let before = store.track_tree().expect("track before");

    fs::write(root.join("a.txt"), "agent\n").expect("agent edit");
    store
        .create_checkpoint(&before, "write_file", "call", "turn-1", "success")
        .expect("create checkpoint")
        .expect("checkpoint");
    fs::write(root.join("a.txt"), "user\n").expect("user edit");

    let rollback = store
        .rollback(RollbackTarget::Latest)
        .expect("rollback latest");

    assert_eq!(rollback.conflicts.len(), 1);
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "user\n");

    let _ = fs::remove_dir_all(root);
}

fn temp_repo(name: &str) -> PathBuf {
    let base = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let counter = VCS_NONCE.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "squeezy_vcs_{name}_{pid}_{base}_{counter}",
        pid = std::process::id()
    ));
    fs::create_dir_all(&root).expect("create temp repo");
    root
}

fn init_repo(root: &Path) {
    git(root, &["init", "--initial-branch=main"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "user.name", "Squeezy Test"]);
    git(root, &["config", "commit.gpgsign", "false"]);
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}
