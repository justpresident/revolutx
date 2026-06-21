//! Encrypted credential vault (`keystore` feature).
//!
//! Stores the API key and Ed25519 private key in an [`rcypher`] vault
//! (Argon2id + AES-256-CBC + HMAC, encrypt-then-MAC) and exposes it as a
//! [`Signer`]. On **every** request the vault is decrypted, the request is
//! signed, and the plaintext is wiped immediately — credentials never sit
//! decrypted in memory between requests. The Argon2id-derived vault key does
//! remain resident for the session (the cost of not re-prompting per request).
//!
//! # Security model
//!
//! This follows `rcypher`'s model and its threat model: it protects data **at
//! rest** plus best-effort runtime hardening. Out of scope: a compromised OS,
//! malware/keyloggers, a privileged (root) attacker, and side channels.
//!
//! Process hardening is the **binary's** responsibility, not this library's
//! (a library must never `fork()` as a side effect): an unlocking process
//! should call [`disable_core_dumps`] and [`enable_ptrace_protection`] at the
//! top of `main`, **before** starting any threads/async runtime
//! ([`enable_ptrace_protection`] forks on Linux). These are re-exported here for
//! convenience. The vault's cipher also refuses to operate while a debugger is
//! attached unless [`KeystoreOptions::trace_detection`] is set to `false`.

use std::path::{Path, PathBuf};

use bincode::{Decode, Encode};
use rcypher::{Cypher, CypherVersion, EncryptionKey, save_encrypted};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::auth::{Ed25519Signer, RequestAuth, Signer};
use crate::error::{Error, Result};

pub use rcypher::{
    Argon2Params, disable_core_dumps, enable_ptrace_protection, is_debugger_attached,
};

/// A freshly generated Ed25519 key pair, PEM-encoded, from [`generate_key_pair`].
pub struct GeneratedKeyPair {
    /// PKCS#8 private key PEM. Store this in the vault; it is zeroized on drop.
    pub private_pem: Zeroizing<String>,
    /// SPKI public key PEM. Not secret — register this with Revolut X.
    pub public_pem: String,
}

/// Generates a new Ed25519 key pair from operating-system randomness.
///
/// The private key is returned as PKCS#8 PEM (the same format
/// [`Keystore::create`] and the SDK consume) and never touches the disk
/// unencrypted; the public key is returned as SPKI PEM to register with the
/// exchange.
pub fn generate_key_pair() -> std::result::Result<GeneratedKeyPair, KeystoreError> {
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    use ed25519_dalek::pkcs8::spki::EncodePublicKey;
    use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;

    // An Ed25519 signing key is a 32-byte seed; fresh OS randomness is a key.
    let mut seed = Zeroizing::new([0u8; 32]);
    getrandom::fill(seed.as_mut_slice())
        .map_err(|e| KeystoreError::Crypto(format!("could not read OS randomness: {e}")))?;
    let signing = SigningKey::from_bytes(&seed);

    let private_pem = signing
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|e| KeystoreError::Crypto(format!("could not encode private key: {e}")))?;
    let public_pem = signing
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .map_err(|e| KeystoreError::Crypto(format!("could not encode public key: {e}")))?;

    Ok(GeneratedKeyPair {
        private_pem,
        public_pem,
    })
}

/// The secrets stored in the vault. Serialized with bincode (compact binary,
/// no text-escaping of the PEM) and zeroized on drop.
#[derive(Encode, Decode, Zeroize, ZeroizeOnDrop)]
struct VaultContents {
    api_key: String,
    private_key_pem: String,
}

/// Options controlling vault key derivation and runtime hardening.
#[derive(Clone)]
pub struct KeystoreOptions {
    /// Argon2id parameters. The default is secure; use
    /// [`Argon2Params::insecure`] **only** for fast tests.
    pub argon2: Argon2Params,
    /// Whether the cipher refuses to operate while a debugger/tracer is
    /// attached. Default `true`; set `false` only on legitimately-traced hosts
    /// (CI, profilers).
    pub trace_detection: bool,
}

impl Default for KeystoreOptions {
    fn default() -> Self {
        Self {
            argon2: Argon2Params::default(),
            trace_detection: true,
        }
    }
}

impl std::fmt::Debug for KeystoreOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeystoreOptions")
            .field("trace_detection", &self.trace_detection)
            .finish_non_exhaustive()
    }
}

