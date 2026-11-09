// SPDX-License-Identifier: AGPL-3.0-only
//! 2FA recovery codes.
//!
//! Each code has 80 bits of entropy (16 base32 characters, shown as
//! `XXXX-XXXX-XXXX-XXXX`), so a fast hash (SHA-256) is sufficient for storage:
//! brute-forcing 2^80 is infeasible even offline.

use data_encoding::{BASE32_NOPAD, HEXLOWER};
use sha2::{Digest, Sha256};

use crate::Result;
use crate::util::{ct_eq, random_array};

pub const DEFAULT_COUNT: usize = 10;

/// Generate `count` codes. Returns `(codes_to_show_once, hashes_to_store)`.
pub fn generate(count: usize) -> Result<(Vec<String>, Vec<String>)> {
    let mut codes = Vec::with_capacity(count);
    let mut hashes = Vec::with_capacity(count);
    for _ in 0..count {
        let raw: [u8; 10] = random_array()?;
        let s = BASE32_NOPAD.encode(&raw); // 16 chars
        let display = format!("{}-{}-{}-{}", &s[0..4], &s[4..8], &s[8..12], &s[12..16]);
        hashes.push(hash(&display));
        codes.push(display);
    }
    Ok((codes, hashes))
}

fn normalize(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn hash(code: &str) -> String {
    HEXLOWER.encode(&Sha256::digest(normalize(code).as_bytes()))
}

/// Find which stored hash `input` matches. Every stored hash is compared so
/// timing does not reveal the position. The caller must delete the matched
/// hash (codes are single-use).
pub fn find_match(input: &str, stored_hashes: &[String]) -> Option<usize> {
    if normalize(input).len() != 16 {
        return None;
    }
    let candidate = hash(input);
    let mut found = None;
    for (i, h) in stored_hashes.iter().enumerate() {
        if ct_eq(candidate.as_bytes(), h.as_bytes()) && found.is_none() {
            found = Some(i);
        }
    }
    found
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn generate_and_match() {
        let (codes, hashes) = generate(DEFAULT_COUNT).unwrap();
        assert_eq!(codes.len(), 10);
        assert_eq!(codes[3].len(), 19);
        assert_eq!(find_match(&codes[3], &hashes), Some(3));
        // Lowercase, no dashes, extra spaces all accepted.
        let sloppy = codes[5].replace('-', " ").to_lowercase();
        assert_eq!(find_match(&sloppy, &hashes), Some(5));
    }

    #[test]
    fn rejects_unknown() {
        let (_, hashes) = generate(3).unwrap();
        assert_eq!(find_match("AAAA-AAAA-AAAA-AAAA", &hashes), None);
        assert_eq!(find_match("short", &hashes), None);
    }

    #[test]
    fn codes_unique() {
        let (codes, _) = generate(50).unwrap();
        let mut sorted = codes.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len());
    }
}
