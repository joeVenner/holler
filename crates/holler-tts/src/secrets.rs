//! API-key storage for cloud TTS. Keys live in the same local `secrets.toml`
//! (OS config dir) managed by [`holler_config::secrets`] — NOT the OS keychain
//! (dropped to avoid the recurring macOS keychain prompt; see `docs/DECISIONS.md`).
//!
//! These thin wrappers keep the TTS crate's call sites (`store_key`/`load_key`)
//! stable while the storage backend lives in `holler-config`, exactly as
//! `holler-stt::secrets` does. The OpenAI TTS backend shares the `"openai"`
//! account with `holler-stt`, so a single configured key serves both.

pub use holler_config::ConfigError;

/// Store `secret` for `account` (e.g. "openai") in `secrets.toml`.
pub fn store_key(account: &str, secret: &str) -> Result<(), ConfigError> {
    holler_config::store_secret(account, secret)
}

/// Load the key for `account` — env var (`HOLLER_<ACCOUNT>_KEY`) then file.
/// Returns `None` when no key is configured.
pub fn load_key(account: &str) -> Option<String> {
    holler_config::load_secret(account)
}
