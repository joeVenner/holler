//! API-key storage in the OS-native keychain (macOS login keychain / Windows
//! Credential Manager) via `keyring` 3 with the platform-native backends. Keys
//! are NEVER written to config files (locked decision in `docs/DECISIONS.md`).
//!
//! keyring 3 selects the platform store at compile time from the enabled
//! `*-native` features, so no runtime store registration is needed.

use keyring::{Entry, Error};

/// Keychain service name; all Holler secrets live under this, keyed by account.
const SERVICE: &str = "holler";

/// Store `secret` for `account` (e.g. "openai") in the OS keychain.
pub fn store_key(account: &str, secret: &str) -> Result<(), Error> {
    Entry::new(SERVICE, account)?.set_password(secret)
}

/// Load the secret for `account`.
///
/// Checks `HOLLER_<ACCOUNT_UPPER>_KEY` first (e.g. `HOLLER_DEEPGRAM_KEY`),
/// so the user can export the key in their shell profile and skip keychain
/// prompts entirely. Falls back to the OS keychain when the env var is absent.
pub fn load_key(account: &str) -> Result<String, Error> {
    let env_var = format!("HOLLER_{}_KEY", account.to_ascii_uppercase());
    if let Ok(val) = std::env::var(&env_var) {
        if !val.is_empty() {
            return Ok(val);
        }
    }
    Entry::new(SERVICE, account)?.get_password()
}

/// Remove the secret for `account` from the OS keychain.
pub fn delete_key(account: &str) -> Result<(), Error> {
    Entry::new(SERVICE, account)?.delete_credential()
}
