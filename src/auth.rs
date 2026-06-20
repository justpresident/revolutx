//! Authentication and request signing.
//!
//! Revolut X authenticates every private request with an Ed25519 signature over
//! a canonical message. This module owns key loading and signature construction
//! so that endpoint code never touches the API key, timestamp, or signature
//! headers directly.
//!
//! # Signature message
//!
//! The signed message is the exact byte concatenation of:
//!
//! 1. the timestamp in Unix epoch milliseconds (as decimal digits),
//! 2. the uppercase HTTP method,
//! 3. the request path starting from `/api/1.0`,
//! 4. the query string without a leading `?` (empty if absent),
//! 5. the minified JSON request body (empty if absent).
//!
//! The bytes used for the body and query when signing are the exact bytes sent
//! on the wire, so the signature always matches the transmitted request.

use std::borrow::Cow;
use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::pkcs8::DecodePrivateKey;
use ed25519_dalek::{Signer as _, SigningKey};

use crate::error::{Error, Result};

/// Header carrying the API key.
pub(crate) const API_KEY_HEADER: &str = "X-Revx-API-Key";
/// Header carrying the request timestamp (Unix epoch milliseconds).
pub(crate) const TIMESTAMP_HEADER: &str = "X-Revx-Timestamp";
/// Header carrying the base64-encoded Ed25519 signature.
pub(crate) const SIGNATURE_HEADER: &str = "X-Revx-Signature";

/// Authenticates Revolut X requests: supplies the API key and signs the
/// canonical message.
///
/// The transport calls [`Signer::api_key`] and [`Signer::sign`] **once per
/// request**, so an implementation may fetch or decrypt key material on demand
/// (and zeroize it immediately after). The default implementation is
/// [`Ed25519Signer`], which keeps the key in memory; custom implementations can
/// back the signing with an encrypted keystore, a hardware token, or a remote
/// signer.
pub trait Signer: Send + Sync {
    /// The API key to send in the `X-Revx-API-Key` header.
    fn api_key(&self) -> Cow<'_, str>;

    /// Signs the canonical message, returning the base64-encoded signature.
    fn sign(&self, message: &[u8]) -> Result<String>;
}

/// The default [`Signer`]: holds the API key and the Ed25519 signing key in
/// memory for the lifetime of the client.
#[derive(Clone)]
pub struct Ed25519Signer {
    api_key: String,
    signing_key: SigningKey,
}

impl Ed25519Signer {
    /// Loads from a PKCS#8 PEM private key, as produced by
    /// `openssl genpkey -algorithm ed25519 -out private.pem`.
    pub fn from_pem(api_key: impl Into<String>, pem: &str) -> Result<Self> {
        let signing_key = SigningKey::from_pkcs8_pem(pem)
            .map_err(|e| Error::key(format!("could not parse PKCS#8 PEM Ed25519 key: {e}")))?;
        Ok(Self {
            api_key: api_key.into(),
            signing_key,
        })
    }

    /// Builds from the raw 32-byte Ed25519 private key seed.
    pub fn from_seed(api_key: impl Into<String>, seed: [u8; 32]) -> Self {
        Self {
            api_key: api_key.into(),
            signing_key: SigningKey::from_bytes(&seed),
        }
    }
}

impl Signer for Ed25519Signer {
    fn api_key(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.api_key)
    }

    fn sign(&self, message: &[u8]) -> Result<String> {
        let signature = self.signing_key.sign(message);
        Ok(BASE64.encode(signature.to_bytes()))
    }
}

impl fmt::Debug for Ed25519Signer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print key material.
        f.debug_struct("Ed25519Signer")
            .field("api_key", &"<redacted>")
            .field("signing_key", &"<redacted>")
            .finish()
    }
}

