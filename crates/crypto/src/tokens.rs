// SPDX-License-Identifier: AGPL-3.0-only
//! Random tokens for sessions and CSRF, and their storage hashes.

use data_encoding::{BASE64URL_NOPAD, HEXLOWER};
use sha2::{Digest, Sha256};

use crate::Result;
use crate::util::{ct_eq, random_array};

/// 256-bit random token, base64url (43 chars). Session tokens have full
/// entropy, so a fast hash (SHA-256) is sufficient for storage.
pub fn random_token() -> Result<String> {
    Ok(BASE64URL_NOPAD.encode(&random_array::<32>()?))
}

/// Storage form of a token: lowercase hex SHA-256.
pub fn token_hash(token: &str) -> String {
    HEXLOWER.encode(&Sha256::digest(token.as_bytes()))
}

/// Constant-time string comparison (for CSRF tokens etc.).
pub fn ct_eq_str(a: &str, b: &str) -> bool {
    ct_eq(a.as_bytes(), b.as_bytes())
}

/// OAuth2 PKCE (RFC 7636, S256) code challenge for `verifier`.
pub fn pkce_challenge(verifier: &str) -> String {
    BASE64URL_NOPAD.encode(&Sha256::digest(verifier.as_bytes()))
}

/// A fresh PKCE pair `(verifier, challenge)`. The verifier is 256 random
/// bits in base64url (43 characters, the RFC 7636 minimum length).
pub fn pkce_pair() -> Result<(String, String)> {
    let v = random_token()?;
    let c = pkce_challenge(&v);
    Ok((v, c))
}

/// Shape check before hashing/looking up untrusted cookie values.
pub fn looks_like_token(s: &str) -> bool {
    s.len() == 43
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        let a = random_token().unwrap();
        let b = random_token().unwrap();
        assert_ne!(a, b);
        assert!(looks_like_token(&a));
        assert!(!looks_like_token("short"));
        assert!(!looks_like_token(&format!("{}!", &a[..42])));
        assert_eq!(token_hash(&a).len(), 64);
        assert_eq!(token_hash(&a), token_hash(&a));
        assert!(ct_eq_str(&a, &a));
        assert!(!ct_eq_str(&a, &b));
        assert!(!ct_eq_str("a", "ab"));
    }

    #[test]
    fn pkce_rfc7636_vector() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
