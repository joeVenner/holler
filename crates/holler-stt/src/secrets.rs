//! API-key storage. Keys live in a local `secrets.toml` in the OS config dir,
//! managed by [`holler_config::secrets`] — NOT in the OS keychain (dropped to
//! avoid the recurring macOS keychain prompt; see `docs/DECISIONS.md`).
//!
//! These thin wrappers keep the STT crate's call sites (`store_key`/`load_key`)
//! stable while the storage backend lives in `holler-config`.

pub use holler_config::ConfigError;

/// Store `secret` for `account` (e.g. "deepgram") in `secrets.toml`.
pub fn store_key(account: &str, secret: &str) -> Result<(), ConfigError> {
    holler_config::store_secret(account, secret)
}

/// Load the key for `account` — env var (`HOLLER_<ACCOUNT>_KEY`) then file.
/// Returns `None` when no key is configured.
pub fn load_key(account: &str) -> Option<String> {
    holler_config::load_secret(account)
}
