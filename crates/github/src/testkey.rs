//! A throwaway RSA key for the tests that build a GitHub App client.
//!
//! Generated on the fly rather than committed: a PEM file in the repository
//! looks exactly like a leaked private key to every scanner that walks past it,
//! and `.gitignore` excludes `*.pem` anyway — which is how CI ended up without
//! the file while it existed on a developer's disk.

use std::sync::OnceLock;

use rsa::RsaPrivateKey;
use rsa::pkcs1::{EncodeRsaPrivateKey, LineEnding};

/// Size of the generated key.
///
/// Deliberately small: the tests only need jsonwebtoken to accept the key and
/// sign a JWT with it, and 2048 bits cost several seconds per test binary.
/// GitHub never sees this key.
const TEST_KEY_BITS: usize = 1024;

/// A PKCS#1 PEM key, generated once per test binary and cached.
pub fn test_app_key_pem() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| {
        RsaPrivateKey::new(&mut rand_core_06::OsRng, TEST_KEY_BITS)
            .expect("generating a test RSA key")
            .to_pkcs1_pem(LineEnding::LF)
            .expect("encoding the test key as PEM")
            .to_string()
    })
    .as_str()
}
