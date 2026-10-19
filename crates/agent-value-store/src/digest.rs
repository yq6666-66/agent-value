//! Keyed digests of user text (v22 §8: store a hash, never the text).
//!
//! HMAC-SHA256 with a per-install key that the caller supplies and keeps
//! OUTSIDE the database, so a copied database alone does not let anyone
//! confirm a guessed prompt. The key is never written to the store.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use agent_value_core::CoreError;

/// Minimum digest key length in bytes.
pub const MIN_DIGEST_KEY_BYTES: usize = 16;

/// The per-install digest key. `Debug` never prints the bytes.
#[derive(Clone)]
pub struct DigestKey(Vec<u8>);

impl DigestKey {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.len() < MIN_DIGEST_KEY_BYTES {
            return Err(CoreError::OutOfRange {
                what: "digest key length (bytes)",
                value: bytes.len() as f64,
                min: MIN_DIGEST_KEY_BYTES as f64,
                max: f64::INFINITY,
            });
        }
        Ok(Self(bytes.to_vec()))
    }

    /// HMAC-SHA256 of `text`, as 64 lower-case hex digits.
    pub fn digest_hex(&self, text: &str) -> String {
        // new_from_slice accepts keys of any length for HMAC, so this cannot fail.
        let mut mac = match <Hmac<Sha256> as Mac>::new_from_slice(&self.0) {
            Ok(m) => m,
            Err(_) => return sha256_hex(text.as_bytes()),
        };
        mac.update(text.as_bytes());
        hex(&mac.finalize().into_bytes())
    }
}

impl std::fmt::Debug for DigestKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DigestKey(<redacted>)")
    }
}

/// Plain SHA-256 as hex; used for config payload content hashes only.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_answer() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hmac_known_answer() {
        // RFC 4231 test case 2.
        let k = DigestKey(b"Jefe".to_vec());
        assert_eq!(
            k.digest_hex("what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn key_rules() {
        assert!(DigestKey::from_bytes(&[0u8; 15]).is_err());
        let k = DigestKey::from_bytes(&[7u8; 32]).unwrap();
        assert_eq!(format!("{k:?}"), "DigestKey(<redacted>)");
        let k2 = DigestKey::from_bytes(&[8u8; 32]).unwrap();
        assert_ne!(k.digest_hex("same text"), k2.digest_hex("same text"));
        assert_eq!(k.digest_hex("same text"), k.digest_hex("same text"));
    }
}
