//! saysdfTerm secret storage (`secret-storage` spec): secrets live only in the OS keychain
//! (Windows Credential Manager, macOS Keychain, Linux Secret Service) under the service
//! [`saysdf_identity::APP_NAME`] and the key `<domain>:<id>` (`db:<id>`, `ssh:<alias>`).
//!
//! Errors never contain the secret value: only the key and a coarse reason.

use std::fmt;

/// Key namespace shared by `database-viewer` and `ssh-connections`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    Db,
    Ssh,
}

impl Domain {
    pub fn as_str(self) -> &'static str {
        match self {
            Domain::Db => "db",
            Domain::Ssh => "ssh",
        }
    }
}

/// Keychain key `<domain>:<id>`.
pub fn key(domain: Domain, id: &str) -> Result<String, Error> {
    if id.is_empty() || id.len() > 256 || id.chars().any(|c| c.is_control()) {
        return Err(Error::InvalidId);
    }
    Ok(format!("{}:{}", domain.as_str(), id))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No secret stored under this key.
    NotFound { key: String },
    /// Keychain is missing/locked/inaccessible: the caller should prompt for the secret
    /// on every connect and never store it in plain text.
    Unavailable { key: String, reason: String },
    /// Empty, too long or control characters in the id.
    InvalidId,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound { key } => write!(f, "no secret stored for {key}"),
            Error::Unavailable { key, reason } => {
                write!(f, "system keychain unavailable for {key}: {reason}")
            }
            Error::InvalidId => write!(f, "invalid secret id"),
        }
    }
}

impl std::error::Error for Error {}

/// Storage backend; [`OsKeychain`] in production, [`MemoryKeychain`] in tests.
pub trait Keychain {
    fn get(&self, key: &str) -> Result<String, Error>;
    fn set(&self, key: &str, secret: &str) -> Result<(), Error>;
    fn delete(&self, key: &str) -> Result<(), Error>;
}

/// The OS keychain via `keyring`, service = `saysdfTerm`.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsKeychain;

fn map_keyring(key: &str, e: keyring::Error) -> Error {
    // Only a coarse classification is kept: keyring errors may wrap platform data.
    let reason = match &e {
        keyring::Error::NoEntry => {
            return Error::NotFound {
                key: key.to_string(),
            }
        }
        keyring::Error::PlatformFailure(_) => "platform failure",
        keyring::Error::NoStorageAccess(_) => "no storage access (locked or missing)",
        keyring::Error::BadEncoding(_) => "stored secret is not valid UTF-8",
        keyring::Error::TooLong(..) => "attribute too long",
        keyring::Error::Invalid(..) => "invalid attribute",
        keyring::Error::Ambiguous(_) => "ambiguous entry",
        _ => "unknown keychain error",
    };
    Error::Unavailable {
        key: key.to_string(),
        reason: reason.to_string(),
    }
}

impl OsKeychain {
    fn entry(key: &str) -> Result<keyring::Entry, Error> {
        keyring::Entry::new(saysdf_identity::APP_NAME, key).map_err(|e| map_keyring(key, e))
    }
}

impl Keychain for OsKeychain {
    fn get(&self, key: &str) -> Result<String, Error> {
        Self::entry(key)?
            .get_password()
            .map_err(|e| map_keyring(key, e))
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), Error> {
        Self::entry(key)?
            .set_password(secret)
            .map_err(|e| map_keyring(key, e))
    }
    fn delete(&self, key: &str) -> Result<(), Error> {
        Self::entry(key)?
            .delete_credential()
            .map_err(|e| map_keyring(key, e))
    }
}

/// In-process store (tests; never persisted).
#[derive(Debug, Default)]
pub struct MemoryKeychain(std::sync::Mutex<std::collections::HashMap<String, String>>);

impl Keychain for MemoryKeychain {
    fn get(&self, key: &str) -> Result<String, Error> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(key)
            .cloned()
            .ok_or_else(|| Error::NotFound { key: key.into() })
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), Error> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.into(), secret.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), Error> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(key)
            .map(|_| ())
            .ok_or_else(|| Error::NotFound { key: key.into() })
    }
}

/// Typed facade: `get/set/delete(domain, id)`.
pub struct Secrets<K: Keychain = OsKeychain> {
    store: K,
}

impl Secrets<OsKeychain> {
    pub fn os() -> Self {
        Self { store: OsKeychain }
    }
}

impl<K: Keychain> Secrets<K> {
    pub fn with(store: K) -> Self {
        Self { store }
    }
    pub fn get(&self, domain: Domain, id: &str) -> Result<String, Error> {
        self.store.get(&key(domain, id)?)
    }
    pub fn set(&self, domain: Domain, id: &str, secret: &str) -> Result<(), Error> {
        self.store.set(&key(domain, id)?, secret)
    }
    /// Deleting a missing secret is not an error (connection removal is idempotent).
    pub fn delete(&self, domain: Domain, id: &str) -> Result<(), Error> {
        match self.store.delete(&key(domain, id)?) {
            Err(Error::NotFound { .. }) => Ok(()),
            r => r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_format_and_domains_do_not_clash() {
        assert_eq!(key(Domain::Db, "prod").unwrap(), "db:prod");
        assert_eq!(key(Domain::Ssh, "prod").unwrap(), "ssh:prod");
        assert_eq!(key(Domain::Db, ""), Err(Error::InvalidId));
        assert_eq!(key(Domain::Db, "a\nb"), Err(Error::InvalidId));
    }

    #[test]
    fn set_get_delete_roundtrip() {
        let s = Secrets::with(MemoryKeychain::default());
        assert!(matches!(
            s.get(Domain::Db, "x"),
            Err(Error::NotFound { .. })
        ));
        s.set(Domain::Db, "x", "hunter2").unwrap();
        s.set(Domain::Ssh, "x", "other").unwrap();
        assert_eq!(s.get(Domain::Db, "x").unwrap(), "hunter2");
        s.delete(Domain::Db, "x").unwrap();
        s.delete(Domain::Db, "x").unwrap();
        assert!(matches!(
            s.get(Domain::Db, "x"),
            Err(Error::NotFound { .. })
        ));
        assert_eq!(s.get(Domain::Ssh, "x").unwrap(), "other");
    }

    #[test]
    fn errors_do_not_leak_secret() {
        let e = map_keyring("db:x", keyring::Error::NoStorageAccess("hunter2".into()));
        let shown = format!("{e} {e:?}");
        assert!(!shown.contains("hunter2"));
        assert!(matches!(e, Error::Unavailable { .. }));
        assert_eq!(
            map_keyring("db:x", keyring::Error::NoEntry),
            Error::NotFound { key: "db:x".into() }
        );
    }

    #[test]
    fn keyring_mock_entry_roundtrip() {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        let e = keyring::Entry::new(saysdf_identity::APP_NAME, "db:mock").unwrap();
        e.set_password("pw").unwrap();
        assert_eq!(e.get_password().unwrap(), "pw");
        e.delete_credential().unwrap();
        assert!(matches!(e.get_password(), Err(keyring::Error::NoEntry)));
    }
}
