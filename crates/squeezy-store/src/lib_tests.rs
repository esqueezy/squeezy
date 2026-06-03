use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use redb::{Database, TableDefinition};
use serde_json::json;
use squeezy_core::AppConfig;
use squeezy_core::FileId;

use crate::{CompactionCheckpoint, GraphWriteBatch, SqueezyStore, sessions::ResumeItem};

fn temp_root(label: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "squeezy-store-tests-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("create temp root");
    path
}

fn open_store(label: &str) -> (PathBuf, SqueezyStore) {
    let root = temp_root(label);
    let config = AppConfig {
        workspace_root: root.clone(),
        ..AppConfig::default()
    };
    let store = SqueezyStore::open(&config.workspace_root, None).expect("open store");
    (root, store)
}

fn sample_checkpoint(replacement_id: &str, created: u128) -> CompactionCheckpoint {
    CompactionCheckpoint {
        replacement_id: replacement_id.to_string(),
        session_id: "sess-1".to_string(),
        generation: 4,
        items: vec![
            ResumeItem::UserText {
                text: "first user turn".to_string(),
            },
            ResumeItem::AssistantText {
                text: "first assistant reply".to_string(),
            },
        ],
        created_unix_millis: created,
    }
}

#[test]
fn compaction_checkpoint_round_trip() {
    let (_root, store) = open_store("ckpt-roundtrip");
    let checkpoint = sample_checkpoint("ckpt-1", 1_000);
    store
        .put_compaction_checkpoint(&checkpoint)
        .expect("put checkpoint");
    let loaded = store
        .get_compaction_checkpoint("ckpt-1")
        .expect("get checkpoint")
        .expect("checkpoint present");
    assert_eq!(loaded, checkpoint);
}

#[test]
fn compaction_checkpoint_missing_id_returns_none() {
    let (_root, store) = open_store("ckpt-missing");
    let loaded = store
        .get_compaction_checkpoint("does-not-exist")
        .expect("get checkpoint");
    assert!(loaded.is_none());
}

#[test]
fn compaction_checkpoint_prune_drops_old_only() {
    let (_root, store) = open_store("ckpt-prune");
    let old = sample_checkpoint("ckpt-old", 100);
    let fresh = sample_checkpoint("ckpt-fresh", 1_000);
    store.put_compaction_checkpoint(&old).expect("put old");
    store.put_compaction_checkpoint(&fresh).expect("put fresh");
    let removed = store
        .prune_compaction_checkpoints(500)
        .expect("prune older than 500");
    assert_eq!(removed, 1);
    assert!(
        store
            .get_compaction_checkpoint("ckpt-old")
            .expect("get old")
            .is_none(),
        "old checkpoint should be pruned",
    );
    assert!(
        store
            .get_compaction_checkpoint("ckpt-fresh")
            .expect("get fresh")
            .is_some(),
        "fresh checkpoint should remain",
    );
}

/// Stamp `version` into the `state.redb` `meta` table so re-opening hits the
/// schema-mismatch reset path.
fn write_schema_version(path: &Path, version: u64) {
    const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
    let database = Database::create(path).expect("create database");
    let write = database.begin_write().expect("begin write");
    {
        let mut table = write.open_table(META).expect("open meta");
        let value = serde_json::to_vec(&version).expect("encode version");
        table
            .insert("schema_version", value.as_slice())
            .expect("insert version");
    }
    write.commit().expect("commit");
}

/// Shared buffer that a `tracing_subscriber::fmt` writer drains into so the
/// test can assert which events the reset path emitted.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogs;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn schema_mismatch_reset_warns_with_backup_path() {
    let root = temp_root("schema-mismatch-warns");
    let state = root.join(".squeezy").join("cache").join("state.redb");
    std::fs::create_dir_all(state.parent().unwrap()).expect("create cache dir");
    write_schema_version(&state, 3);

    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_target(true)
        .with_max_level(tracing::Level::WARN)
        .finish();

    let store = tracing::subscriber::with_default(subscriber, || {
        SqueezyStore::open(&root, None).expect("open store")
    });

    let backup_name = std::fs::read_dir(state.parent().unwrap())
        .expect("read cache")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .find(|name| name.contains("schema-3"))
        .expect("old schema database should be backed up");

    let captured = logs.contents();
    assert!(
        captured.contains("squeezy::store"),
        "warn must target squeezy::store, got: {captured}"
    );
    assert!(
        captured.contains("schema mismatch"),
        "warn must describe the schema mismatch, got: {captured}"
    );
    assert!(
        captured.contains(&backup_name),
        "warn must reference the backup path {backup_name}, got: {captured}"
    );

    drop(store);
}

#[test]
fn graph_write_batch_applies_resolver_cache_changes() {
    let (_root, store) = open_store("resolver-batch");
    let first = FileId::new("src/first.rs");
    let second = FileId::new("src/second.rs");

    let mut batch = GraphWriteBatch::new();
    batch
        .upsert_resolver_entry(&first, &json!({"exports": ["First"]}))
        .expect("encode first resolver entry");
    batch
        .upsert_resolver_entry(&second, &json!({"exports": ["Second"]}))
        .expect("encode second resolver entry");
    assert_eq!(batch.len(), 2);
    store
        .apply_graph_batch(&batch)
        .expect("apply resolver batch");

    let first_entry: serde_json::Value = store
        .resolver_entry(&first)
        .expect("load first")
        .expect("first present");
    assert_eq!(first_entry["exports"][0], "First");
    let second_entry: serde_json::Value = store
        .resolver_entry(&second)
        .expect("load second")
        .expect("second present");
    assert_eq!(second_entry["exports"][0], "Second");

    let mut update = GraphWriteBatch::new();
    update.remove_resolver_entry(&first);
    update
        .upsert_resolver_entry(&second, &json!({"exports": ["SecondV2"]}))
        .expect("encode updated second resolver entry");
    store
        .apply_graph_batch(&update)
        .expect("apply resolver update");

    assert!(
        store
            .resolver_entry::<serde_json::Value>(&first)
            .expect("load removed first")
            .is_none(),
        "resolver removal should be applied in the batch"
    );
    let second_entry: serde_json::Value = store
        .resolver_entry(&second)
        .expect("load updated second")
        .expect("second remains");
    assert_eq!(second_entry["exports"][0], "SecondV2");
}
