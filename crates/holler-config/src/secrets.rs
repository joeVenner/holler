//! API-key storage in a plain TOML file in the OS config dir
//! (`<config_dir>/Holler/secrets.toml`), kept SEPARATE from `config.toml`.
//!
//! Why a file and not the OS keychain (the previous `keyring` backend)? An
//! ad-hoc-signed macOS bundle changes identity on every rebuild, so the
//! keychain TCC grant never sticks and macOS re-prompts for the login password
//! on every run. A user-scoped file removes that prompt entirely and behaves
//! identically on both OSes (one code path). See `docs/DECISIONS.md`.
//!
//! Security posture: this is the standard BYOK-tool tradeoff (cf.
//! `~/.aws/credentials`, `gh` `hosts.yml`, `~/.netrc`). The file is created
//! `0600` (owner-only) on Unix, lives under the per-user config dir, is kept
//! out of `config.toml` (which the tray "Edit Settings" opens), and is never
//! committed (`.gitignore`).
//!
//! Resolution order for [`load_secret`]: the `HOLLER_<ACCOUNT>_KEY` env var
//! first (handy for CI/headless), then `secrets.toml`.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crate::{project_dirs, ConfigError};

/// `<config_dir>/Holler/secrets.toml` — next to `config.toml`.
pub fn secrets_path() -> Result<PathBuf, ConfigError> {
    Ok(project_dirs()?.config_dir().join("secrets.toml"))
}

/// Load the API key for `account` (e.g. "deepgram").
///
/// Checks `HOLLER_<ACCOUNT_UPPER>_KEY` (e.g. `HOLLER_DEEPGRAM_KEY`) first so the
/// key can be supplied via the environment, then falls back to `secrets.toml`.
/// Returns `None` (not an error) when no key is configured — callers treat that
/// as "provider not set up".
pub fn load_secret(account: &str) -> Option<String> {
    let env_var = format!("HOLLER_{}_KEY", account.to_ascii_uppercase());
    if let Ok(val) = std::env::var(&env_var) {
        if !val.is_empty() {
            return Some(val);
        }
    }
    read_all().ok()?.remove(account).filter(|k| !k.is_empty())
}

/// Store `secret` for `account`, creating `secrets.toml` (0600 on Unix) if
/// needed and preserving any other providers' keys already present.
pub fn store_secret(account: &str, secret: &str) -> Result<(), ConfigError> {
    let path = secrets_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| ConfigError::Io(e.to_string()))?;
    }

    let mut keys = read_all().unwrap_or_default();
    keys.insert(account.to_string(), secret.to_string());

    let text = toml::to_string_pretty(&keys).map_err(|e| ConfigError::Serialize(e.to_string()))?;
    let header = "# Holler API keys — keep this file private (it is chmod 0600 on\n\
                  # macOS/Linux) and NEVER commit it. Delete a line to remove a key.\n\n";
    fs::write(&path, format!("{header}{text}")).map_err(|e| ConfigError::Io(e.to_string()))?;
    restrict_permissions(&path)?;
    Ok(())
}

/// Remove the stored key for `account` from `secrets.toml` (other providers'
/// keys are preserved). A missing file or missing entry is not an error.
/// Note: an env-var override (`HOLLER_<ACCOUNT>_KEY`) is NOT touched — that
/// belongs to the user's shell, not to us.
pub fn remove_secret(account: &str) -> Result<(), ConfigError> {
    let mut keys = read_all()?;
    if keys.remove(account).is_none() {
        return Ok(()); // nothing stored — done.
    }
    let path = secrets_path()?;
    let text = toml::to_string_pretty(&keys).map_err(|e| ConfigError::Serialize(e.to_string()))?;
    let header = "# Holler API keys — keep this file private (it is chmod 0600 on\n\
                  # macOS/Linux) and NEVER commit it. Delete a line to remove a key.\n\n";
    fs::write(&path, format!("{header}{text}")).map_err(|e| ConfigError::Io(e.to_string()))?;
    restrict_permissions(&path)?;
    Ok(())
}

/// Where a provider's key comes from, WITHOUT exposing the key itself —
/// the settings UI shows "configured ✓/✗" and never the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretStatus {
    /// No key anywhere — the provider is not set up.
    Missing,
    /// Key present in `secrets.toml` (clearable from the UI).
    FromFile,
    /// Key supplied via `HOLLER_<ACCOUNT>_KEY` (not clearable from the UI;
    /// it shadows any file entry, matching [`load_secret`]'s order).
    FromEnv,
}

/// Probe the key source for `account` (same resolution order as
/// [`load_secret`]: env var first, then the file).
pub fn secret_status(account: &str) -> SecretStatus {
    let env_var = format!("HOLLER_{}_KEY", account.to_ascii_uppercase());
    if std::env::var(&env_var).map(|v| !v.is_empty()).unwrap_or(false) {
        return SecretStatus::FromEnv;
    }
    match read_all().ok().and_then(|mut t| t.remove(account)) {
        Some(k) if !k.is_empty() => SecretStatus::FromFile,
        _ => SecretStatus::Missing,
    }
}

/// Read the whole secrets table; an absent file is an empty table.
fn read_all() -> Result<BTreeMap<String, String>, ConfigError> {
    let path = secrets_path()?;
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let text = fs::read_to_string(&path).map_err(|e| ConfigError::Io(e.to_string()))?;
    toml::from_str(&text).map_err(|e| ConfigError::Parse(e.to_string()))
}

/// Tighten the secrets file to owner read/write only. No-op on non-Unix
/// (Windows relies on the per-user `%APPDATA%` ACL).
#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) -> Result<(), ConfigError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .map_err(|e| ConfigError::Io(e.to_string()))?
        .permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms).map_err(|e| ConfigError::Io(e.to_string()))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) -> Result<(), ConfigError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // File-backed paths resolve to the REAL user config dir, so tests stick
    // to accounts that cannot exist there and to the env-var path.

    #[test]
    fn missing_account_reports_missing() {
        assert_eq!(
            secret_status("holler-test-nonexistent-provider"),
            SecretStatus::Missing
        );
    }

    #[test]
    fn env_var_reports_from_env() {
        // Serialised by account name uniqueness; no file I/O involved.
        std::env::set_var("HOLLER_HOLLER_TEST_ENV_PROV_KEY", "k");
        assert_eq!(
            secret_status("holler_test_env_prov"),
            SecretStatus::FromEnv
        );
        std::env::remove_var("HOLLER_HOLLER_TEST_ENV_PROV_KEY");
    }

    #[test]
    fn remove_of_absent_account_is_ok() {
        assert!(remove_secret("holler-test-nonexistent-provider").is_ok());
    }
}
