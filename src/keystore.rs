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
use rcypher::{Cypher, CypherVersion, EncryptionKey};
use zeroize::{Zeroize, Zeroizing};

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
/// The private key is returned as PKCS#8 PEM (the same format the vault and the
/// SDK consume) and never touches the disk unencrypted; the public key is
/// returned as SPKI PEM to register with the exchange.
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

/// The decrypted vault contents: `name -> value` records. Serialized with
/// bincode (compact, no text-escaping of the PEM). The sensitive string contents
/// are zeroized on drop, so a decrypted copy never outlives the operation that
/// produced it.
#[derive(Encode, Decode, Default)]
struct Records(Vec<(String, String)>);

impl Records {
    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn set(&mut self, name: &str, value: &str) {
        if let Some(entry) = self.0.iter_mut().find(|(key, _)| key == name) {
            value.clone_into(&mut entry.1);
        } else {
            self.0.push((name.to_owned(), value.to_owned()));
        }
    }
}

impl Drop for Records {
    fn drop(&mut self) {
        for (name, value) in &mut self.0 {
            name.zeroize();
            value.zeroize();
        }
    }
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

/// An encrypted credential store.
///
/// Initialize it from a master password with [`Keystore::open`] — the encryption
/// key is derived once (Argon2id) and the encrypted blob is held in memory. Read
/// and write individual records with [`get`](Keystore::get) /
/// [`set`](Keystore::set); each decrypts the blob **transiently** and wipes the
/// plaintext immediately, so credentials never sit decrypted between operations.
/// Persist changes to disk with [`save`](Keystore::save).
///
/// It implements [`Signer`] by reading the [`API_KEY`](Keystore::API_KEY) and
/// [`PRIVATE_KEY_PEM`](Keystore::PRIVATE_KEY_PEM) records on each request.
pub struct Keystore {
    cypher: Cypher,
    /// The encrypted records (ciphertext; not secret) — the single in-memory copy
    /// of the vault contents.
    blob: Vec<u8>,
    path: PathBuf,
}

impl Keystore {
    /// Record name for the Revolut X API key.
    pub const API_KEY: &'static str = "api_key";
    /// Record name for the Ed25519 private key (PKCS#8 PEM).
    pub const PRIVATE_KEY_PEM: &'static str = "private_key_pem";

    /// Opens the vault at `path`, deriving the key from `password`, with secure
    /// defaults. See [`Keystore::open_with`].
    pub fn open(path: &Path, password: &str) -> std::result::Result<Self, KeystoreError> {
        Self::open_with(path, password, &KeystoreOptions::default())
    }

    /// Opens the vault at `path`, deriving the key from `password`.
    ///
    /// If the file exists it is loaded and the password verified (an error if it
    /// is wrong or the file is corrupted). If it does **not** exist, a new empty
    /// vault is initialized in memory — populate it with [`set`](Keystore::set)
    /// and write it with [`save`](Keystore::save).
    pub fn open_with(
        path: &Path,
        password: &str,
        options: &KeystoreOptions,
    ) -> std::result::Result<Self, KeystoreError> {
        let (cypher, blob) = if path.exists() {
            let blob = std::fs::read(path)?;
            let key = EncryptionKey::for_data_with_params(password, &blob, &options.argon2)
                .map_err(|e| KeystoreError::Unlock(e.to_string()))?;
            let cypher = Cypher::with_trace_detection(key, options.trace_detection);
            // Decrypt once up front so a wrong password fails here, not at first use.
            cypher
                .decrypt(&blob)
                .map_err(|e| KeystoreError::Unlock(e.to_string()))?;
            (cypher, blob)
        } else {
            let key = EncryptionKey::from_password_with_params(
                CypherVersion::default(),
                password,
                &options.argon2,
            )
            .map_err(|e| KeystoreError::Crypto(e.to_string()))?;
            let cypher = Cypher::with_trace_detection(key, options.trace_detection);
            let blob = encrypt_records(&cypher, &Records::default())?;
            (cypher, blob)
        };
        Ok(Self {
            cypher,
            blob,
            path: path.to_owned(),
        })
    }

    /// Reads a record's value, or `None` if it is not set. The value is decrypted
    /// transiently and returned in a [`Zeroizing`] wrapper.
    pub fn get(&self, name: &str) -> std::result::Result<Option<Zeroizing<String>>, KeystoreError> {
        Ok(self
            .decrypt()?
            .get(name)
            .map(|value| Zeroizing::new(value.to_owned())))
    }

    /// Inserts or replaces a record, updating the in-memory encrypted blob. Call
    /// [`save`](Keystore::save) to persist it.
    pub fn set(&mut self, name: &str, value: &str) -> std::result::Result<(), KeystoreError> {
        let mut records = self.decrypt()?;
        records.set(name, value);
        self.blob = encrypt_records(&self.cypher, &records)?;
        Ok(())
    }

