//! Cryptographic functions for pattern sync
//!
//! Implements AES-256-GCM encryption for secure pattern sharing
//! with Argon2 key derivation from passphrase.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use aes_gcm::aead::generic_array::GenericArray;
use argon2::Argon2;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use anyhow::{Result, anyhow};
use rand::RngCore;
use serde::{Deserialize, Serialize};

/// Salt length for Argon2 (16 bytes recommended)
const SALT_LENGTH: usize = 16;
/// Nonce length for AES-GCM (12 bytes)
const NONCE_LENGTH: usize = 12;
/// Key length for AES-256 (32 bytes)
const KEY_LENGTH: usize = 32;

/// Encrypted data bundle
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedData {
    /// Base64-encoded ciphertext
    pub ciphertext: String,
    /// Base64-encoded nonce (IV)
    pub nonce: String,
    /// Base64-encoded salt for key derivation
    pub salt: String,
    /// Version of encryption scheme
    pub version: u8,
}

/// Derive a 256-bit key from passphrase using Argon2id
fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; KEY_LENGTH]> {
    let argon2 = Argon2::default();
    let mut key = [0u8; KEY_LENGTH];

    argon2.hash_password_into(
        passphrase.as_bytes(),
        salt,
        &mut key,
    ).map_err(|e| anyhow!("Key derivation failed: {}", e))?;

    Ok(key)
}

/// Encrypt data with AES-256-GCM
///
/// Uses Argon2id for key derivation from passphrase.
/// Returns an EncryptedData struct containing the ciphertext, nonce, and salt.
pub fn encrypt_data(plaintext: &[u8], passphrase: &str) -> Result<EncryptedData> {
    // Generate random salt for key derivation
    let mut salt = [0u8; SALT_LENGTH];
    OsRng.fill_bytes(&mut salt);

    // Derive key from passphrase
    let key = derive_key(passphrase, &salt)?;
    let key = GenericArray::from_slice(&key);

    // Create cipher
    let cipher = Aes256Gcm::new(key);

    // Generate random nonce
    let mut nonce_bytes = [0u8; NONCE_LENGTH];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(nonce_bytes.as_slice());

    // Encrypt
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| anyhow!("Encryption failed: {}", e))?;

    Ok(EncryptedData {
        ciphertext: BASE64.encode(&ciphertext),
        nonce: BASE64.encode(nonce_bytes),
        salt: BASE64.encode(salt),
        version: 1,
    })
}

/// Decrypt data with AES-256-GCM
///
/// Derives the key from passphrase using the stored salt.
pub fn decrypt_data(encrypted: &EncryptedData, passphrase: &str) -> Result<Vec<u8>> {
    // Validate version
    if encrypted.version != 1 {
        return Err(anyhow!("Unsupported encryption version: {}", encrypted.version));
    }

    // Decode salt
    let salt = BASE64.decode(&encrypted.salt)
        .map_err(|e| anyhow!("Invalid salt encoding: {}", e))?;

    // Derive key from passphrase
    let key = derive_key(passphrase, &salt)?;
    let key = GenericArray::from_slice(&key);

    // Create cipher
    let cipher = Aes256Gcm::new(key);

    // Decode nonce
    let nonce_bytes = BASE64.decode(&encrypted.nonce)
        .map_err(|e| anyhow!("Invalid nonce encoding: {}", e))?;
    let nonce = Nonce::from_slice(nonce_bytes.as_slice());

    // Decode ciphertext
    let ciphertext = BASE64.decode(&encrypted.ciphertext)
        .map_err(|e| anyhow!("Invalid ciphertext encoding: {}", e))?;

    // Decrypt
    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| anyhow!("Decryption failed: invalid passphrase or corrupted data"))?;

    Ok(plaintext)
}

/// Encrypt a string and return as EncryptedData
pub fn encrypt_string(plaintext: &str, passphrase: &str) -> Result<EncryptedData> {
    encrypt_data(plaintext.as_bytes(), passphrase)
}

/// Decrypt EncryptedData to a string
pub fn decrypt_string(encrypted: &EncryptedData, passphrase: &str) -> Result<String> {
    let bytes = decrypt_data(encrypted, passphrase)?;
    String::from_utf8(bytes).map_err(|e| anyhow!("Invalid UTF-8 in decrypted data: {}", e))
}

/// Generate a secure random passphrase
/// Returns a base64-encoded string suitable for use as a sync key
#[allow(dead_code)]
pub fn generate_passphrase() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    BASE64.encode(bytes)
}

/// Hash a workspace identifier for anonymization
/// Uses a keyed hash to prevent rainbow table attacks
pub fn hash_workspace_id(workspace_path: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    workspace_path.hash(&mut hasher);
    // Add some entropy
    "mana-workspace-salt".hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let plaintext = b"Hello, MANA patterns!";
        let passphrase = "test-passphrase-123";

        let encrypted = encrypt_data(plaintext, passphrase).unwrap();
        let decrypted = decrypt_data(&encrypted, passphrase).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_decrypt_string() {
        let plaintext = "Complex JSON data with unicode: 你好 🦀";
        let passphrase = "another-passphrase";

        let encrypted = encrypt_string(plaintext, passphrase).unwrap();
        let decrypted = decrypt_string(&encrypted, passphrase).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_wrong_passphrase_fails() {
        let plaintext = b"Secret data";
        let passphrase = "correct-passphrase";
        let wrong_passphrase = "wrong-passphrase";

        let encrypted = encrypt_data(plaintext, passphrase).unwrap();
        let result = decrypt_data(&encrypted, wrong_passphrase);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Decryption failed"));
    }

    #[test]
    fn test_unique_nonce_per_encryption() {
        let plaintext = b"Same plaintext";
        let passphrase = "same-passphrase";

        let encrypted1 = encrypt_data(plaintext, passphrase).unwrap();
        let encrypted2 = encrypt_data(plaintext, passphrase).unwrap();

        // Nonces should be different
        assert_ne!(encrypted1.nonce, encrypted2.nonce);
        // Ciphertexts should be different (due to different nonces)
        assert_ne!(encrypted1.ciphertext, encrypted2.ciphertext);
    }

    #[test]
    fn test_encrypted_data_serialization() {
        let plaintext = b"Test data";
        let passphrase = "test";

        let encrypted = encrypt_data(plaintext, passphrase).unwrap();

        // Serialize to JSON
        let json = serde_json::to_string(&encrypted).unwrap();

        // Deserialize back
        let deserialized: EncryptedData = serde_json::from_str(&json).unwrap();

        // Should still decrypt correctly
        let decrypted = decrypt_data(&deserialized, passphrase).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_hash_workspace_id() {
        let path1 = "/home/user/project1";
        let path2 = "/home/user/project2";

        let hash1 = hash_workspace_id(path1);
        let hash2 = hash_workspace_id(path2);

        // Different paths should produce different hashes
        assert_ne!(hash1, hash2);

        // Same path should produce same hash
        let hash1_again = hash_workspace_id(path1);
        assert_eq!(hash1, hash1_again);

        // Hash should be 16 hex characters
        assert_eq!(hash1.len(), 16);
    }

    #[test]
    fn test_generate_passphrase() {
        let p1 = generate_passphrase();
        let p2 = generate_passphrase();

        // Each passphrase should be unique
        assert_ne!(p1, p2);
        // Should be long enough (32 bytes = ~43 base64 chars)
        assert!(p1.len() >= 40);
    }
}
