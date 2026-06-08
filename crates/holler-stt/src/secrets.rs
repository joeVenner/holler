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

/// Load the secret for `account` from the OS keychain.
pub fn load_key(account: &str) -> Result<String, Error> {
    Entry::new(SERVICE, account)?.get_password()
}

/// Remove the secret for `account` from the OS keychain.
pub fn delete_key(account: &str) -> Result<(), Error> {
    Entry::new(SERVICE, account)?.delete_credential()
}
