# Configuration

Squeezy uses one typed TOML configuration model for provider selection,
budgets, permissions, telemetry, graph behavior, cache paths, MCP server
definitions, and TUI preferences.

Configuration is resolved in this order, from lowest to highest precedence:

1. Built-in defaults
2. User settings at `~/.squeezy/settings.toml`
3. Project settings in the nearest ancestor `squeezy.toml`
4. Environment variables
5. CLI flags

Set `SQUEEZY_SETTINGS_PATH` to use a different user settings file. Shared
project configuration should live in `squeezy.toml`; `.squeezy/` remains local
runtime state and is ignored by git in this repository.

## Commands

```sh
squeezy config inspect
squeezy config init --user
squeezy config init --project
squeezy --health
```

`config inspect` prints the effective merged configuration and redacts
sensitive-looking values. `config init` refuses to overwrite an existing file
unless `--force` is passed. `--health` validates configuration and prints the
source chain used for resolution.

## Example User Settings

```toml
[model]
provider = "openai"
profile = "balanced"
model = ""
max_output_tokens = 128
store_responses = false

[providers.openai]
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.openai.com/v1"
default_model = "gpt-5-nano"

[permissions]
read = "allow"
edit = "ask"
shell = "ask"
ignored_search = "allow"
web = "ask"

[telemetry]
enabled = true
```

## Example Project Settings

```toml
[budgets]
max_parallel_tools = 8
max_tool_calls_per_turn = 64
max_tool_bytes_read_per_turn = 20000000
max_search_files_per_turn = 50000
max_tool_result_bytes_per_round = 50000

[graph]
languages = ["rust", "python"]
max_file_bytes = 1000000
include_hidden = false
require_indexing_signal = true

[cache]
tool_outputs = ".squeezy/tool_outputs"

[tui]
tick_rate_ms = 50
status_verbosity = "compact"
```

## Sections

- `[model]`: `provider`, `model`, `profile`, `max_output_tokens`, and
  `store_responses`.
- `[providers.<id>]`: provider defaults such as `api_key_env`, `base_url`,
  `default_model`, `api_version`, and `region`.
- `[budgets]`: per-turn and per-tool output limits.
- `[permissions]`: `read`, `edit`, `shell`, `ignored_search`, and `web`, each
  set to `allow`, `ask`, or `deny`.
- `[telemetry]`: `enabled` and `endpoint`.
- `[web]`: `exa_mcp_url` and `exa_api_key_env`.
- `[graph]`: supported `languages`, file size limits, hidden-file behavior, and
  indexing-signal requirements.
- `[cache]`: `root` and `tool_outputs`.
- `[tui]`: `tick_rate_ms` and `status_verbosity`.
- `[mcp.servers.<name>]`: parse-only v0 MCP definitions with `enabled`,
  `transport`, `command`, `args`, `url`, `timeout_ms`, and `env`.

Legacy top-level `provider`, `model`, and `profile` keys remain accepted, but
new configuration should use `[model]`.

## Environment Overrides

Existing environment overrides remain supported, including:

- `SQUEEZY_PROVIDER`, `SQUEEZY_MODEL`, `SQUEEZY_PROFILE`
- `SQUEEZY_MAX_OUTPUT_TOKENS`
- `SQUEEZY_MAX_PARALLEL_TOOLS`
- `SQUEEZY_TOOL_SPILL_THRESHOLD_BYTES`
- `SQUEEZY_TOOL_PREVIEW_BYTES`
- `SQUEEZY_MAX_TOOL_RESULT_BYTES_PER_ROUND`
- `SQUEEZY_TOOL_OUTPUT_RETENTION_DAYS`
- `SQUEEZY_MAX_TOOL_CALLS_PER_TURN`
- `SQUEEZY_MAX_TOOL_BYTES_READ_PER_TURN`
- `SQUEEZY_MAX_SEARCH_FILES_PER_TURN`
- `SQUEEZY_READ_PERMISSION`, `SQUEEZY_EDIT_PERMISSION`,
  `SQUEEZY_SHELL_PERMISSION`, `SQUEEZY_IGNORED_SEARCH_PERMISSION`,
  `SQUEEZY_WEB_PERMISSION`
- `SQUEEZY_TELEMETRY`, `SQUEEZY_TELEMETRY_ENDPOINT`
- Provider-specific API key environment variable names and base URLs

Unknown fields, invalid enum values, and invalid numeric limits are reported as
configuration errors with a source and dotted path.
