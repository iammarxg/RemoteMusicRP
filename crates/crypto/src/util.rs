// SPDX-License-Identifier: AGPL-3.0-only
use crate::{Error, Result};

/// Fill `buf` from the OS CSPRNG.
pub(crate) fn fill_random(buf: &mut [u8]) -> Result<()> {
    getrandom::fill(buf).map_err(|_| Error::Rng)
}

pub(crate) fn random_array<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0u8; N];
    fill_random(&mut out)?;
    Ok(out)
}

/// Append a length-prefixed field (u32 big-endian length + bytes).
pub(crate) fn push_lp(out: &mut Vec<u8>, field: &[u8]) {
    // Fields are short identifiers; saturate rather than panic on absurd input.
    let len = u32::try_from(field.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(field);
}

/// Constant-time byte comparison.
pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

pub(crate) fn xcipher(key: &[u8]) -> Result<chacha20poly1305::XChaCha20Poly1305> {
    use chacha20poly1305::aead::KeyInit;
    chacha20poly1305::XChaCha20Poly1305::new_from_slice(key).map_err(|_| Error::Encrypt)
}

pub(crate) fn xnonce(n: &[u8]) -> Result<chacha20poly1305::XNonce> {
    chacha20poly1305::XNonce::try_from(n).map_err(|_| Error::Malformed)
}
