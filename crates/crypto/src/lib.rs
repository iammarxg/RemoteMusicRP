// SPDX-License-Identifier: AGPL-3.0-only
//! Cryptography for RemoteMusicRP.
//!
//! - [`envelope`]: per-record data keys (XChaCha20-Poly1305) wrapped by a
//!   versioned master keyring, with AAD bound to table/record/purpose.
//! - [`passphrase`]: Argon2id-derived encryption for passphrase mode and
//!   encrypted exports.
//! - [`password`]: Argon2id admin password hashing.
//! - [`totp`]: RFC 6238 TOTP with replay protection.
//! - [`recovery`]: high-entropy 2FA recovery codes.
//! - [`keystore`]: where the master keyring lives (file, OS keystore).
#![forbid(unsafe_code)]

pub mod envelope;
pub mod keystore;
pub mod passphrase;
pub mod password;
pub mod recovery;
pub mod tokens;
pub mod totp;

mod util;

pub use envelope::{Keyring, MasterKey, RecordContext};
pub use secrecy::{ExposeSecret, SecretString};
pub use zeroize::Zeroizing;

/// Errors from this crate. Messages never contain secret material.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("random number generator failure")]
    Rng,
    #[error("encryption failed")]
    Encrypt,
    /// Authentication failed: wrong key, wrong context, or tampered data.
    #[error("decryption failed")]
    Decrypt,
    #[error("malformed encrypted blob")]
    Malformed,
    #[error("unknown master key id {0}")]
    UnknownKey(u32),
    #[error("password hashing failed")]
    Hash,
    #[error("invalid keyring encoding")]
    KeyringFormat,
    #[error("key storage error: {0}")]
    Storage(String),
}

pub type Result<T> = std::result::Result<T, Error>;
