//! Stores the issued API key in the OS keychain (macOS Keychain, Windows
//! Credential Manager, Linux Secret Service).

use keyring::Entry;

const SERVICE: &str = "divisas-desktop";
const ACCOUNT: &str = "api-key";

/// Read the stored API key, if any.
pub fn load_api_key() -> Option<String> {
    Entry::new(SERVICE, ACCOUNT)
        .ok()?
        .get_password()
        .ok()
        .filter(|key| !key.trim().is_empty())
}

/// Persist the API key.
pub fn save_api_key(key: &str) -> Result<(), keyring::Error> {
    Entry::new(SERVICE, ACCOUNT)?.set_password(key)
}

/// Remove the stored API key (sign out).
#[allow(dead_code)]
pub fn clear_api_key() -> Result<(), keyring::Error> {
    Entry::new(SERVICE, ACCOUNT)?.delete_credential()
}