/// Error opening or creating an encrypted vault.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum KeystoreError {
    /// The vault file could not be read or written.
    #[error("vault file error: {0}")]
    Io(#[from] std::io::Error),
    /// The vault could not be unlocked (wrong password or corrupted file).
    #[error("could not unlock the vault (wrong password or corrupted file): {0}")]
    Unlock(String),
    /// A cryptographic operation failed.
    #[error("vault crypto error: {0}")]
    Crypto(String),
    /// The decrypted vault contents were not valid.
    #[error("invalid vault contents: {0}")]
    Contents(String),
}

/// A new vault from [`Keystore::init`]: its encryption key is derived but its
/// credentials have not been written yet. Fill it in with [`NewVault::store`].
pub struct NewVault {
    cypher: Cypher,
    path: PathBuf,
}

impl NewVault {
    /// Encrypts the API key and Ed25519 private key into the vault and writes it
    /// to disk (atomically, `0600`).
    pub fn store(
        self,
        api_key: &str,
        private_key_pem: &str,
    ) -> std::result::Result<(), KeystoreError> {
        let contents = VaultContents {
            api_key: api_key.to_owned(),
            private_key_pem: private_key_pem.to_owned(),
        };
        let plaintext = Zeroizing::new(
            bincode::encode_to_vec(&contents, bincode::config::standard())
                .map_err(|e| KeystoreError::Contents(e.to_string()))?,
        );
        // `save_encrypted` encrypts and atomically persists (0600 temp file).
        save_encrypted(&self.cypher, plaintext.as_slice(), &self.path)
            .map_err(|e| KeystoreError::Crypto(e.to_string()))?;
        Ok(())
    }
}

/// An opened encrypted credential vault, usable as a [`Signer`].
pub struct Keystore {
    cypher: Cypher,
    /// The encrypted blob (ciphertext; not secret). Re-decrypted per request.
    blob: Vec<u8>,
}

impl Keystore {
    /// Creates a new vault at `path` holding `api_key` and `private_key_pem`,
    /// encrypted with `password`. Uses secure defaults.
    pub fn create(
        path: &Path,
        password: &str,
        api_key: &str,
        private_key_pem: &str,
    ) -> std::result::Result<(), KeystoreError> {
        Self::create_with(
            path,
            password,
            api_key,
            private_key_pem,
            &KeystoreOptions::default(),
        )
    }

    /// Like [`Keystore::create`] with explicit [`KeystoreOptions`].
    pub fn create_with(
        path: &Path,
        password: &str,
        api_key: &str,
        private_key_pem: &str,
        options: &KeystoreOptions,
    ) -> std::result::Result<(), KeystoreError> {
        Self::init(path, password, options)?.store(api_key, private_key_pem)
    }

    /// Initializes a new vault at `path`, deriving its encryption key from
    /// `password` (Argon2id).
    ///
    /// The returned [`NewVault`] holds only the derived key, so the caller can
    /// **wipe the password immediately** and then write the credentials with
    /// [`NewVault::store`] once they are gathered — the password need not stay
    /// resident while, say, the user creates an API key on the exchange website.
    pub fn init(
        path: &Path,
        password: &str,
        options: &KeystoreOptions,
    ) -> std::result::Result<NewVault, KeystoreError> {
        let key = EncryptionKey::from_password_with_params(
            CypherVersion::default(),
            password,
            &options.argon2,
        )
        .map_err(|e| KeystoreError::Crypto(e.to_string()))?;
        Ok(NewVault {
            cypher: Cypher::with_trace_detection(key, options.trace_detection),
            path: path.to_owned(),
        })
    }

    /// Opens the vault at `path`, deriving the key from `password`. Uses secure
    /// defaults. Fails if the password is wrong or the file is corrupted.
    pub fn open(path: &Path, password: &str) -> std::result::Result<Self, KeystoreError> {
        Self::open_with(path, password, &KeystoreOptions::default())
    }

    /// Like [`Keystore::open`] with explicit [`KeystoreOptions`].
    pub fn open_with(
        path: &Path,
        password: &str,
        options: &KeystoreOptions,
    ) -> std::result::Result<Self, KeystoreError> {
        let blob = std::fs::read(path)?;
        let key = EncryptionKey::for_data_with_params(password, &blob, &options.argon2)
            .map_err(|e| KeystoreError::Unlock(e.to_string()))?;
        let cypher = Cypher::with_trace_detection(key, options.trace_detection);
        // Decrypt once up front so a wrong password fails here, not at first use.
        cypher
            .decrypt(&blob)
            .map_err(|e| KeystoreError::Unlock(e.to_string()))?;
        Ok(Self { cypher, blob })
    }
}

