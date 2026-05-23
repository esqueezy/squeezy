use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use squeezy_core::SkillsConfig;

use super::*;

#[test]
fn parses_skill_frontmatter_and_body() {
    let (metadata, body) = parse_skill_file(
        r#"---
name: rust-nav
description: "Use Rust navigation"
when_to_use: "Rust symbols"
triggers:
  - "rust symbol"
  - cargo metadata
---
# Rust Nav
"#,
    )
    .expect("parse");

    assert_eq!(metadata.name, "rust-nav");
    assert_eq!(metadata.description, "Use Rust navigation");
    assert_eq!(metadata.when_to_use.as_deref(), Some("Rust symbols"));
    assert_eq!(metadata.triggers, vec!["rust symbol", "cargo metadata"]);
    assert_eq!(body.trim(), "# Rust Nav");
}

#[test]
fn project_overrides_user_and_compat_user() {
    let root = temp_workspace("skills_precedence");
    let user = root.join("user");
    let compat = root.join("compat");
    write_skill(
        &compat.join("same"),
        "same",
        "compat description",
        &["compat trigger"],
    );
    write_skill(
        &user.join("same"),
        "same",
        "user description",
        &["user trigger"],
    );
    write_skill(
        &root.join(".agents/skills/same"),
        "same",
        "project description",
        &["project trigger"],
    );
    let config = SkillsConfig {
        user_dir: user,
        compat_user_dir: compat,
    };

    let catalog = SkillCatalog::discover(&root, &config).expect("discover");
    let summary = catalog.summaries().pop().expect("summary");
    assert_eq!(summary.description, "project description");
    assert_eq!(summary.source, SkillSource::Project);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn explicit_and_trigger_activation_loads_lazily() {
    let root = temp_workspace("skills_activation");
    let config = SkillsConfig {
        user_dir: root.join("user"),
        compat_user_dir: root.join("compat"),
    };
    write_skill(
        &root.join(".agents/skills/rust-nav"),
        "rust-nav",
        "Rust nav",
        &["rust symbol"],
    );
    let catalog = SkillCatalog::discover(&root, &config).expect("discover");

    let explicit = catalog
        .activate_for_input("/skill rust-nav find main")
        .expect("activate");
    assert_eq!(explicit.task_input, "find main");
    assert_eq!(explicit.skills.len(), 1);

    let trigger = catalog
        .activate_for_input("please inspect this Rust symbol")
        .expect("activate");
    assert_eq!(trigger.skills.len(), 1);

    let _ = fs::remove_dir_all(root);
}

fn write_skill(dir: &Path, name: &str, description: &str, triggers: &[&str]) {
    fs::create_dir_all(dir).expect("mkdir");
    let triggers = triggers
        .iter()
        .map(|trigger| format!("  - {trigger}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: {description}\ntriggers:\n{triggers}\n---\n# {name}\n"
        ),
    )
    .expect("write skill");
}

fn temp_workspace(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("squeezy_{name}_{nonce}"));
    fs::create_dir_all(&path).expect("create temp workspace");
    path
}
