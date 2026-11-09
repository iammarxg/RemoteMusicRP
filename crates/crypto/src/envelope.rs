// SPDX-License-Identifier: AGPL-3.0-only
//! Envelope encryption.
//!
//! Every record gets a fresh random 256-bit data key (DEK). The record is
//! encrypted with the DEK using XChaCha20-Poly1305; the DEK is then wrapped
//! (also XChaCha20-Poly1305) by the current master key. Both layers bind the
//! [`RecordContext`] as AAD, so a blob cannot be moved to another record,
//! table or purpose. Key rotation only re-wraps the DEK ([`Keyring::rewrap`]);
//! record ciphertext is untouched.
//!
//! Blob layout (version 1):
//! ```text
//! [0]        format version (1)
//! [1..5]     master key id (u32 BE)
//! [5..29]    wrap nonce (24)
//! [29..77]   wrapped DEK (32 + 16 tag)
//! [77..101]  data nonce (24)
//! [101..]    ciphertext + 16 tag
//! ```

use std::collections::BTreeMap;
use std::fmt;

use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, Payload};
use data_encoding::BASE64URL_NOPAD;
use zeroize::{Zeroize, Zeroizing};

use crate::util::{push_lp, random_array, xcipher, xnonce};
use crate::{Error, Result};

const FORMAT_V1: u8 = 1;
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const WRAPPED_DEK_LEN: usize = KEY_LEN + TAG_LEN;
const HEADER_LEN: usize = 1 + 4 + NONCE_LEN + WRAPPED_DEK_LEN + NONCE_LEN;

// Historical identifiers from before the rename: existing encrypted data is
// bound to these exact bytes. NEVER change them.
const DATA_DOMAIN: &[u8] = b"scrobbleaway/record/v1";
const WRAP_DOMAIN: &[u8] = b"scrobbleaway/dek-wrap/v1";

/// Identifies what an encrypted blob belongs to. Bound as AAD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordContext<'a> {
    pub table: &'a str,
    pub record_id: &'a str,
    pub purpose: &'a str,
}

impl RecordContext<'_> {
    fn aad(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        push_lp(&mut out, DATA_DOMAIN);
        push_lp(&mut out, self.table.as_bytes());
        push_lp(&mut out, self.record_id.as_bytes());
        push_lp(&mut out, self.purpose.as_bytes());
        out
    }

    fn wrap_aad(&self, key_id: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(96);
        push_lp(&mut out, WRAP_DOMAIN);
        out.extend_from_slice(&key_id.to_be_bytes());
        out.extend_from_slice(&self.aad());
        out
    }
}

/// A 256-bit master key with a version id. Zeroized on drop; never printed.
#[derive(Clone)]
pub struct MasterKey {
    id: u32,
    key: Zeroizing<[u8; KEY_LEN]>,
}

impl MasterKey {
    pub fn generate(id: u32) -> Result<Self> {
        Ok(Self {
            id,
            key: Zeroizing::new(random_array()?),
        })
    }

