//! Authentication and request-signing primitives.
//!
//! Revolut X requests are signed with Ed25519. The implementation will live
//! here so endpoint modules do not handle API key, timestamp, or signature
//! headers directly.

/// Authentication headers required by Revolut X.
#[allow(dead_code)]
pub(crate) const API_KEY_HEADER: &str = "X-Revx-API-Key";
#[allow(dead_code)]
pub(crate) const TIMESTAMP_HEADER: &str = "X-Revx-Timestamp";
#[allow(dead_code)]
pub(crate) const SIGNATURE_HEADER: &str = "X-Revx-Signature";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_names_match_revolutx_spec() {
        assert_eq!(API_KEY_HEADER, "X-Revx-API-Key");
        assert_eq!(TIMESTAMP_HEADER, "X-Revx-Timestamp");
        assert_eq!(SIGNATURE_HEADER, "X-Revx-Signature");
    }
}
