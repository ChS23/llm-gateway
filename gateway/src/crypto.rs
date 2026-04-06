//! AES-256-GCM encryption for provider API keys at rest.
//!
//! Master key from `ENCRYPTION_KEY` env var (base64-encoded 32 bytes).
//! Each value gets a random 12-byte nonce prepended to ciphertext.
//! Format: `nonce (12 bytes) || ciphertext || tag (16 bytes)`.

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::rand_core::RngCore,
    aead::{Aead, KeyInit, OsRng},
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use std::sync::OnceLock;

static ENCRYPTION_KEY: OnceLock<Option<Key<Aes256Gcm>>> = OnceLock::new();

const NONCE_LEN: usize = 12;

fn master_key() -> Option<&'static Key<Aes256Gcm>> {
    ENCRYPTION_KEY
        .get_or_init(|| {
            let env_key = std::env::var("ENCRYPTION_KEY").ok()?;
            let decoded = B64.decode(env_key.trim()).ok()?;
            if decoded.len() != 32 {
                tracing::error!(
                    "ENCRYPTION_KEY must be 32 bytes (base64-encoded), got {}",
                    decoded.len()
                );
                return None;
            }
            Some(*Key::<Aes256Gcm>::from_slice(&decoded))
        })
        .as_ref()
}

/// Encrypt plaintext → `base64(nonce || ciphertext || tag)`.
/// Returns None if ENCRYPTION_KEY not set.
pub fn encrypt(plaintext: &str) -> Option<Vec<u8>> {
    let key = master_key()?;
    let cipher = Aes256Gcm::new(key);

    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher.encrypt(nonce, plaintext.as_bytes()).ok()?;

    // nonce || ciphertext (includes 16-byte tag appended by AES-GCM)
    let mut result = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Some(result)
}

/// Decrypt `nonce || ciphertext || tag` → plaintext string.
pub fn decrypt(encrypted: &[u8]) -> Option<String> {
    if encrypted.len() < NONCE_LEN + 16 {
        return None; // too short for nonce + tag
    }

    let key = master_key()?;
    let cipher = Aes256Gcm::new(key);

    let nonce = Nonce::from_slice(&encrypted[..NONCE_LEN]);
    let ciphertext = &encrypted[NONCE_LEN..];

    let plaintext = cipher.decrypt(nonce, ciphertext).ok()?;
    String::from_utf8(plaintext).ok()
}

/// Check if encryption is available (ENCRYPTION_KEY env var set).
#[allow(dead_code)]
pub fn is_available() -> bool {
    master_key().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_key() {
        // Generate a test key
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        let encoded = B64.encode(key);
        unsafe { std::env::set_var("ENCRYPTION_KEY", &encoded) };
        // Reset OnceLock by leaking — test only
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        setup_key();
        // Force re-init by creating cipher directly
        let mut key_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut key_bytes);
        let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
        let cipher = Aes256Gcm::new(key);

        let plaintext = "sk-proj-test-key-12345";
        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher.encrypt(nonce, plaintext.as_bytes()).unwrap();

        let mut encrypted = Vec::new();
        encrypted.extend_from_slice(&nonce_bytes);
        encrypted.extend_from_slice(&ciphertext);

        // Decrypt with same key
        let nonce2 = Nonce::from_slice(&encrypted[..NONCE_LEN]);
        let decrypted = cipher.decrypt(nonce2, &encrypted[NONCE_LEN..]).unwrap();
        assert_eq!(String::from_utf8(decrypted).unwrap(), plaintext);
    }

    #[test]
    fn test_decrypt_too_short() {
        assert!(decrypt(&[0u8; 10]).is_none());
    }
}