    pub fn from_bytes(id: u32, bytes: [u8; KEY_LEN]) -> Self {
        Self {
            id,
            key: Zeroizing::new(bytes),
        }
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    fn cipher(&self) -> Result<XChaCha20Poly1305> {
        xcipher(self.key.as_ref())
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MasterKey")
            .field("id", &self.id)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

/// All master keys still needed to open existing records, plus the current
/// key used for new records.
#[derive(Clone, Debug)]
pub struct Keyring {
    current: u32,
    keys: BTreeMap<u32, MasterKey>,
}

impl Keyring {
    /// A brand-new keyring with a single freshly generated key (id 1).
    pub fn generate() -> Result<Self> {
        let key = MasterKey::generate(1)?;
        Ok(Self::from_key(key))
    }

    pub fn from_key(key: MasterKey) -> Self {
        let current = key.id;
        let mut keys = BTreeMap::new();
        keys.insert(current, key);
        Self { current, keys }
    }

    pub fn current_id(&self) -> u32 {
        self.current
    }

    pub fn key_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.keys.keys().copied()
    }

    fn current_key(&self) -> &MasterKey {
        // Invariant: `current` is always present in `keys`.
        &self.keys[&self.current]
    }

    fn key(&self, id: u32) -> Result<&MasterKey> {
        self.keys.get(&id).ok_or(Error::UnknownKey(id))
    }

    /// Start a rotation: generate a new current key. Old keys stay until
    /// [`Keyring::retire`] is called after every record has been re-wrapped.
    pub fn rotate(&mut self) -> Result<u32> {
        let next = self
            .keys
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::KeyringFormat)?;
        self.keys.insert(next, MasterKey::generate(next)?);
        self.current = next;
        Ok(next)
    }

    /// Drop every key except the current one.
    pub fn retire_old_keys(&mut self) {
        let current = self.current;
        self.keys.retain(|id, _| *id == current);
    }

    /// Encrypt `plaintext` for the record identified by `ctx`.
    pub fn seal(&self, ctx: &RecordContext<'_>, plaintext: &[u8]) -> Result<Vec<u8>> {
        let dek = Zeroizing::new(random_array::<KEY_LEN>()?);
        let data_nonce: [u8; NONCE_LEN] = random_array()?;
        let ciphertext = xcipher(dek.as_ref())?
            .encrypt(
                &xnonce(&data_nonce)?,
                Payload {
                    msg: plaintext,
                    aad: &ctx.aad(),
                },
            )
            .map_err(|_| Error::Encrypt)?;

        let master = self.current_key();
        let (wrap_nonce, wrapped) = wrap_dek(master, ctx, &dek)?;

        let mut out = Vec::with_capacity(HEADER_LEN + ciphertext.len());
        out.push(FORMAT_V1);
        out.extend_from_slice(&master.id.to_be_bytes());
        out.extend_from_slice(&wrap_nonce);
        out.extend_from_slice(&wrapped);
        out.extend_from_slice(&data_nonce);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Decrypt a blob produced by [`Keyring::seal`] with the same context.
    pub fn open(&self, ctx: &RecordContext<'_>, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let parts = Parts::parse(blob)?;
        let dek = unwrap_dek(
            self.key(parts.key_id)?,
            ctx,
            parts.wrap_nonce,
            parts.wrapped,
        )?;
        let plaintext = xcipher(dek.as_ref())?
            .decrypt(
                &xnonce(parts.data_nonce)?,
                Payload {
                    msg: parts.ciphertext,
                    aad: &ctx.aad(),
                },
            )
            .map_err(|_| Error::Decrypt)?;
        Ok(Zeroizing::new(plaintext))
    }

    /// Re-wrap the blob's data key with the current master key. Returns
    /// `Ok(None)` when the blob already uses the current key.
    pub fn rewrap(&self, ctx: &RecordContext<'_>, blob: &[u8]) -> Result<Option<Vec<u8>>> {
        let parts = Parts::parse(blob)?;
        if parts.key_id == self.current {
            return Ok(None);
        }
        let dek = unwrap_dek(
            self.key(parts.key_id)?,
            ctx,
            parts.wrap_nonce,
            parts.wrapped,
        )?;
        let master = self.current_key();
        let (wrap_nonce, wrapped) = wrap_dek(master, ctx, &dek)?;

        let mut out = Vec::with_capacity(blob.len());
        out.push(FORMAT_V1);
        out.extend_from_slice(&master.id.to_be_bytes());
        out.extend_from_slice(&wrap_nonce);
        out.extend_from_slice(&wrapped);
        out.extend_from_slice(parts.data_nonce);
        out.extend_from_slice(parts.ciphertext);
        Ok(Some(out))
    }

    /// Serialize for storage in a keystore. The returned string is secret.
    ///
    /// Format: `sa-keyring-v1;current=<id>;<id>=<base64url>;...`
    pub fn to_secret_string(&self) -> Zeroizing<String> {
        let mut s = Zeroizing::new(format!("sa-keyring-v1;current={}", self.current));
        for (id, key) in &self.keys {
            let mut b64 = BASE64URL_NOPAD.encode(key.key.as_ref());
            s.push_str(&format!(";{id}={b64}"));
            b64.zeroize();
        }
        s
    }

    pub fn from_secret_str(s: &str) -> Result<Self> {
        let mut fields = s.split(';');
        if fields.next() != Some("sa-keyring-v1") {
            return Err(Error::KeyringFormat);
        }
        let current = fields
            .next()
            .and_then(|f| f.strip_prefix("current="))
            .and_then(|v| v.parse::<u32>().ok())
            .ok_or(Error::KeyringFormat)?;
        let mut keys = BTreeMap::new();
        for field in fields {
            let (id, b64) = field.split_once('=').ok_or(Error::KeyringFormat)?;
            let id: u32 = id.parse().map_err(|_| Error::KeyringFormat)?;
            let mut raw = Zeroizing::new(
                BASE64URL_NOPAD
                    .decode(b64.as_bytes())
                    .map_err(|_| Error::KeyringFormat)?,
            );
            let bytes: [u8; KEY_LEN] = raw
                .as_slice()
                .try_into()
                .map_err(|_| Error::KeyringFormat)?;
            raw.zeroize();
            if keys.insert(id, MasterKey::from_bytes(id, bytes)).is_some() {
                return Err(Error::KeyringFormat);
            }
        }
        if !keys.contains_key(&current) {
            return Err(Error::KeyringFormat);
        }
        Ok(Self { current, keys })
    }
}

/// Read the master key id of a blob without decrypting it.
pub fn blob_key_id(blob: &[u8]) -> Result<u32> {
    Parts::parse(blob).map(|p| p.key_id)
}

fn wrap_dek(
    master: &MasterKey,
    ctx: &RecordContext<'_>,
    dek: &[u8; KEY_LEN],
) -> Result<([u8; NONCE_LEN], Vec<u8>)> {
    let nonce: [u8; NONCE_LEN] = random_array()?;
    let wrapped = master
        .cipher()?
        .encrypt(
            &xnonce(&nonce)?,
            Payload {
                msg: dek,
                aad: &ctx.wrap_aad(master.id),
            },
        )
        .map_err(|_| Error::Encrypt)?;
    Ok((nonce, wrapped))
}

fn unwrap_dek(
    master: &MasterKey,
    ctx: &RecordContext<'_>,
    nonce: &[u8],
    wrapped: &[u8],
) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let raw = Zeroizing::new(
        master
            .cipher()?
            .decrypt(
                &xnonce(nonce)?,
                Payload {
                    msg: wrapped,
                    aad: &ctx.wrap_aad(master.id),
                },
            )
            .map_err(|_| Error::Decrypt)?,
    );
    let dek: [u8; KEY_LEN] = raw.as_slice().try_into().map_err(|_| Error::Decrypt)?;
    Ok(Zeroizing::new(dek))
}

struct Parts<'a> {
    key_id: u32,
    wrap_nonce: &'a [u8],
    wrapped: &'a [u8],
    data_nonce: &'a [u8],
    ciphertext: &'a [u8],
}

