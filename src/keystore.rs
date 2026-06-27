//! Encrypted credential vault (`keystore` feature).
//!
//! Credentials live in rcypher's own [`SecretStore`] — the *standard* rcypher
//! storage format, so the same vault can be inspected and managed with the
//! `rcypher` command-line tool (rotate the API key, add a FIDO2 security key,
//! change the password). The store is held inside rcypher's multi-factor
//! [`UnlockedContainer`] facade, which gives it:
//!
//! - **Multi-factor unlock** — one or more passwords and/or FIDO2 security keys,
//!   combined by an access policy (`pass or key`, `pass and key`, …).
//! - **Double encryption** — each value is individually encrypted under the
//!   store's data key ([`EncryptedValue`]) *inside* the encrypted container, so a
//!   value's plaintext never sits in the decrypted store payload; we decrypt one
//!   record at a time, on demand.
//!
//! The API key and Ed25519 private key are stored as the named records
//! [`API_KEY`](Keystore::API_KEY) and [`PRIVATE_KEY_PEM`](Keystore::PRIVATE_KEY_PEM).
//! [`Keystore`] exposes the store as a [`Signer`]: on **every** request the two
//! records are decrypted, the request is signed, and the plaintext is wiped
//! immediately — credentials never sit decrypted between requests.
//!
//! # Security model
//!
//! This follows `rcypher`'s threat model: data protected **at rest** plus
//! best-effort runtime hardening. Out of scope: a compromised OS,
//! malware/keyloggers, a privileged (root) attacker, and side channels.
//!
//! Process hardening is the **binary's** responsibility, not this library's (a
//! library must never `fork()` as a side effect): an unlocking process should
//! call [`disable_core_dumps`] and [`enable_ptrace_protection`] at the top of
//! `main`, **before** starting any threads/async runtime
//! ([`enable_ptrace_protection`] forks on Linux). These are re-exported here for
//! convenience. rcypher's cipher also refuses to operate while a *foreign*
//! debugger is attached (the legitimate watchdog parent installed by
//! [`enable_ptrace_protection`] is recognised and allowed).

use std::path::Path;

use rcypher::{EncryptedValue, SecretStore, UnlockedContainer};
use zeroize::Zeroizing;

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

/// Error creating or using an encrypted vault.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum KeystoreError {
    /// A store operation (create, read, write, or save) failed.
    #[error("vault store error: {0}")]
    Store(String),
    /// Key generation failed.
    #[error("key generation error: {0}")]
    Crypto(String),
}

/// An encrypted credential store over rcypher's [`SecretStore`].
///
/// Create a new one with [`create`](Keystore::create), or wrap an
/// already-unlocked container with [`from_unlocked`](Keystore::from_unlocked)
/// (the binary drives the interactive unlock — passwords and/or FIDO2 — with
/// rcypher's `cli` helpers). Read and write records with [`get`](Keystore::get) /
/// [`set`](Keystore::set); each value is encrypted/decrypted under the store's
/// data key, so plaintext never sits in the in-memory store. Persist with
/// [`save`](Keystore::save).
///
/// It implements [`Signer`] by decrypting the [`API_KEY`](Keystore::API_KEY) and
/// [`PRIVATE_KEY_PEM`](Keystore::PRIVATE_KEY_PEM) records on each request.
pub struct Keystore {
    store: UnlockedContainer<SecretStore>,
}

impl Keystore {
    /// Record name for the Revolut X API key.
    pub const API_KEY: &'static str = "api_key";
    /// Record name for the Ed25519 private key (PKCS#8 PEM).
    pub const PRIVATE_KEY_PEM: &'static str = "private_key_pem";

    /// Wraps an already-unlocked rcypher container. The caller (the binary) loads
    /// the file as a [`rcypher::LockedContainer`], satisfies its factors, and
    /// [`unlock`](rcypher::LockedContainer::unlock)s it into the container passed
    /// here.
    #[must_use]
    pub const fn from_unlocked(store: UnlockedContainer<SecretStore>) -> Self {
        Self { store }
    }

    /// Creates a brand-new, empty store protected by a single password factor.
    /// Populate it with [`set`](Self::set) and persist it with
    /// [`save`](Self::save). Enroll additional factors (e.g. a FIDO2 key) via
    /// [`container_mut`](Self::container_mut).
    pub fn create(
        factor_name: &str,
        password: &str,
        argon2: &Argon2Params,
    ) -> std::result::Result<Self, KeystoreError> {
        let store = UnlockedContainer::create_with_params(
            factor_name,
            password,
            SecretStore::new(),
            argon2,
        )
        .map_err(|e| KeystoreError::Store(e.to_string()))?;
        Ok(Self { store })
    }

    /// The underlying unlocked container, for factor management
    /// (enroll/policy/remove) and inspection.
    #[must_use]
    pub const fn container(&self) -> &UnlockedContainer<SecretStore> {
        &self.store
    }

    /// Mutable access to the underlying container, for factor management.
    #[must_use]
    pub const fn container_mut(&mut self) -> &mut UnlockedContainer<SecretStore> {
        &mut self.store
    }

