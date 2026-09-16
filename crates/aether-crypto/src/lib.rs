//! Cryptographic compatibility helpers shared by Aether services.
//!
//! The Fernet API in this crate is intentionally compatible with the existing
//! Python implementation. It accepts a padded 32-byte Fernet key or derives a
//! key from a passphrase with the fixed application salt and PBKDF2 settings.
//! The compatibility ciphertext format applies URL-safe Base64 twice. The RSA
//! helpers use AWS-LC for PKCS#1 v1.5 signatures with SHA-256.
//!
//! This crate only handles the key supplied by its caller. Key lookup,
//! fallback ordering, rotation, and storage policy are responsibilities of the
//! calling service.
#![warn(missing_docs)]

mod python_fernet;
mod rsa_pkcs1_sha256;

pub use python_fernet::{
    decrypt_python_fernet_ciphertext, derive_python_fernet_key, encrypt_python_fernet_plaintext,
    looks_like_python_fernet_ciphertext, warm_python_fernet_secret, PythonFernetCompat,
    PythonFernetError, APP_SALT_HEX, APP_SALT_SEED, DEVELOPMENT_ENCRYPTION_KEY,
};
pub use rsa_pkcs1_sha256::{rsa_pkcs1_sha256_sign, rsa_pkcs1_sha256_verify, RsaPkcs1Sha256Error};
