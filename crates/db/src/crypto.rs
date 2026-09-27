//! Encryption of guest ID document numbers: AES-256-GCM under a named key, so keys can be rotated.
//!
//! A sealed value is the random 12-byte nonce followed by the ciphertext and tag. It is stored with the id of
//! the key that sealed it, and bound to its row by the additional data (see [`guest_aad`]), so a ciphertext
//! copied onto another guest fails to open.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use std::fmt;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    #[error("the key id must be 1 to 16 letters, digits, `_` or `-`")]
    InvalidKeyId,
    #[error("the key must be base64 of exactly 32 bytes")]
    InvalidKey,
    #[error("the value was sealed with a different key")]
    WrongKey,
    #[error("the value could not be decrypted")]
    Decrypt,
}

/// An AES-256-GCM key and its id. `Debug` shows only the id.
#[derive(Clone)]
pub struct GuestIdKey {
    id: String,
    key: LessSafeKey,
    rng: SystemRandom,
}

/// A sealed value and the id of the key that sealed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub key_id: String,
    /// The 12-byte nonce followed by the ciphertext and tag.
    pub bytes: Vec<u8>,
}

impl GuestIdKey {
    pub fn from_base64(key_id: &str, b64: &str) -> Result<Self, CryptoError> {
        let plain_id = (1..=16).contains(&key_id.len())
            && key_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !plain_id {
            return Err(CryptoError::InvalidKeyId);
        }
        let bytes = STANDARD.decode(b64.trim()).map_err(|_| CryptoError::InvalidKey)?;
        // `UnboundKey::new` refuses any length other than the algorithm's 32 bytes.
        let key = UnboundKey::new(&AES_256_GCM, &bytes).map_err(|_| CryptoError::InvalidKey)?;
        Ok(Self { id: key_id.to_owned(), key: LessSafeKey::new(key), rng: SystemRandom::new() })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Seals `plaintext` under a fresh random nonce.
    pub fn seal(&self, plaintext: &str, aad: &[u8]) -> Sealed {
        let mut nonce = [0u8; NONCE_LEN];
        self.rng.fill(&mut nonce).expect("OS random number generator available");
        let mut bytes = Vec::with_capacity(NONCE_LEN + plaintext.len() + AES_256_GCM.tag_len());
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(plaintext.as_bytes());
        let tag = self
            .key
            .seal_in_place_separate_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut bytes[NONCE_LEN..])
            .expect("an ID number is far below the AES-GCM length limit");
        bytes.extend_from_slice(tag.as_ref());
        Sealed { key_id: self.id.clone(), bytes }
    }

    /// Opens a value sealed by this key with the same additional data.
    pub fn open(&self, key_id: &str, bytes: &[u8], aad: &[u8]) -> Result<String, CryptoError> {
        if key_id != self.id {
            return Err(CryptoError::WrongKey);
        }
        let (nonce, ciphertext) = bytes.split_first_chunk::<NONCE_LEN>().ok_or(CryptoError::Decrypt)?;
        let mut in_out = ciphertext.to_vec();
        let plaintext = self
            .key
            .open_in_place(Nonce::assume_unique_for_key(*nonce), Aad::from(aad), &mut in_out)
            .map_err(|_| CryptoError::Decrypt)?;
        String::from_utf8(plaintext.to_vec()).map_err(|_| CryptoError::Decrypt)
    }
}

impl fmt::Debug for GuestIdKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuestIdKey").field("id", &self.id).finish_non_exhaustive()
    }
}

/// The additional data that binds a guest's sealed ID number to that guest: tenant id then guest id.
pub fn guest_aad(tenant: Uuid, guest: Uuid) -> [u8; 32] {
    let mut aad = [0u8; 32];
    aad[..16].copy_from_slice(tenant.as_bytes());
    aad[16..].copy_from_slice(guest.as_bytes());
    aad
}

/// The last 4 characters of an ID number, after trimming; all of it when shorter.
pub fn last4(id_number: &str) -> String {
    let trimmed = id_number.trim();
    let start = trimmed.char_indices().rev().nth(3).map_or(0, |(i, _)| i);
    trimmed[start..].to_owned()
}