    /// Reads a record's latest value, or `None` if it is not set. The value is
    /// decrypted transiently into a [`Zeroizing`] buffer.
    pub fn get(&self, name: &str) -> std::result::Result<Option<Zeroizing<String>>, KeystoreError> {
        let cypher = self.store.cypher();
        self.store
            .data()
            .latest(name)
            .map(|entry| entry.value.decrypt(&cypher))
            .transpose()
            .map_err(|e| KeystoreError::Store(e.to_string()))
    }

    /// Inserts (or appends a new version of) a record, encrypting the value under
    /// the store's data key. Call [`save`](Self::save) to persist it.
    pub fn set(&mut self, name: &str, value: &str) -> std::result::Result<(), KeystoreError> {
        let encrypted = EncryptedValue::encrypt(&self.store.cypher(), value)
            .map_err(|e| KeystoreError::Store(e.to_string()))?;
        self.store.data_mut().put(name.to_owned(), encrypted);
        Ok(())
    }

    /// Writes the store to `path` atomically, in rcypher's current format.
    pub fn save(&mut self, path: &Path) -> std::result::Result<(), KeystoreError> {
        self.store
            .save(path)
            .map_err(|e| KeystoreError::Store(e.to_string()))
    }
}

impl Signer for Keystore {
    fn authenticate(&self, message: &[u8]) -> Result<RequestAuth> {
        // Decrypt the two records for this request only; each plaintext lives in a
        // zeroizing buffer wiped at the end of this scope, as is the ephemeral
        // signing key (ed25519-dalek's zeroize feature).
        let cypher = self.store.cypher();
        let data = self.store.data();
        let decrypt = |name: &'static str| -> Result<Zeroizing<String>> {
            let entry = data.latest(name).ok_or_else(|| Error::Signing {
                message: format!("vault has no '{name}' record"),
            })?;
            entry.value.decrypt(&cypher).map_err(|e| Error::Signing {
                message: format!("vault decrypt of '{name}' failed: {e}"),
            })
        };
        let api_key = decrypt(Self::API_KEY)?;
        let private_key_pem = decrypt(Self::PRIVATE_KEY_PEM)?;
        let signer = Ed25519Signer::from_pem(api_key.as_str(), private_key_pem.as_str())?;
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
    use rcypher::LockedContainer;

    const TEST_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
        MC4CAQAwBQYDK2VwBCIEIFMSbiie3sYstkM3gSCUb+oVO5xucWXdyv9l4k2pRrZ0\n\
        -----END PRIVATE KEY-----\n";
    // GET /balances, ts = 1700000000000 — the same OpenSSL golden vector the
    // auth tests use, so this proves the keystore signs identically.
    const GET_BALANCES_SIG: &str =
        "GZOMBk8Dy8QYI/esfxUSuZW6aDsPD/Yt12eX0xmjDsYR9GIqUSBolSNiP0ZUWvSQvD5oKUlq+LGqAoT/H1hBBg==";

    fn params() -> Argon2Params {
        // Fast KDF so the suite runs quickly and works in CI.
        Argon2Params::insecure()
    }

    /// Builds an in-memory store with the two credentials, returning its bytes in
    /// rcypher's on-disk format (as `vault init` would write).
    fn store_bytes(password: &str, api_key: &str, pem: &str) -> Vec<u8> {
        let mut ks = Keystore::create("primary", password, &params()).unwrap();
        ks.set(Keystore::API_KEY, api_key).unwrap();
        ks.set(Keystore::PRIVATE_KEY_PEM, pem).unwrap();
        ks.container().to_vec().unwrap()
    }

    /// Unlocks store `bytes` with `password` (the non-interactive equivalent of
    /// the CLI's unlock loop).
    fn open(bytes: &[u8], password: &str) -> std::result::Result<Keystore, String> {
        let mut locked =
            LockedContainer::from_slice_with_params(bytes, &params()).map_err(|e| e.to_string())?;
        if !locked.try_password(password).map_err(|e| e.to_string())? || !locked.can_unlock() {
            return Err("wrong password".to_string());
        }
        let unlocked = locked.unlock::<SecretStore>().map_err(|e| e.to_string())?;
        Ok(Keystore::from_unlocked(unlocked))
    }

    #[test]
    fn vault_round_trips_and_signs_identically() {
        let bytes = store_bytes("master-pw", "api-key", TEST_PEM);
        let keystore = open(&bytes, "master-pw").unwrap();

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

        let bytes = store_bytes("pw", "api-key", &generated.private_pem);
        let keystore = open(&bytes, "pw").unwrap();
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
        let bytes = store_bytes("right", "k", TEST_PEM);
        assert!(open(&bytes, "wrong").is_err());
    }

    #[test]
    fn records_round_trip_through_a_file_and_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.rcx");

        let mut ks = Keystore::create("primary", "pw", &params()).unwrap();
        assert!(ks.get("api_key").unwrap().is_none(), "empty to start");
        ks.set("api_key", "first").unwrap();
        ks.set("api_key", "second").unwrap(); // newer version wins
        ks.save(&path).unwrap();

        let reopened = open(&std::fs::read(&path).unwrap(), "pw").unwrap();
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
        let bytes = store_bytes("pw", "k", TEST_PEM);
        let keystore = open(&bytes, "pw").unwrap();

        let client = crate::RevolutXClient::builder()
            .signer(Arc::new(keystore))
            .build()
            .unwrap();
        assert!(client.is_authenticated());
    }
}
