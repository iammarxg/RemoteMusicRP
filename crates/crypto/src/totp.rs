// SPDX-License-Identifier: AGPL-3.0-only
//! RFC 6238 TOTP (HMAC-SHA1, 6 digits, 30 s): the parameters every
//! authenticator app supports.

use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, KeyInit, Mac};
use secrecy::{ExposeSecret, SecretBox};
use sha1::Sha1;

use crate::util::{ct_eq, random_array};
use crate::{Error, Result};

const STEP_SECS: u64 = 30;
const DIGITS: u32 = 6;
/// Accept codes from one step before/after to tolerate clock drift.
const WINDOW: i64 = 1;

/// A TOTP shared secret (20 random bytes). Zeroized on drop.
#[derive(Debug)]
pub struct TotpSecret(SecretBox<[u8; 20]>);

impl TotpSecret {
    pub fn generate() -> Result<Self> {
        Ok(Self(SecretBox::new(Box::new(random_array()?))))
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let arr: [u8; 20] = bytes.try_into().map_err(|_| Error::Malformed)?;
        Ok(Self(SecretBox::new(Box::new(arr))))
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.expose_secret()
    }

    /// Base32 form shown to the user for manual entry.
    pub fn to_base32(&self) -> String {
        BASE32_NOPAD.encode(self.0.expose_secret())
    }

    /// `otpauth://` URI for QR codes. `account` is a non-secret label.
    pub fn otpauth_uri(&self, account: &str) -> String {
        let label = percent_encode(&format!("RemoteMusicRP:{account}"));
        format!(
            "otpauth://totp/{label}?secret={}&issuer=RemoteMusicRP&algorithm=SHA1&digits={DIGITS}&period={STEP_SECS}",
            self.to_base32()
        )
    }
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn code_at(secret: &[u8], step: u64) -> Result<u32> {
    let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(secret).map_err(|_| Error::Malformed)?;
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let bin = (u32::from(digest[offset]) & 0x7f) << 24
        | u32::from(digest[offset + 1]) << 16
        | u32::from(digest[offset + 2]) << 8
        | u32::from(digest[offset + 3]);
    Ok(bin % 10u32.pow(DIGITS))
}

/// Verify `code` at time `now_unix`. `last_used_step` is the step of the last
/// accepted code (to block replay); on success returns the matched step,
/// which the caller must persist as the new `last_used_step`.
pub fn verify(
    secret: &TotpSecret,
    code: &str,
    now_unix: u64,
    last_used_step: Option<u64>,
) -> Option<u64> {
    let code = code.trim();
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let current = now_unix / STEP_SECS;
    let mut matched = None;
    // Check every candidate step (no early exit) to keep timing uniform.
    for delta in -WINDOW..=WINDOW {
        let Some(step) = current.checked_add_signed(delta) else {
            continue;
        };
        let Ok(expected) = code_at(secret.as_bytes(), step) else {
            continue;
        };
        let expected = format!("{expected:0width$}", width = DIGITS as usize);
        if ct_eq(expected.as_bytes(), code.as_bytes()) && matched.is_none() {
            matched = Some(step);
        }
    }
    match (matched, last_used_step) {
        (Some(step), Some(last)) if step <= last => None,
        (m, _) => m,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // RFC 6238 Appendix B test secret (SHA1): ASCII "12345678901234567890".
    fn rfc_secret() -> TotpSecret {
        TotpSecret::from_bytes(b"12345678901234567890").unwrap()
    }

    #[test]
    fn rfc6238_vectors() {
        // RFC lists 8-digit codes; the 6-digit code is the last 6 digits.
        for (t, code8) in [
            (59u64, "94287082"),
            (1_111_111_109, "07081804"),
            (1_111_111_111, "14050471"),
            (1_234_567_890, "89005924"),
            (2_000_000_000, "69279037"),
            (20_000_000_000, "65353130"),
        ] {
            let got = code_at(rfc_secret().as_bytes(), t / 30).unwrap();
            assert_eq!(format!("{got:06}"), &code8[2..], "t={t}");
        }
    }

    #[test]
    fn verify_window_and_replay() {
        let s = rfc_secret();
        let t = 1_234_567_890;
        let code = format!("{:06}", code_at(s.as_bytes(), t / 30).unwrap());
        let step = verify(&s, &code, t, None).unwrap();
        // Same code again is a replay.
        assert!(verify(&s, &code, t, Some(step)).is_none());
        // Accepted 30 s later (window), rejected 90 s later.
        assert!(verify(&s, &code, t + 30, None).is_some());
        assert!(verify(&s, &code, t + 90, None).is_none());
    }

    #[test]
    fn rejects_malformed_codes() {
        let s = rfc_secret();
        for bad in ["", "12345", "1234567", "12a456", "１２３４５６"] {
            assert!(verify(&s, bad, 59, None).is_none(), "{bad}");
        }
    }

    #[test]
    fn uri_format() {
        let s = rfc_secret();
        let uri = s.otpauth_uri("owner@home");
        assert!(uri.starts_with(
            "otpauth://totp/RemoteMusicRP%3Aowner%40home?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"
        ));
    }
}