    /// Writes the in-memory encrypted blob to disk, atomically and `0600`.
    pub fn save(&self) -> std::result::Result<(), KeystoreError> {
        atomic_write(&self.path, &self.blob)?;
        Ok(())
    }

    /// Decrypts the blob into its records, which are zeroized when dropped.
    fn decrypt(&self) -> std::result::Result<Records, KeystoreError> {
        let plaintext = self
            .cypher
            .decrypt(&self.blob)
            .map_err(|e| KeystoreError::Crypto(e.to_string()))?;
        let (records, _) =
            bincode::decode_from_slice(plaintext.as_slice(), bincode::config::standard())
                .map_err(|e| KeystoreError::Contents(e.to_string()))?;
        Ok(records)
    }
}

/// Encrypts `records` into a self-contained blob (header carries the salt + IV).
fn encrypt_records(
    cypher: &Cypher,
    records: &Records,
) -> std::result::Result<Vec<u8>, KeystoreError> {
    let plaintext = Zeroizing::new(
        bincode::encode_to_vec(records, bincode::config::standard())
            .map_err(|e| KeystoreError::Contents(e.to_string()))?,
    );
    cypher
        .encrypt(plaintext.as_slice())
        .map_err(|e| KeystoreError::Crypto(e.to_string()))
}

/// Atomically writes `data` to `path` with `0600` permissions: write a unique
/// temp file in the same directory, fsync, then rename it over `path`.
fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "vault path has no file name",
        )
    })?;
    let tmp_name = format!(
        ".{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id()
    );
    let tmp = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from(&tmp_name), |parent| parent.join(&tmp_name));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let write = (|| {
        let mut file = options.open(&tmp)?;
        file.write_all(data)?;
        file.sync_all()
    })();
    if let Err(e) = write {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path)
}

impl Signer for Keystore {
    fn authenticate(&self, message: &[u8]) -> Result<RequestAuth> {
        // Decrypt the vault for this request only; `records` (with the decrypted
        // secrets) is zeroized when this scope ends, as is the ephemeral signing
        // key (ed25519-dalek's zeroize feature).
        let records = self.decrypt().map_err(|e| Error::Signing {
            message: format!("vault decrypt failed: {e}"),
        })?;
        let api_key = records.get(Self::API_KEY).ok_or_else(|| Error::Signing {
            message: format!("vault has no '{}' record", Self::API_KEY),
        })?;
        let private_key_pem = records
            .get(Self::PRIVATE_KEY_PEM)
            .ok_or_else(|| Error::Signing {
                message: format!("vault has no '{}' record", Self::PRIVATE_KEY_PEM),
            })?;
        let signer = Ed25519Signer::from_pem(api_key, private_key_pem)?;
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

    /// Creates a vault at `path` with the two credentials and persists it.
    fn write_vault(path: &Path, password: &str, api_key: &str, pem: &str) {
        let mut vault = Keystore::open_with(path, password, &test_options()).unwrap();
        vault.set(Keystore::API_KEY, api_key).unwrap();
        vault.set(Keystore::PRIVATE_KEY_PEM, pem).unwrap();
        vault.save().unwrap();
    }

    #[test]
    fn vault_round_trips_and_signs_identically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.rcx");
        write_vault(&path, "master-pw", "api-key", TEST_PEM);

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
        write_vault(&path, "pw", "api-key", &generated.private_pem);
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
        write_vault(&path, "right", "k", TEST_PEM);
        assert!(Keystore::open_with(&path, "wrong", &test_options()).is_err());
    }

    #[test]
    fn records_round_trip_and_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.rcx");
        let mut vault = Keystore::open_with(&path, "pw", &test_options()).unwrap();
        assert!(vault.get("api_key").unwrap().is_none(), "empty to start");
        vault.set("api_key", "first").unwrap();
        vault.set("api_key", "second").unwrap(); // overwrite
        vault.save().unwrap();

        let reopened = Keystore::open_with(&path, "pw", &test_options()).unwrap();
        assert_eq!(
            reopened
                .get("api_key")
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("second")
        );
        assert!(reopened.get("missing").unwrap().is_none());
    }

    #[test]
    fn drives_a_client_as_signer() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.rcx");
        write_vault(&path, "pw", "k", TEST_PEM);
        let keystore = Keystore::open_with(&path, "pw", &test_options()).unwrap();

        let client = crate::RevolutXClient::builder()
            .signer(Arc::new(keystore))
            .build()
            .unwrap();
        assert!(client.is_authenticated());
    }
}
