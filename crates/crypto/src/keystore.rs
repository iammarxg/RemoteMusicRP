// SPDX-License-Identifier: AGPL-3.0-only
//! Where the master keyring lives. The keyring is never stored in the
//! database or in the data directory (except passphrase-encrypted).
//!
//! - [`FileKeyStore`]: plaintext keyring in a `0600` file. Used for Docker
//!   secrets, a separate key volume, or systemd-creds output.
//! - [`PassphraseFileKeyStore`]: keyring encrypted under an Argon2id-derived
//!   key; the passphrase is required after every restart.
//! - `OsKeyStore` (feature `os-keystore`): Windows Credential Manager, macOS
//!   Keychain, Linux Secret Service.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroizing;

use crate::passphrase::{self, KdfParams};
use crate::{Error, Keyring, Result};

/// Maximum size we will read from a key file (defends against pointing the
/// key path at something huge).
const MAX_KEY_FILE: u64 = 64 * 1024;

pub trait KeyStore: Send + Sync {
    /// Load the keyring, or `Ok(None)` if none has been created yet.
    fn load(&self) -> Result<Option<Keyring>>;
    /// Persist the keyring (atomically where the backend allows).
    fn save(&self, keyring: &Keyring) -> Result<()>;
    /// Remove the keyring permanently ("Delete everything").
    fn delete(&self) -> Result<()>;
    /// Short human-readable backend name for the UI/logs.
    fn describe(&self) -> &'static str;
}

/// Load the keyring, creating and saving a fresh one on first run.
pub fn load_or_create(store: &dyn KeyStore) -> Result<Keyring> {
    if let Some(kr) = store.load()? {
        return Ok(kr);
    }
    let kr = Keyring::generate()?;
    store.save(&kr)?;
    Ok(kr)
}

fn io_err(e: std::io::Error) -> Error {
    // Only the error kind, never paths or contents.
    Error::Storage(format!("{:?}", e.kind()))
}

fn read_limited(path: &Path) -> Result<Option<Zeroizing<Vec<u8>>>> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err(e)),
    };
    if !meta.is_file() || meta.len() > MAX_KEY_FILE {
        return Err(Error::Storage(
            "key file is not a regular file or too large".into(),
        ));
    }
    check_permissions(&meta)?;
    fs::read(path)
        .map(|v| Some(Zeroizing::new(v)))
        .map_err(io_err)
}

#[cfg(unix)]
fn check_permissions(meta: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // Refuse group/other-accessible key files (like ssh does).
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(Error::Storage(
            "key file permissions too open (need 0600)".into(),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_permissions(_meta: &fs::Metadata) -> Result<()> {
    // On Windows the file inherits the per-user ACL of its directory
    // (%LOCALAPPDATA%); the OS keystore is the recommended default there.
    Ok(())
}

/// Write `data` to `path` atomically with owner-only permissions.
fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| Error::Storage("key path has no parent".into()))?;
    fs::create_dir_all(dir).map_err(io_err)?;
    let tmp: PathBuf = path.with_extension("tmp");
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(io_err)?;
        f.write_all(data).map_err(io_err)?;
        f.sync_all().map_err(io_err)?;
    }
    fs::rename(&tmp, path).map_err(io_err)?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(e)),
    }
}

/// Plaintext keyring in an owner-only file.
#[derive(Debug, Clone)]
pub struct FileKeyStore {
    path: PathBuf,
    read_only: bool,
}

impl FileKeyStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            read_only: false,
        }
    }

    /// For externally provisioned keys (Docker secrets, systemd-creds):
    /// never written or deleted by the app.
    pub fn read_only(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            read_only: true,
        }
    }
}

impl KeyStore for FileKeyStore {
    fn load(&self) -> Result<Option<Keyring>> {
        let Some(bytes) = read_limited(&self.path)? else {
            return Ok(None);
        };
        let text = std::str::from_utf8(&bytes).map_err(|_| Error::KeyringFormat)?;
        Keyring::from_secret_str(text.trim()).map(Some)
    }

    fn save(&self, keyring: &Keyring) -> Result<()> {
        if self.read_only {
            return Err(Error::Storage(
                "key file is externally managed (read-only)".into(),
            ));
        }
        write_atomic(&self.path, keyring.to_secret_string().as_bytes())
    }

    fn delete(&self) -> Result<()> {
        if self.read_only {
            return Err(Error::Storage(
                "key file is externally managed (read-only)".into(),
            ));
        }
        remove_if_exists(&self.path)
    }

    fn describe(&self) -> &'static str {
        if self.read_only {
            "external key file"
        } else {
            "key file"
        }
    }
}

/// Keyring encrypted with a passphrase-derived key.
pub struct PassphraseFileKeyStore {
    path: PathBuf,
    passphrase: SecretString,
    params: KdfParams,
}

impl std::fmt::Debug for PassphraseFileKeyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassphraseFileKeyStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

const KEYRING_PURPOSE: &str = "keyring";

impl PassphraseFileKeyStore {
    pub fn new(path: impl Into<PathBuf>, passphrase: SecretString) -> Self {
        Self {
            path: path.into(),
            passphrase,
            params: KdfParams::DEFAULT,
        }
    }

    #[cfg(test)]
    fn with_params(mut self, params: KdfParams) -> Self {
        self.params = params;
        self
    }
}

impl KeyStore for PassphraseFileKeyStore {
    fn load(&self) -> Result<Option<Keyring>> {
        let Some(blob) = read_limited(&self.path)? else {
            return Ok(None);
        };
        let pt = passphrase::open(
            self.passphrase.expose_secret().as_bytes(),
            KEYRING_PURPOSE,
            &blob,
        )?;
        let text = std::str::from_utf8(&pt).map_err(|_| Error::KeyringFormat)?;
        Keyring::from_secret_str(text).map(Some)
    }