impl<'a> Parts<'a> {
    fn parse(blob: &'a [u8]) -> Result<Self> {
        if blob.len() < HEADER_LEN + TAG_LEN || blob[0] != FORMAT_V1 {
            return Err(Error::Malformed);
        }
        let key_id = u32::from_be_bytes(blob[1..5].try_into().map_err(|_| Error::Malformed)?);
        let (wrap_nonce, rest) = blob[5..].split_at(NONCE_LEN);
        let (wrapped, rest) = rest.split_at(WRAPPED_DEK_LEN);
        let (data_nonce, ciphertext) = rest.split_at(NONCE_LEN);
        Ok(Self {
            key_id,
            wrap_nonce,
            wrapped,
            data_nonce,
            ciphertext,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const CTX: RecordContext<'static> = RecordContext {
        table: "secrets",
        record_id: "lastfm",
        purpose: "api_key",
    };

    #[test]
    fn roundtrip() {
        let kr = Keyring::generate().unwrap();
        let blob = kr.seal(&CTX, b"hello").unwrap();
        assert_eq!(kr.open(&CTX, &blob).unwrap().as_slice(), b"hello");
    }

    #[test]
    fn empty_plaintext_roundtrip() {
        let kr = Keyring::generate().unwrap();
        let blob = kr.seal(&CTX, b"").unwrap();
        assert!(kr.open(&CTX, &blob).unwrap().is_empty());
    }

    #[test]
    fn nonces_are_unique() {
        let kr = Keyring::generate().unwrap();
        assert_ne!(kr.seal(&CTX, b"x").unwrap(), kr.seal(&CTX, b"x").unwrap());
    }

    #[test]
    fn context_is_bound() {
        let kr = Keyring::generate().unwrap();
        let blob = kr.seal(&CTX, b"secret").unwrap();
        for other in [
            RecordContext {
                table: "other",
                ..CTX
            },
            RecordContext {
                record_id: "discord",
                ..CTX
            },
            RecordContext {
                purpose: "token",
                ..CTX
            },
        ] {
            assert!(matches!(kr.open(&other, &blob), Err(Error::Decrypt)));
        }
    }

    #[test]
    fn length_prefix_prevents_field_shifting() {
        let kr = Keyring::generate().unwrap();
        let a = RecordContext {
            table: "ab",
            record_id: "c",
            purpose: "p",
        };
        let b = RecordContext {
            table: "a",
            record_id: "bc",
            purpose: "p",
        };
        let blob = kr.seal(&a, b"x").unwrap();
        assert!(kr.open(&b, &blob).is_err());
    }

    #[test]
    fn tampering_detected_everywhere() {
        let kr = Keyring::generate().unwrap();
        let blob = kr.seal(&CTX, b"secret data").unwrap();
        for i in 5..blob.len() {
            let mut t = blob.clone();
            t[i] ^= 0x01;
            assert!(kr.open(&CTX, &t).is_err(), "byte {i} not authenticated");
        }
    }

    #[test]
    fn wrong_key_fails() {
        let a = Keyring::generate().unwrap();
        let b = Keyring::generate().unwrap();
        let blob = a.seal(&CTX, b"x").unwrap();
        assert!(matches!(b.open(&CTX, &blob), Err(Error::Decrypt)));
    }

    #[test]
    fn malformed_rejected() {
        let kr = Keyring::generate().unwrap();
        assert!(matches!(kr.open(&CTX, b""), Err(Error::Malformed)));
        assert!(matches!(kr.open(&CTX, &[2u8; 200]), Err(Error::Malformed)));
        let mut blob = kr.seal(&CTX, b"x").unwrap();
        blob.truncate(HEADER_LEN + TAG_LEN - 1);
        assert!(matches!(kr.open(&CTX, &blob), Err(Error::Malformed)));
    }

    #[test]
    fn unknown_key_id() {
        let kr = Keyring::generate().unwrap();
        let mut blob = kr.seal(&CTX, b"x").unwrap();
        blob[1..5].copy_from_slice(&99u32.to_be_bytes());
        assert!(matches!(kr.open(&CTX, &blob), Err(Error::UnknownKey(99))));
    }

    #[test]
    fn rotation_rewrap_and_retire() {
        let mut kr = Keyring::generate().unwrap();
        let old = kr.seal(&CTX, b"keep me").unwrap();
        assert_eq!(blob_key_id(&old).unwrap(), 1);

        assert_eq!(kr.rotate().unwrap(), 2);
        // Old blobs still open during rotation.
        assert_eq!(kr.open(&CTX, &old).unwrap().as_slice(), b"keep me");

        let new = kr.rewrap(&CTX, &old).unwrap().unwrap();
        assert_eq!(blob_key_id(&new).unwrap(), 2);
        // Ciphertext section unchanged by rewrap.
        assert_eq!(&new[77..], &old[77..]);
        assert!(kr.rewrap(&CTX, &new).unwrap().is_none());

        kr.retire_old_keys();
        assert!(matches!(kr.open(&CTX, &old), Err(Error::UnknownKey(1))));
        assert_eq!(kr.open(&CTX, &new).unwrap().as_slice(), b"keep me");
    }

    #[test]
    fn rewrap_key_id_cannot_be_swapped() {
        // Changing the key id in the header must fail (key id is in wrap AAD).
        let mut kr = Keyring::generate().unwrap();
        let blob = kr.seal(&CTX, b"x").unwrap();
        kr.rotate().unwrap();
        let mut t = blob.clone();
        t[1..5].copy_from_slice(&2u32.to_be_bytes());
        assert!(kr.open(&CTX, &t).is_err());
    }

    #[test]
    fn keyring_serialization_roundtrip() {
        let mut kr = Keyring::generate().unwrap();
        let blob = kr.seal(&CTX, b"x").unwrap();
        kr.rotate().unwrap();
        let s = kr.to_secret_string();
        let back = Keyring::from_secret_str(&s).unwrap();
        assert_eq!(back.current_id(), 2);
        assert_eq!(back.key_ids().collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(back.open(&CTX, &blob).unwrap().as_slice(), b"x");
    }

    #[test]
    fn keyring_parse_rejects_bad_input() {
        for bad in [
            "",
            "sa-keyring-v2;current=1;1=AAAA",
            "sa-keyring-v1;current=2;1=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "sa-keyring-v1;current=1;1=short",
            "sa-keyring-v1;current=x",
            "sa-keyring-v1;current=1;1=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA;1=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ] {
            assert!(Keyring::from_secret_str(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn debug_never_prints_key() {
        let kr = Keyring::generate().unwrap();
        let s = kr.to_secret_string();
        let b64 = s.split('=').next_back().unwrap();
        assert!(!format!("{kr:?}").contains(b64));
    }
}
