use crate::error::CmdError;
use keyring::Entry;

const SERVICE: &str = "waypoint";
const USER: &str = "anthropic_api_key";

fn entry() -> Result<Entry, CmdError> {
    Entry::new(SERVICE, USER).map_err(CmdError::from)
}

/// Returns the stored API key, or None if not configured / unreadable.
pub fn get_key() -> Option<String> {
    Entry::new(SERVICE, USER).ok()?.get_password().ok()
}

pub fn require_key() -> Result<String, CmdError> {
    get_key().ok_or_else(|| {
        CmdError::new(
            "auth",
            "No API key is configured. Add your Anthropic API key in Settings.",
        )
    })
}

pub fn set_key(key: &str) -> Result<(), CmdError> {
    entry()?.set_password(key).map_err(CmdError::from)
}

pub fn clear_key() -> Result<(), CmdError> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(CmdError::from(e)),
    }
}