    fn save(&self, keyring: &Keyring) -> Result<()> {
        let blob = passphrase::seal(
            self.passphrase.expose_secret().as_bytes(),
            KEYRING_PURPOSE,
            keyring.to_secret_string().as_bytes(),
            self.params,
        )?;
        write_atomic(&self.path, &blob)
    }

    fn delete(&self) -> Result<()> {
        remove_if_exists(&self.path)
    }

    fn describe(&self) -> &'static str {
        "passphrase-protected key file"
    }
}

#[cfg(feature = "os-keystore")]
pub use os::OsKeyStore;

#[cfg(feature = "os-keystore")]
mod os {
    use super::*;

    const SERVICE: &str = "RemoteMusicRP";
    /// Service names used under earlier project names (newest first).
    const LEGACY_SERVICES: [&str; 2] = ["RemoteScrobbleRP", "ScrobbleAway"];

    /// OS credential store. `account` lets several instances coexist.
    #[derive(Debug, Clone)]
    pub struct OsKeyStore {
        account: String,
    }

    impl OsKeyStore {
        pub fn new(account: impl Into<String>) -> Self {
            Self {
                account: account.into(),
            }
        }

        fn entry(&self) -> Result<keyring::Entry> {
            keyring::Entry::new(SERVICE, &self.account)
                .map_err(|_| Error::Storage("OS keystore unavailable".into()))
        }

        /// Move a keyring stored under an earlier project name's service.
        fn migrate_legacy(&self) -> Result<Option<Keyring>> {
            for service in LEGACY_SERVICES {
                let Ok(legacy) = keyring::Entry::new(service, &self.account) else {
                    continue;
                };
                if let Ok(s) = legacy.get_password() {
                    let s = Zeroizing::new(s);
                    let kr = Keyring::from_secret_str(&s)?;
                    self.save(&kr)?;
                    let _ = legacy.delete_credential();
                    return Ok(Some(kr));
                }
            }
            Ok(None)
        }

        /// True if an OS keystore can be used on this machine.
        pub fn available() -> bool {
            keyring::Entry::store_status().is_ok()
        }
    }

    impl KeyStore for OsKeyStore {
        fn load(&self) -> Result<Option<Keyring>> {
            match self.entry()?.get_password() {
                Ok(s) => {
                    let s = Zeroizing::new(s);
                    Keyring::from_secret_str(&s).map(Some)
                }
                Err(keyring::Error::NoEntry) => self.migrate_legacy(),
                Err(_) => Err(Error::Storage("OS keystore read failed".into())),
            }
        }

        fn save(&self, keyring: &Keyring) -> Result<()> {
            self.entry()?
                .set_password(&keyring.to_secret_string())
                .map_err(|_| Error::Storage("OS keystore write failed".into()))
        }

        fn delete(&self) -> Result<()> {
            match self.entry()?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err(Error::Storage("OS keystore delete failed".into())),
            }
        }

        fn describe(&self) -> &'static str {
            "OS keystore"
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::RecordContext;

    fn tmpdir(name: &str) -> PathBuf {
        let mut buf = [0u8; 8];
        crate::util::fill_random(&mut buf).unwrap();
        let d = std::env::temp_dir().join(format!(
            "sa-test-{name}-{}",
            data_encoding::HEXLOWER.encode(&buf)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    const CTX: RecordContext<'static> = RecordContext {
        table: "t",
        record_id: "r",
        purpose: "p",
    };

    #[test]
    fn file_store_roundtrip_and_delete() {
        let dir = tmpdir("file");
        let store = FileKeyStore::new(dir.join("keys").join("master.key"));
        assert!(store.load().unwrap().is_none());
        let kr = load_or_create(&store).unwrap();
        let blob = kr.seal(&CTX, b"x").unwrap();
        let again = load_or_create(&store).unwrap();
        assert_eq!(again.open(&CTX, &blob).unwrap().as_slice(), b"x");
        store.delete().unwrap();
        assert!(store.load().unwrap().is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn file_store_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmpdir("perm");
        let path = dir.join("master.key");
        let store = FileKeyStore::new(&path);
        load_or_create(&store).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.load().is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn read_only_store_refuses_writes() {
        let dir = tmpdir("ro");
        let store = FileKeyStore::read_only(dir.join("missing.key"));
        assert!(store.save(&Keyring::generate().unwrap()).is_err());
        assert!(store.delete().is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn passphrase_store() {
        let dir = tmpdir("pp");
        let path = dir.join("keyring.enc");
        let fast = KdfParams {
            m_cost_kib: 19 * 1024,
            t_cost: 2,
            p_cost: 1,
        };
        let store =
            PassphraseFileKeyStore::new(&path, SecretString::from("open sesame".to_owned()))
                .with_params(fast);
        let kr = load_or_create(&store).unwrap();
        let blob = kr.seal(&CTX, b"y").unwrap();
        // File content must not contain the plaintext keyring marker.
        assert!(!String::from_utf8_lossy(&fs::read(&path).unwrap()).contains("sa-keyring"));
        let back = store.load().unwrap().unwrap();
        assert_eq!(back.open(&CTX, &blob).unwrap().as_slice(), b"y");
        let wrong = PassphraseFileKeyStore::new(&path, SecretString::from("wrong".to_owned()));
        assert!(matches!(wrong.load(), Err(Error::Decrypt)));
        fs::remove_dir_all(dir).unwrap();
    }
}
