//! OAuth-backed [`ApiKeySource`] implementations for vendor subscription
//! credentials (Anthropic Claude Pro/Max, OpenAI ChatGPT Plus/Pro via
//! Codex, GitHub Copilot, …).
//!
//! Each provider's flow lives in its own submodule so the constants
//! (client id, scopes, endpoints) stay close to the wire format they
//! describe. Shared helpers — PKCE generation, base64url encoding,
//! local HTTP callback server — sit at this module's root so a new
//! OAuth subagent can be added without copy-pasting the cryptographic
//! primitives.
//!
//! The submodules return an `Arc<dyn ApiKeySource>` so the existing
//! provider clients (which already hold their credential through that
//! trait, per `crates/squeezy-llm/src/credentials.rs`) keep working
//! unchanged: the same `bearer_auth` path stamps the rotating access
//! token on every request, and the auth-retry layer
//! ([`crate::retry::send_with_auth_retry`]) handles `401`/`403`
//! refreshes.
//!
//! [`ApiKeySource`]: crate::credentials::ApiKeySource

pub mod anthropic;
pub(crate) mod openai_codex;
mod pkce;

pub use anthropic::{
    ANTHROPIC_OAUTH_TOKEN_PREFIX, AnthropicLoginConfig, AnthropicOAuthSource, PersistedTokens,
    TokenResponse, anthropic_oauth_beta_header,
    default_storage_path as anthropic_default_storage_path, exchange_authorization_code,
    is_anthropic_oauth_token, parse_authorization_input, read_tokens as anthropic_read_tokens,
    refresh_anthropic_token, write_tokens as anthropic_write_tokens,
};
pub use openai_codex::{
    OPENAI_CODEX_AUTH_FILE_NAME, OpenAiCodexLoginOutcome, OpenAiCodexOAuthSource,
    OpenAiCodexProvider, codex_auth_file_path, default_codex_auth_path, load_codex_token,
    login_openai_codex_interactive, save_codex_token,
};
pub use pkce::{PkceCodes, generate_pkce};
