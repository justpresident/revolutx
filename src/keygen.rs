//! Ed25519 key-pair generation (`rest` feature).
//!
//! A small, dependency-light helper for onboarding: generate a fresh signing key
//! to register with Revolut X. It is rcypher-free — available whenever the REST
//! client is, independent of the `keystore` feature.

use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::spki::EncodePublicKey;
use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use ed25519_dalek::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// A freshly generated Ed25519 key pair, PEM-encoded, from [`generate_key_pair`].
pub struct GeneratedKeyPair {
    /// PKCS#8 private key PEM. Store this securely; it is zeroized on drop.
    pub private_pem: Zeroizing<String>,
    /// SPKI public key PEM. Not secret — register this with Revolut X.
    pub public_pem: String,
}

/// Generates a new Ed25519 key pair from operating-system randomness.
///
/// The private key is returned as PKCS#8 PEM (the same format the vault and the
/// SDK consume) and never touches the disk unencrypted; the public key is
/// returned as SPKI PEM to register with the exchange.
pub fn generate_key_pair() -> Result<GeneratedKeyPair> {
    // An Ed25519 signing key is a 32-byte seed; fresh OS randomness is a key.
    let mut seed = Zeroizing::new([0u8; 32]);
    getrandom::fill(seed.as_mut_slice()).map_err(|e| Error::KeyGeneration {
        message: format!("could not read OS randomness: {e}"),
    })?;
    let signing = SigningKey::from_bytes(&seed);

    let private_pem = signing
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|e| Error::KeyGeneration {
            message: format!("could not encode private key: {e}"),
        })?;
    let public_pem = signing
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .map_err(|e| Error::KeyGeneration {
            message: format!("could not encode public key: {e}"),
        })?;

    Ok(GeneratedKeyPair {
        private_pem,
        public_pem,
    })
}

/// Derives the SPKI public-key PEM from a PKCS#8 private-key PEM.
///
/// Use when only the private key is on hand — for example importing one with
/// `vault init --key-file` — to recover the matching public key to register with
/// the exchange and to store alongside the private key. It is exactly the public
/// key [`generate_key_pair`] would have returned for that private key.
pub fn public_pem_from_private_pem(private_pem: &str) -> Result<String> {
    let signing = SigningKey::from_pkcs8_pem(private_pem)
        .map_err(|e| Error::key(format!("could not parse PKCS#8 PEM Ed25519 key: {e}")))?;
    signing
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .map_err(|e| Error::KeyGeneration {
            message: format!("could not encode public key: {e}"),
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn generates_distinct_pem_pairs() {
        let a = generate_key_pair().unwrap();
        let b = generate_key_pair().unwrap();
        assert!(a.private_pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        assert!(a.public_pem.starts_with("-----BEGIN PUBLIC KEY-----"));
        assert_ne!(*a.private_pem, *b.private_pem);
        assert_ne!(a.public_pem, b.public_pem);
    }

    #[test]
    fn derives_the_matching_public_key_from_a_private_pem() {
        let pair = generate_key_pair().unwrap();
        let derived = public_pem_from_private_pem(&pair.private_pem).unwrap();
        assert_eq!(derived, pair.public_pem);
    }

    #[test]
    fn rejects_an_unparseable_private_pem() {
        assert!(public_pem_from_private_pem("-----BEGIN PRIVATE KEY-----\nnope\n").is_err());
    }
}