/// How an ID number is shown: only its last 4 characters.
pub fn mask(last4: &str) -> String {
    format!("•••• {last4}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{GUEST_ID_KEY_B64 as B64, guest_id_key as key};

    fn aad() -> [u8; 32] {
        guest_aad(Uuid::from_u128(1), Uuid::from_u128(2))
    }

    #[test]
    fn a_sealed_value_opens_to_its_plaintext() {
        let key = key();
        let sealed = key.seal("N1234567", &aad());

        assert_eq!(sealed.key_id, "k1");
        assert_eq!(sealed.bytes.len(), NONCE_LEN + "N1234567".len() + AES_256_GCM.tag_len());
        assert_eq!(key.open(&sealed.key_id, &sealed.bytes, &aad()).unwrap(), "N1234567");
    }

    #[test]
    fn a_value_moved_to_another_guest_does_not_open() {
        let key = key();
        let sealed = key.seal("N1234567", &aad());
        let other = guest_aad(Uuid::from_u128(1), Uuid::from_u128(3));

        assert_eq!(key.open("k1", &sealed.bytes, &other), Err(CryptoError::Decrypt));
    }

    #[test]
    fn a_tampered_value_does_not_open() {
        let key = key();
        let sealed = key.seal("N1234567", &aad());
        for i in 0..sealed.bytes.len() {
            let mut bytes = sealed.bytes.clone();
            bytes[i] ^= 1;
            assert_eq!(key.open("k1", &bytes, &aad()), Err(CryptoError::Decrypt), "byte {i}");
        }
    }

    #[test]
    fn a_short_value_does_not_open() {
        let key = key();

        assert_eq!(key.open("k1", &[], &aad()), Err(CryptoError::Decrypt));
        assert_eq!(key.open("k1", &[0; NONCE_LEN + 15], &aad()), Err(CryptoError::Decrypt));
    }

    #[test]
    fn a_value_sealed_under_another_key_id_is_refused() {
        let key = key();
        let sealed = key.seal("N1234567", &aad());

        assert_eq!(key.open("k2", &sealed.bytes, &aad()), Err(CryptoError::WrongKey));
    }

    #[test]
    fn sealing_twice_uses_a_fresh_nonce() {
        let key = key();

        assert_ne!(key.seal("N1234567", &aad()).bytes, key.seal("N1234567", &aad()).bytes);
    }

    #[test]
    fn a_key_must_be_base64_of_32_bytes() {
        let short = STANDARD.encode([7u8; 31]);
        let long = STANDARD.encode([7u8; 33]);

        assert_eq!(GuestIdKey::from_base64("k1", "not base64!").unwrap_err(), CryptoError::InvalidKey);
        assert_eq!(GuestIdKey::from_base64("k1", &short).unwrap_err(), CryptoError::InvalidKey);
        assert_eq!(GuestIdKey::from_base64("k1", &long).unwrap_err(), CryptoError::InvalidKey);
        assert_eq!(GuestIdKey::from_base64("k1", "").unwrap_err(), CryptoError::InvalidKey);
    }

    #[test]
    fn a_key_id_is_short_and_plain() {
        for bad in ["", "k 1", "k1!", "ключ", "k12345678901234567"] {
            assert_eq!(GuestIdKey::from_base64(bad, B64).unwrap_err(), CryptoError::InvalidKeyId, "{bad:?}");
        }
        for good in ["k", "k1", "2026-09_a", "k123456789012345"] {
            assert_eq!(GuestIdKey::from_base64(good, B64).unwrap().id(), good);
        }
    }

    #[test]
    fn debug_shows_only_the_key_id() {
        let shown = format!("{:?}", key());

        assert!(shown.contains("k1"), "{shown}");
        assert!(!shown.contains(B64), "{shown}");
        let bytes = STANDARD.decode(B64).unwrap();
        assert!(!shown.contains(&format!("{bytes:?}")), "{shown}");
        assert!(!shown.contains(&format!("{:?}", &bytes[..4])), "{shown}");
    }

    #[test]
    fn errors_never_carry_the_input() {
        let err = GuestIdKey::from_base64("k1", "c2VjcmV0").unwrap_err().to_string();

        assert!(!err.contains("c2VjcmV0"), "{err}");
    }

    #[test]
    fn the_aad_is_tenant_then_guest() {
        let (tenant, guest) = (Uuid::now_v7(), Uuid::now_v7());
        let aad = guest_aad(tenant, guest);

        assert_eq!(&aad[..16], tenant.as_bytes());
        assert_eq!(&aad[16..], guest.as_bytes());
    }

    #[test]
    fn last4_keeps_the_last_four_characters_after_trimming() {
        assert_eq!(last4(" N1234567 "), "4567");
        assert_eq!(last4("AB12"), "AB12");
        assert_eq!(last4(" 12 "), "12");
        assert_eq!(last4(""), "");
        assert_eq!(last4("ÄÖÜßéè"), "Üßéè");
    }

    #[test]
    fn mask_shows_only_the_last_four() {
        assert_eq!(mask("1234"), "•••• 1234");
        assert_eq!(mask(&last4("N1234567")), "•••• 4567");
    }
}
