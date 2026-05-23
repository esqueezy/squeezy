//! Local persistence layer for Squeezy.
//!
//! This crate hosts two independent on-disk stores that share little code
//! beyond a few small helpers, but live together because both are part of
//! the local-state surface (and so consumers can reach them through a
//! single `squeezy-store` dependency).
//!
//! * `repo_profile` — generated per-repo facts (`~/.squeezy/repos.toml`).
//! * `sessions` — per-session metadata and event logs.

pub mod repo_profile;
pub mod sessions;

pub use repo_profile::*;
pub use sessions::*;

pub const CRATE_NAME: &str = "squeezy-store";

pub fn crate_name() -> &'static str {
    CRATE_NAME
}
