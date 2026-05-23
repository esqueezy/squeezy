# Skills

Squeezy skills are local `SKILL.md` directories that add specialized instructions only when activated. Inactive skills are discovered as metadata and do not add their descriptions or bodies to provider requests.

## Layout

```text
skill-name/
└── SKILL.md
```

`SKILL.md` uses YAML-style frontmatter followed by Markdown instructions:

```markdown
---
name: rust-code-navigation
description: Use for Rust declarations, references, hierarchy, and impact tasks.
when_to_use: Rust source navigation and semantic graph inspection.
triggers:
  - Rust declaration
  - dependency path
---

# Rust Code Navigation
...
```

Required fields are `name` and `description`. Optional fields are `when_to_use` and `triggers`. Skill names must start with a lowercase ASCII letter and contain only lowercase letters, digits, `-`, or `_`.

## Discovery

Squeezy discovers skills from:

1. `<workspace>/.agents/skills/`
2. `~/.squeezy/skills/`
3. `~/.agents/skills/`

Project skills override user skills with the same name. Native Squeezy user skills override compatibility user skills.

The user skill directories can be changed with `SQUEEZY_SKILLS_USER_DIR` and `SQUEEZY_SKILLS_COMPAT_USER_DIR`. The same fields are available in `~/.squeezy/settings.toml`:

```toml
[skills]
user_dir = "/path/to/squeezy-skills"
compat_user_dir = "/path/to/agent-skills"
```

An example skill ships in `tests/artifacts/skills/rust-code-navigation/SKILL.md`.

## Activation

Skills can activate in three ways:

- Explicit user command: `/skill rust-code-navigation inspect this symbol`
- Trigger match: a configured trigger appears in the user task, case-insensitively
- Model request: the model calls `list_skills`, then `load_skill`

Loading a skill only injects instructions. It does not grant tools, bypass approvals, execute shell snippets, or change the session permission policy.
