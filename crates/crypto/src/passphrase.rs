// SPDX-License-Identifier: AGPL-3.0-only
//! Passphrase-based encryption (Argon2id + XChaCha20-Poly1305).
//!
//! Used for passphrase mode (the keyring is stored encrypted under a key
//! derived from the owner's passphrase) and for encrypted settings exports.
//! KDF parameters and salt are stored in the blob so they can be raised later.
//!
//! Blob layout:
//! ```text
//! "SAPP" magic (4) | version 1 (1) | m_cost KiB u32 BE | t_cost u32 BE |
//! p_cost u32 BE | salt (16) | nonce (24) | ciphertext + tag
//! ```

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, Payload};
use zeroize::Zeroizing;

use crate::util::{push_lp, random_array, xcipher, xnonce};
use crate::{Error, Result};

const MAGIC: &[u8; 4] = b"SAPP";
const VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = 4 + 1 + 12 + SALT_LEN + NONCE_LEN;

/// Argon2id cost parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl KdfParams {
    /// Default: 64 MiB, 3 passes, 1 lane (RFC 9106 second recommended
    /// option, single lane to bound CPU). Memory is only used transiently
    /// during unlock/export.
    pub const DEFAULT: Self = Self {
        m_cost_kib: 64 * 1024,
        t_cost: 3,
        p_cost: 1,
    };

    /// Lower bound accepted when reading a blob, to stop a crafted file from
    /// downgrading the KDF to something trivially brute-forceable.
    const MIN: Self = Self {
        m_cost_kib: 19 * 1024,
        t_cost: 2,
        p_cost: 1,
    };

    /// Upper bound accepted when reading a blob, to stop a crafted file from
    /// exhausting memory/CPU.
    const MAX: Self = Self {
        m_cost_kib: 1024 * 1024,
        t_cost: 16,
        p_cost: 8,
    };

    fn validate(&self) -> Result<()> {
        let ok = (Self::MIN.m_cost_kib..=Self::MAX.m_cost_kib).contains(&self.m_cost_kib)
            && (Self::MIN.t_cost..=Self::MAX.t_cost).contains(&self.t_cost)
            && (Self::MIN.p_cost..=Self::MAX.p_cost).contains(&self.p_cost);
        if ok { Ok(()) } else { Err(Error::Malformed) }
    }
}

fn derive_key(passphrase: &[u8], salt: &[u8], p: KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    let params =
        Params::new(p.m_cost_kib, p.t_cost, p.p_cost, Some(32)).map_err(|_| Error::Hash)?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase, salt, out.as_mut())
        .map_err(|_| Error::Hash)?;
    Ok(out)
}

fn aad(purpose: &str, header: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(header.len() + 48);
    // Historical identifier (pre-rename); NEVER change: existing blobs depend on it.
    push_lp(&mut out, b"scrobbleaway/passphrase/v1");
    push_lp(&mut out, purpose.as_bytes());
    out.extend_from_slice(header);
    out
}

/// Encrypt `plaintext` under `passphrase`. `purpose` (e.g. `"keyring"`,
/// `"export"`) is bound as AAD so blobs cannot be swapped between uses.
pub fn seal(
    passphrase: &[u8],
    purpose: &str,
    plaintext: &[u8],
    params: KdfParams,
) -> Result<Vec<u8>> {
    params.validate()?;
    let salt: [u8; SALT_LEN] = random_array()?;
    let nonce: [u8; NONCE_LEN] = random_array()?;

    let mut out = Vec::with_capacity(HEADER_LEN + plaintext.len() + 16);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&params.m_cost_kib.to_be_bytes());
    out.extend_from_slice(&params.t_cost.to_be_bytes());
    out.extend_from_slice(&params.p_cost.to_be_bytes());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);

    let key = derive_key(passphrase, &salt, params)?;
    let ct = xcipher(key.as_ref())?
        .encrypt(
            &xnonce(&nonce)?,
            Payload {
                msg: plaintext,
                aad: &aad(purpose, &out),
            },
        )
        .map_err(|_| Error::Encrypt)?;
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Decrypt a blob from [`seal`]. Wrong passphrase and tampering both yield
/// [`Error::Decrypt`].
pub fn open(passphrase: &[u8], purpose: &str, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if blob.len() < HEADER_LEN + 16 || &blob[..4] != MAGIC || blob[4] != VERSION {
        return Err(Error::Malformed);
    }
    let be = |i: usize| -> Result<u32> {
        Ok(u32::from_be_bytes(
            blob[i..i + 4].try_into().map_err(|_| Error::Malformed)?,
        ))
    };
    let params = KdfParams {
        m_cost_kib: be(5)?,
        t_cost: be(9)?,
        p_cost: be(13)?,
    };
    params.validate()?;
    let salt = &blob[17..17 + SALT_LEN];
    let nonce = &blob[17 + SALT_LEN..HEADER_LEN];
    let (header, ct) = blob.split_at(HEADER_LEN);

    let key = derive_key(passphrase, salt, params)?;
    let pt = xcipher(key.as_ref())?
        .decrypt(
            &xnonce(nonce)?,
            Payload {
                msg: ct,
                aad: &aad(purpose, header),
            },
        )
        .map_err(|_| Error::Decrypt)?;
    Ok(Zeroizing::new(pt))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // Minimum params keep tests fast; production uses DEFAULT.
    const FAST: KdfParams = KdfParams::MIN;

    #[test]
    fn roundtrip() {
        let blob = seal(b"correct horse", "export", b"data", FAST).unwrap();
        assert_eq!(
            open(b"correct horse", "export", &blob).unwrap().as_slice(),
            b"data"
        );
    }

    #[test]
    fn wrong_passphrase_or_purpose_fails() {
        let blob = seal(b"pw", "export", b"data", FAST).unwrap();
        assert!(matches!(open(b"pw2", "export", &blob), Err(Error::Decrypt)));
        assert!(matches!(open(b"pw", "keyring", &blob), Err(Error::Decrypt)));
    }

    #[test]
    fn header_is_authenticated() {
        let blob = seal(b"pw", "export", b"data", FAST).unwrap();
        // Flip a salt byte and a nonce byte: both must fail authentication.
        for i in [17, 17 + SALT_LEN] {
            let mut t = blob.clone();
            t[i] ^= 1;
            assert!(open(b"pw", "export", &t).is_err());
        }
    }

    #[test]
    fn rejects_downgraded_or_huge_params() {
        let blob = seal(b"pw", "export", b"data", FAST).unwrap();
        let mut weak = blob.clone();
        weak[5..9].copy_from_slice(&8u32.to_be_bytes()); // 8 KiB
        assert!(matches!(
            open(b"pw", "export", &weak),
            Err(Error::Malformed)
        ));
        let mut huge = blob.clone();
        huge[5..9].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(
            open(b"pw", "export", &huge),
            Err(Error::Malformed)
        ));
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            open(b"pw", "export", b"nope"),
            Err(Error::Malformed)
        ));
    }
}
