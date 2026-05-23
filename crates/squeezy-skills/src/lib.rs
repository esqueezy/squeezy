use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use squeezy_core::{Result, SkillsConfig, SqueezyError};

const SKILL_FILE: &str = "SKILL.md";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    CompatUser,
    User,
    Project,
}

impl SkillSource {
    const fn precedence(self) -> u8 {
        match self {
            Self::CompatUser => 0,
            Self::User => 1,
            Self::Project => 2,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::CompatUser => "compat_user",
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub source: SkillSource,
    pub location: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSkill {
    pub summary: SkillSummary,
    pub base_dir: PathBuf,
    pub body: String,
}

impl LoadedSkill {
    pub fn prompt_block(&self) -> String {
        let SkillSummary {
            name,
            description,
            when_to_use,
            source,
            location,
        } = &self.summary;
        let when_to_use = when_to_use
            .as_ref()
            .map(|value| format!("\n<when_to_use>{value}</when_to_use>"))
            .unwrap_or_default();
        format!(
            "<skill name=\"{name}\" source=\"{}\">\n<description>{description}</description>{when_to_use}\n<location>{}</location>\n<base_directory>{}</base_directory>\n<content>\n{}\n</content>\n</skill>",
            source.as_str(),
            location.display(),
            self.base_dir.display(),
            self.body.trim()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SkillEntry {
    summary: SkillSummary,
    base_dir: PathBuf,
    triggers: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SkillCatalog {
    skills: BTreeMap<String, SkillEntry>,
}

impl SkillCatalog {
    pub fn discover(workspace_root: &Path, config: &SkillsConfig) -> Result<Self> {
        let mut catalog = Self::default();
        catalog.discover_dir(
            &config.compat_user_dir,
            SkillSource::CompatUser,
            DiscoveryMode::Optional,
        )?;
        catalog.discover_dir(&config.user_dir, SkillSource::User, DiscoveryMode::Optional)?;
        catalog.discover_dir(
            &workspace_root.join(".agents").join("skills"),
            SkillSource::Project,
            DiscoveryMode::Optional,
        )?;
        Ok(catalog)
    }

    pub fn summaries(&self) -> Vec<SkillSummary> {
        self.skills
            .values()
            .map(|entry| entry.summary.clone())
            .collect()
    }

    pub fn summaries_json(&self) -> Value {
        json!({
            "skills": self.summaries()
                .into_iter()
                .map(|summary| {
                    json!({
                        "name": summary.name,
                        "description": summary.description,
                        "when_to_use": summary.when_to_use,
                        "source": summary.source.as_str(),
                        "location": summary.location,
                    })
                })
                .collect::<Vec<_>>()
        })
    }

    pub fn load(&self, name: &str) -> Result<LoadedSkill> {
        let Some(entry) = self.skills.get(name) else {
            return Err(SqueezyError::Tool(format!("skill not found: {name}")));
        };
        let content = fs::read_to_string(&entry.summary.location)?;
        let (_metadata, body) = parse_skill_file(&content).map_err(SqueezyError::Tool)?;
        Ok(LoadedSkill {
            summary: entry.summary.clone(),
            base_dir: entry.base_dir.clone(),
            body,
        })
    }

    pub fn activate_for_input(&self, input: &str) -> Result<SkillActivation> {
        let mut task = input.to_string();
        let mut names = Vec::new();
        if let Some((name, rest)) = parse_explicit_skill_command(input) {
            names.push(name.to_string());
            task = rest.to_string();
        }

        let lowered = task.to_ascii_lowercase();
        for entry in self.skills.values() {
            if entry.triggers.iter().any(|trigger| {
                let trigger = trigger.trim();
                !trigger.is_empty() && lowered.contains(&trigger.to_ascii_lowercase())
            }) {
                names.push(entry.summary.name.clone());
            }
        }

        let mut seen = BTreeSet::new();
        let mut loaded = Vec::new();
        for name in names {
            if seen.insert(name.clone()) {
                loaded.push(self.load(&name)?);
            }
        }
        Ok(SkillActivation {
            task_input: task,
            skills: loaded,
        })
    }

    fn discover_dir(&mut self, dir: &Path, source: SkillSource, mode: DiscoveryMode) -> Result<()> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if mode == DiscoveryMode::Optional => return Err(error.into()),
            Err(error) => return Err(error.into()),
        };

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let metadata = entry.metadata()?;
            if !metadata.is_dir() {
                continue;
            }
            let skill_path = path.join(SKILL_FILE);
            let Ok(content) = fs::read_to_string(&skill_path) else {
                continue;
            };
            let Ok((metadata, _body)) = parse_skill_file(&content) else {
                continue;
            };
            if !is_valid_skill_name(&metadata.name) {
                continue;
            }
            let summary = SkillSummary {
                name: metadata.name.clone(),
                description: metadata.description,
                when_to_use: metadata.when_to_use,
                source,
                location: skill_path,
            };
            self.insert(SkillEntry {
                summary,
                base_dir: path,
                triggers: metadata.triggers,
            });
        }
        Ok(())
    }

    fn insert(&mut self, entry: SkillEntry) {
        match self.skills.get(&entry.summary.name) {
            Some(existing)
                if existing.summary.source.precedence() > entry.summary.source.precedence() => {}
            _ => {
                self.skills.insert(entry.summary.name.clone(), entry);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillActivation {
    pub task_input: String,
    pub skills: Vec<LoadedSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SkillMetadata {
    name: String,
    description: String,
    when_to_use: Option<String>,
    triggers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoveryMode {
    Optional,
}

fn parse_explicit_skill_command(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim_start();
    let rest = trimmed.strip_prefix("/skill ")?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next()?.trim();
    if name.is_empty() {
        return None;
    }
    let task = parts.next().unwrap_or("").trim_start();
    Some((name, task))
}

fn parse_skill_file(content: &str) -> std::result::Result<(SkillMetadata, String), String> {
    let mut lines = content.lines();
    if lines.next() != Some("---") {
        return Err("missing YAML frontmatter".to_string());
    }

    let mut frontmatter = Vec::new();
    let mut body = Vec::new();
    let mut in_frontmatter = true;
    for line in lines {
        if in_frontmatter && line.trim() == "---" {
            in_frontmatter = false;
            continue;
        }
        if in_frontmatter {
            frontmatter.push(line);
        } else {
            body.push(line);
        }
    }
    if in_frontmatter {
        return Err("unterminated YAML frontmatter".to_string());
    }
    let metadata = parse_frontmatter(&frontmatter)?;
    Ok((metadata, body.join("\n")))
}

fn parse_frontmatter(lines: &[&str]) -> std::result::Result<SkillMetadata, String> {
    let mut name = None;
    let mut description = None;
    let mut when_to_use = None;
    let mut triggers = Vec::new();
    let mut list_key: Option<&str> = None;

    for raw in lines {
        let line = raw.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let trimmed = line.trim_start();
        if let Some(key) = list_key {
            if let Some(item) = trimmed.strip_prefix("- ") {
                if key == "triggers" {
                    triggers.push(unquote(item.trim()).to_string());
                }
                continue;
            }
            list_key = None;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            return Err(format!("invalid frontmatter line: {line}"));
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "name" => name = Some(unquote(value).to_string()),
            "description" => description = Some(unquote(value).to_string()),
            "when_to_use" => when_to_use = Some(unquote(value).to_string()),
            "triggers" if value.is_empty() => list_key = Some("triggers"),
            "triggers" => triggers.extend(parse_inline_list(value)),
            _ => {}
        }
    }

    let name = name
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "skill frontmatter requires name".to_string())?;
    let description = description
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "skill frontmatter requires description".to_string())?;
    Ok(SkillMetadata {
        name,
        description,
        when_to_use,
        triggers,
    })
}

fn parse_inline_list(value: &str) -> Vec<String> {
    let value = value.trim();
    let Some(inner) = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    else {
        return vec![unquote(value).to_string()];
    };
    inner
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(unquote)
        .map(str::to_string)
        .collect()
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
}

fn is_valid_skill_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'))
        && value
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase())
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
