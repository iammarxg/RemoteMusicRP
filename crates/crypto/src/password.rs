// SPDX-License-Identifier: AGPL-3.0-only
//! Admin password hashing (Argon2id, PHC string format).

use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, PasswordHash, Version};
use secrecy::{ExposeSecret, SecretString};

use crate::passphrase::KdfParams;
use crate::util::random_array;
use crate::{Error, Result};

/// Minimum accepted admin password length (characters).
pub const MIN_PASSWORD_CHARS: usize = 12;
/// Maximum accepted length, to bound hashing work on hostile input.
pub const MAX_PASSWORD_BYTES: usize = 1024;

fn hasher(p: KdfParams) -> Result<Argon2<'static>> {
    let params =
        argon2::Params::new(p.m_cost_kib, p.t_cost, p.p_cost, None).map_err(|_| Error::Hash)?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// Validate password policy. Returns a user-facing reason on failure.
pub fn check_policy(password: &SecretString) -> std::result::Result<(), &'static str> {
    let pw = password.expose_secret();
    if pw.len() > MAX_PASSWORD_BYTES {
        return Err("password is too long");
    }
    if pw.chars().count() < MIN_PASSWORD_CHARS {
        return Err("password must be at least 12 characters");
    }
    Ok(())
}

/// Hash a password into a PHC string (`$argon2id$v=19$m=...`).
pub fn hash(password: &SecretString) -> Result<String> {
    hash_with(password, KdfParams::DEFAULT)
}

pub(crate) fn hash_with(password: &SecretString, params: KdfParams) -> Result<String> {
    let pw = password.expose_secret().as_bytes();
    if pw.len() > MAX_PASSWORD_BYTES {
        return Err(Error::Hash);
    }
    let salt: [u8; 16] = random_array()?;
    hasher(params)?
        .hash_password_with_salt(pw, &salt)
        .map(|h| h.to_string())
        .map_err(|_| Error::Hash)
}

/// Verify a password against a stored PHC string. The comparison inside
/// argon2 is constant-time. Parameters are taken from the stored hash.
pub fn verify(password: &SecretString, phc: &str) -> bool {
    let pw = password.expose_secret().as_bytes();
    if pw.len() > MAX_PASSWORD_BYTES {
        return false;
    }
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    if parsed.algorithm.as_str() != "argon2id" {
        return false;
    }
    Argon2::default().verify_password(pw, &parsed).is_ok()
}

/// True if the stored hash uses weaker parameters than the current default
/// and should be re-hashed after the next successful login.
pub fn needs_rehash(phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return true;
    };
    let Ok(params) = argon2::Params::try_from(&parsed) else {
        return true;
    };
    params.m_cost() < KdfParams::DEFAULT.m_cost_kib || params.t_cost() < KdfParams::DEFAULT.t_cost
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const FAST: KdfParams = KdfParams {
        m_cost_kib: 19 * 1024,
        t_cost: 2,
        p_cost: 1,
    };

    fn s(v: &str) -> SecretString {
        SecretString::from(v.to_owned())
    }

    #[test]
    fn hash_and_verify() {
        let h = hash_with(&s("a long enough password"), FAST).unwrap();
        assert!(h.starts_with("$argon2id$v=19$"));
        assert!(verify(&s("a long enough password"), &h));
        assert!(!verify(&s("a long enough passwore"), &h));
    }

    #[test]
    fn salts_differ() {
        let a = hash_with(&s("same password here"), FAST).unwrap();
        let b = hash_with(&s("same password here"), FAST).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn rejects_garbage_and_other_algorithms() {
        assert!(!verify(&s("x"), "not a hash"));
        assert!(!verify(
            &s("x"),
            "$argon2i$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaA"
        ));
    }

    #[test]
    fn policy() {
        assert!(check_policy(&s("short")).is_err());
        assert!(check_policy(&s("twelve chars")).is_ok());
        assert!(check_policy(&s(&"x".repeat(2000))).is_err());
    }

    #[test]
    fn rehash_detection() {
        let weak = hash_with(&s("a long enough password"), FAST).unwrap();
        assert!(needs_rehash(&weak));
        assert!(needs_rehash("garbage"));
    }
}