/// Builds the canonical message that must be signed for a request.
///
/// `method` must already be uppercase; `path` must start from `/api/1.0`;
/// `query` must be the wire query string without a leading `?` (empty if
/// absent); `body` must be the exact minified JSON bytes sent (empty if
/// absent).
pub(crate) fn signing_message(
    timestamp_millis: i64,
    method: &str,
    path: &str,
    query: &str,
    body: &[u8],
) -> Vec<u8> {
    let ts = timestamp_millis.to_string();
    let mut message =
        Vec::with_capacity(ts.len() + method.len() + path.len() + query.len() + body.len());
    message.extend_from_slice(ts.as_bytes());
    message.extend_from_slice(method.as_bytes());
    message.extend_from_slice(path.as_bytes());
    message.extend_from_slice(query.as_bytes());
    message.extend_from_slice(body);
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    // Deterministic test vectors generated independently with OpenSSL:
    //   openssl genpkey -algorithm ed25519 -out private.pem
    //   openssl pkeyutl -sign -inkey private.pem -rawin -in <message> | base64
    const TEST_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
        MC4CAQAwBQYDK2VwBCIEIFMSbiie3sYstkM3gSCUb+oVO5xucWXdyv9l4k2pRrZ0\n\
        -----END PRIVATE KEY-----\n";

    const TEST_SEED: [u8; 32] = [
        0x53, 0x12, 0x6e, 0x28, 0x9e, 0xde, 0xc6, 0x2c, 0xb6, 0x43, 0x37, 0x81, 0x20, 0x94, 0x6f,
        0xea, 0x15, 0x3b, 0x9c, 0x6e, 0x71, 0x65, 0xdd, 0xca, 0xff, 0x65, 0xe2, 0x4d, 0xa9, 0x46,
        0xb6, 0x74,
    ];

    // GET /balances, ts = 1700000000000, no query, no body.
    const GET_BALANCES_SIG: &str =
        "GZOMBk8Dy8QYI/esfxUSuZW6aDsPD/Yt12eX0xmjDsYR9GIqUSBolSNiP0ZUWvSQvD5oKUlq+LGqAoT/H1hBBg==";

    // POST /orders, ts = 1700000000000, with a minified JSON body.
    const POST_ORDERS_BODY: &str = r#"{"client_order_id":"3fa85f64-5717-4562-b3fc-2c963f66afa6","symbol":"BTC-USD","side":"buy","order_configuration":{"limit":{"quote_size":"0.1","price":"50000.50"}}}"#;
    const POST_ORDERS_SIG: &str =
        "cEvw1PP6iGMRYrG9hER2tRzVMZrsRlb5KQfjVRDeH5dqo9rjmLAcVjhk9wkR0cJ6zoKSmfU8PxPKv7TBJeQYAg==";

    #[test]
    fn header_names_match_revolutx_spec() {
        assert_eq!(API_KEY_HEADER, "X-Revx-API-Key");
        assert_eq!(TIMESTAMP_HEADER, "X-Revx-Timestamp");
        assert_eq!(SIGNATURE_HEADER, "X-Revx-Signature");
    }

    #[test]
    fn builds_exact_message_for_get_without_query_or_body() {
        let msg = signing_message(1_700_000_000_000, "GET", "/api/1.0/balances", "", b"");
        assert_eq!(msg, b"1700000000000GET/api/1.0/balances");
    }

    #[test]
    fn builds_exact_message_for_get_with_query() {
        let msg = signing_message(
            1_700_000_000_000,
            "GET",
            "/api/1.0/orders/active",
            "symbols=BTC-USD&limit=100",
            b"",
        );
        assert_eq!(
            msg,
            b"1700000000000GET/api/1.0/orders/activesymbols=BTC-USD&limit=100"
        );
    }

    #[test]
    fn builds_exact_message_for_post_with_body() {
        let msg = signing_message(
            1_700_000_000_000,
            "POST",
            "/api/1.0/orders",
            "",
            POST_ORDERS_BODY.as_bytes(),
        );
        let expected = format!("1700000000000POST/api/1.0/orders{POST_ORDERS_BODY}");
        assert_eq!(msg, expected.as_bytes());
    }

    #[test]
    fn pem_key_signs_to_known_signature() {
        let creds = Ed25519Signer::from_pem("api-key", TEST_PEM).unwrap();
        assert_eq!(creds.api_key().as_ref(), "api-key");
        let msg = signing_message(1_700_000_000_000, "GET", "/api/1.0/balances", "", b"");
        assert_eq!(creds.sign(&msg).unwrap(), GET_BALANCES_SIG);
    }

    #[test]
    fn seed_key_matches_pem_key() {
        let pem = Ed25519Signer::from_pem("k", TEST_PEM).unwrap();
        let seed = Ed25519Signer::from_seed("k", TEST_SEED);
        let msg = signing_message(1_700_000_000_000, "GET", "/api/1.0/balances", "", b"");
        assert_eq!(seed.sign(&msg).unwrap(), pem.sign(&msg).unwrap());
        assert_eq!(seed.sign(&msg).unwrap(), GET_BALANCES_SIG);
    }

    #[test]
    fn signs_post_body_to_known_signature() {
        let creds = Ed25519Signer::from_seed("k", TEST_SEED);
        let msg = signing_message(
            1_700_000_000_000,
            "POST",
            "/api/1.0/orders",
            "",
            POST_ORDERS_BODY.as_bytes(),
        );
        assert_eq!(creds.sign(&msg).unwrap(), POST_ORDERS_SIG);
    }

    #[test]
    fn signature_is_valid_base64_of_64_bytes() {
        let creds = Ed25519Signer::from_seed("k", TEST_SEED);
        let sig = creds.sign(b"anything").unwrap();
        let bytes = BASE64.decode(sig).unwrap();
        assert_eq!(bytes.len(), 64);
    }

    #[test]
    fn invalid_pem_is_rejected() {
        let err = Ed25519Signer::from_pem(
            "k",
            "-----BEGIN PRIVATE KEY-----\nnope\n-----END PRIVATE KEY-----\n",
        )
        .unwrap_err();
        assert!(matches!(err, Error::Key { .. }));
    }

    #[test]
    fn debug_does_not_leak_key_material() {
        let creds = Ed25519Signer::from_seed("super-secret-api-key", TEST_SEED);
        let rendered = format!("{creds:?}");
        assert!(!rendered.contains("super-secret-api-key"));
        assert!(rendered.contains("redacted"));
    }
}
