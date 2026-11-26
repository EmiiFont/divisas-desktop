//! Session persistence.
//!
//! The API key is stored in the OS keychain when possible, with a
//! permission-restricted file fallback in the app-data directory. The
//! fallback matters in development: every rebuild produces a new unsigned
//! binary, and macOS keychain access grants are tied to the binary, so
//! `get_password()` can silently fail after a rebuild.
//!
//! We also remember the last auth method ("email" or "anonymous") so the
//! app can restore guest sessions silently at launch.

use keyring::Entry;
use std::io::Write;
use std::path::PathBuf;

const SERVICE: &str = "divisas-desktop";
const ACCOUNT: &str = "api-key";
const FILE_KEY: &str = "api-key";
const FILE_METHOD: &str = "auth-method";

/// Where session files live (created if missing).
fn data_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    // macOS: ~/Library/Application Support, Linux: ~/.local/share
    let dir = if cfg!(target_os = "macos") {
        PathBuf::from(home).join("Library/Application Support/divisas-desktop")
    } else {
        PathBuf::from(home).join(".local/share/divisas-desktop")
    };
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn file_path(name: &str) -> Option<PathBuf> {
    data_dir().map(|dir| dir.join(name))
}

/// Read the stored API key: keychain first, then the file fallback.
pub fn load_api_key() -> Option<String> {
    if let Ok(entry) = Entry::new(SERVICE, ACCOUNT) {
        if let Ok(key) = entry.get_password() {
            if !key.trim().is_empty() {
                return Some(key);
            }
        }
    }
    read_file(FILE_KEY).filter(|key| !key.trim().is_empty())
}

/// Persist the API key. Tries the keychain, falls back to a 0600 file.
pub fn save_api_key(key: &str) -> Result<(), String> {
    if let Ok(entry) = Entry::new(SERVICE, ACCOUNT) {
        if entry.set_password(key).is_ok() {
            // Keep the file in sync so a keychain ACL hiccup can't lose it.
            let _ = write_file(FILE_KEY, key);
            return Ok(());
        }
    }
    write_file(FILE_KEY, key)
}

/// Remove the stored API key (sign out).
#[allow(dead_code)]
pub fn clear_api_key() -> Result<(), String> {
    let mut failed = String::new();
    if let Ok(entry) = Entry::new(SERVICE, ACCOUNT) {
        if let Err(err) = entry.delete_credential() {
            failed = err.to_string();
        }
    }
    if let Some(path) = file_path(FILE_KEY) {
        let _ = std::fs::remove_file(path);
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed)
    }
}

/// Remember how the current session was authenticated ("email"/"anonymous").
pub fn save_auth_method(method: &str) {
    let _ = write_file(FILE_METHOD, method);
}

/// Last auth method, if recorded.
pub fn load_auth_method() -> Option<String> {
    read_file(FILE_METHOD)
}

fn read_file(name: &str) -> Option<String> {
    std::fs::read_to_string(file_path(name)?).ok()
}

fn write_file(name: &str, content: &str) -> Result<(), String> {
    let path = file_path(name).ok_or_else(|| "no data dir".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(content.as_bytes())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, content).map_err(|e| e.to_string())
    }
}
