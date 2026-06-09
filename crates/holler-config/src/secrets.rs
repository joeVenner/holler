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