impl Signer for Keystore {
    fn authenticate(&self, message: &[u8]) -> Result<RequestAuth> {
        // Decrypt the vault for this request only; everything below is wiped
        // when the scope ends (plaintext via Zeroizing, contents via
        // ZeroizeOnDrop, the ephemeral signing key via ed25519-dalek's zeroize).
        let plaintext = self
            .cypher
            .decrypt(&self.blob)
            .map_err(|e| Error::Signing {
                message: format!("vault decrypt failed: {e}"),
            })?;
        let (contents, _): (VaultContents, usize) =
            bincode::decode_from_slice(plaintext.as_slice(), bincode::config::standard()).map_err(
                |e| Error::Signing {
                    message: format!("vault contents invalid: {e}"),
                },
            )?;
        let signer = Ed25519Signer::from_pem(contents.api_key.as_str(), &contents.private_key_pem)?;
        signer.authenticate(message)
    }
}

impl std::fmt::Debug for Keystore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keystore").finish_non_exhaustive()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::auth::signing_message;

    const TEST_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
        MC4CAQAwBQYDK2VwBCIEIFMSbiie3sYstkM3gSCUb+oVO5xucWXdyv9l4k2pRrZ0\n\
        -----END PRIVATE KEY-----\n";
    // GET /balances, ts = 1700000000000 — the same OpenSSL golden vector the
    // auth tests use, so this proves the keystore signs identically.
    const GET_BALANCES_SIG: &str =
        "GZOMBk8Dy8QYI/esfxUSuZW6aDsPD/Yt12eX0xmjDsYR9GIqUSBolSNiP0ZUWvSQvD5oKUlq+LGqAoT/H1hBBg==";

    fn test_options() -> KeystoreOptions {
        // Fast KDF + no anti-debug, so the suite runs quickly and works in CI.
        KeystoreOptions {
            argon2: Argon2Params::insecure(),
            trace_detection: false,
        }
    }

    #[test]
    fn vault_round_trips_and_signs_identically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.rcx");
        Keystore::create_with(&path, "master-pw", "api-key", TEST_PEM, &test_options()).unwrap();

        let keystore = Keystore::open_with(&path, "master-pw", &test_options()).unwrap();
        let message = signing_message(1_700_000_000_000, "GET", "/api/1.0/balances", "", b"");
        let auth = keystore.authenticate(&message).unwrap();

        assert_eq!(auth.api_key.as_str(), "api-key");
        assert_eq!(auth.signature, GET_BALANCES_SIG);
    }

    #[test]
    fn generated_key_pair_round_trips_through_a_vault() {
        let generated = generate_key_pair().unwrap();
        assert!(
            generated
                .private_pem
                .starts_with("-----BEGIN PRIVATE KEY-----")
        );
        assert!(
            generated
                .public_pem
                .starts_with("-----BEGIN PUBLIC KEY-----")
        );

        // The generated private key must drive a vault like an imported one: it
        // parses, and signing produces a stable signature for a fixed message.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.rcx");
        Keystore::create_with(
            &path,
            "pw",
            "api-key",
            &generated.private_pem,
            &test_options(),
        )
        .unwrap();
        let keystore = Keystore::open_with(&path, "pw", &test_options()).unwrap();
        let message = signing_message(1_700_000_000_000, "GET", "/api/1.0/balances", "", b"");
        let auth = keystore.authenticate(&message).unwrap();
        assert_eq!(auth.api_key.as_str(), "api-key");
        // A non-empty, base64-ish signature (64-byte Ed25519 sig -> 88 chars).
        assert_eq!(auth.signature.len(), 88);
    }

    #[test]
    fn two_generated_key_pairs_differ() {
        let a = generate_key_pair().unwrap();
        let b = generate_key_pair().unwrap();
        assert_ne!(*a.private_pem, *b.private_pem);
        assert_ne!(a.public_pem, b.public_pem);
    }

    #[test]
    fn wrong_password_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.rcx");
        Keystore::create_with(&path, "right", "k", TEST_PEM, &test_options()).unwrap();
        assert!(Keystore::open_with(&path, "wrong", &test_options()).is_err());
    }

    #[test]
    fn drives_a_client_as_signer() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.rcx");
        Keystore::create_with(&path, "pw", "k", TEST_PEM, &test_options()).unwrap();
        let keystore = Keystore::open_with(&path, "pw", &test_options()).unwrap();

        let client = crate::RevolutXClient::builder()
            .signer(Arc::new(keystore))
            .build()
            .unwrap();
        assert!(client.is_authenticated());
    }
}
