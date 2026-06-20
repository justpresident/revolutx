//! Public error model.
//!
//! Every fallible SDK operation returns [`Result<T>`], an alias for
//! `std::result::Result<T, Error>`. [`Error`] classifies the failure modes a
//! trading bot needs to react to differently: configuration mistakes, missing
//! credentials, key/signing problems, transport failures, (de)serialization
//! failures, and structured API errors (including rate limiting).

use std::time::Duration;

/// Crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Error returned by the SDK.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Invalid client or request configuration (for example, an empty base URL
    /// or only one half of the API credentials being supplied).
    #[error("configuration error: {message}")]
    Configuration {
        /// Human-readable description of the configuration problem.
        message: String,
    },

    /// An authenticated endpoint was called on a client that was built without
    /// API credentials.
    #[error(
        "missing credentials: this request must be signed, but the client was built without an API key and private key"
    )]
    MissingCredentials,

    /// The Ed25519 private key could not be parsed or loaded.
    #[error("invalid private key: {message}")]
    Key {
        /// Description of why the key could not be loaded.
        message: String,
    },

    /// A request signature could not be produced.
    #[error("signing error: {message}")]
    Signing {
        /// Description of the signing failure.
        message: String,
    },

    /// Local validation of a request rejected obviously invalid input, such as
    /// an empty symbol or a non-positive price or size.
    #[error("invalid request: {message}")]
    InvalidRequest {
        /// Description of the validation failure.
        message: String,
    },

    /// The HTTP request failed at the transport layer (DNS, TLS, connection, or
    /// timeout). The original `reqwest` error is preserved as the source.
    #[cfg(feature = "rest")]
    #[error("transport error: {0}")]
    Transport(#[source] reqwest::Error),

    /// A request body could not be serialized to JSON.
    #[error("failed to serialize request body: {0}")]
    Serialize(#[source] serde_json::Error),

    /// A successful HTTP response body could not be deserialized into the
    /// expected model.
    #[error("failed to deserialize response from {method} {path}: {source}")]
    Deserialize {
        /// HTTP method of the originating request.
        method: String,
        /// Request path of the originating request.
        path: String,
        /// The underlying serde error.
        #[source]
        source: serde_json::Error,
        /// The (possibly truncated) response body that failed to parse.
        body: String,
    },

    /// The Revolut X API returned a structured error response.
    #[error("{0}")]
    Api(#[from] ApiError),

    /// Communication with the signing agent failed: the socket could not be
    /// reached, a frame could not be encoded/decoded, or the agent itself
    /// reported an execution error.
    #[cfg(feature = "agent")]
    #[error("agent error: {message}")]
    Agent {
        /// Description of the agent communication or execution failure.
        message: String,
    },

    /// The server returned a status or body the SDK could not classify as a
    /// normal API error.
    #[error("unexpected response: HTTP {status}: {body}")]
    Unexpected {
        /// HTTP status code.
        status: u16,
        /// The (possibly truncated) response body.
        body: String,
    },
}

impl Error {
    #[cfg(feature = "rest")]
    pub(crate) fn configuration(message: impl Into<String>) -> Self {
        Self::Configuration {
            message: message.into(),
        }
    }

    #[cfg(feature = "rest")]
    pub(crate) fn key(message: impl Into<String>) -> Self {
        Self::Key {
            message: message.into(),
        }
    }

    #[cfg(feature = "agent")]
    pub(crate) fn agent(message: impl Into<String>) -> Self {
        Self::Agent {
            message: message.into(),
        }
    }

    pub(crate) fn invalid_request(message: impl Into<String>) -> Self {
        Self::InvalidRequest {
            message: message.into(),
        }
    }

    /// Returns the HTTP status code if this error originated from a server
    /// response.
    pub const fn status(&self) -> Option<u16> {
        match self {
            Self::Api(api) => Some(api.status),
            Self::Unexpected { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// Returns the structured API error, if any.
    pub const fn api_error(&self) -> Option<&ApiError> {
        match self {
            Self::Api(api) => Some(api),
            _ => None,
        }
    }

    /// Returns `true` if the server reported a rate-limit (HTTP 429) error.
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, Self::Api(api) if api.kind == ApiErrorKind::RateLimited)
    }

    /// Returns `true` if the error is an authentication or authorization
    /// failure (HTTP 401 or 403), or a locally-detected missing credential.
    pub const fn is_auth_error(&self) -> bool {
        match self {
            Self::MissingCredentials => true,
            Self::Api(api) => {
                matches!(
                    api.kind,
                    ApiErrorKind::Unauthorized | ApiErrorKind::Forbidden
                )
            }
            _ => false,
        }
    }

    /// For rate-limit errors, returns the server-advised delay before retrying,
    /// parsed from the `Retry-After` header.
    pub fn retry_after(&self) -> Option<Duration> {
        self.api_error().and_then(|api| api.retry_after)
    }
}

/// A structured error response returned by the Revolut X API.
#[derive(Debug, Clone)]
pub struct ApiError {
    /// HTTP status code of the response.
    pub status: u16,
    /// Coarse classification of the error derived from the status code.
    pub kind: ApiErrorKind,
    /// Human-readable message from the API (`message` field), or the HTTP
    /// status reason if the body could not be parsed.
    pub message: String,
    /// Unique identifier for this error occurrence (`error_id` field).
    pub error_id: Option<String>,
    /// Server timestamp of the error in Unix epoch milliseconds.
    pub timestamp: Option<i64>,
    /// For rate-limit errors, the advised delay before retrying, parsed from
    /// the `Retry-After` header.
    pub retry_after: Option<Duration>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "API error (HTTP {} {}): {}",
            self.status, self.kind, self.message
        )?;
        if let Some(id) = &self.error_id {
            write!(f, " [error_id={id}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for ApiError {}

/// Coarse classification of an [`ApiError`], derived from the HTTP status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ApiErrorKind {
    /// HTTP 400 — malformed or rejected request.
    BadRequest,
    /// HTTP 401 — authentication failed.
    Unauthorized,
    /// HTTP 403 — authenticated but not permitted.
    Forbidden,
    /// HTTP 404 — resource not found.
    NotFound,
    /// HTTP 409 — request conflict (e.g. a stale or future timestamp).
    Conflict,
    /// HTTP 429 — rate limit exceeded.
    RateLimited,
    /// HTTP 5xx — server-side error.
    Server,
    /// Any other client-error status.
    Other,
}

#[cfg(feature = "rest")]
impl ApiErrorKind {
    const fn from_status(status: u16) -> Self {
        match status {
            400 => Self::BadRequest,
            401 => Self::Unauthorized,
            403 => Self::Forbidden,
            404 => Self::NotFound,
            409 => Self::Conflict,
            429 => Self::RateLimited,
            500..=599 => Self::Server,
            _ => Self::Other,
        }
    }
}

impl std::fmt::Display for ApiErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::BadRequest => "bad request",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not found",
            Self::Conflict => "conflict",
            Self::RateLimited => "rate limited",
            Self::Server => "server error",
            Self::Other => "error",
        };
        f.write_str(s)
    }
}

/// The wire shape of a Revolut X error payload (`ErrorResponse` schema).
#[cfg(feature = "rest")]
#[derive(Debug, serde::Deserialize)]
struct ErrorPayload {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    error_id: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[cfg(feature = "rest")]
const MAX_BODY_PREVIEW: usize = 2048;

#[cfg(feature = "rest")]
fn truncate_body(body: &str) -> String {
    if body.len() <= MAX_BODY_PREVIEW {
        body.to_owned()
    } else {
        let mut end = MAX_BODY_PREVIEW;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}… ({} bytes total)", &body[..end], body.len())
    }
}

/// Builds the appropriate [`Error`] from a non-success HTTP response.
///
/// If the body parses as the documented error payload, an [`Error::Api`] is
/// produced; otherwise an [`Error::Unexpected`] preserves the raw body.
#[cfg(feature = "rest")]
pub(crate) fn classify_error_response(
    status: u16,
    retry_after: Option<Duration>,
    body: &[u8],
) -> Error {
    match serde_json::from_slice::<ErrorPayload>(body) {
        Ok(payload) if payload.message.is_some() || payload.error_id.is_some() => {
            Error::Api(ApiError {
                status,
                kind: ApiErrorKind::from_status(status),
                message: payload
                    .message
                    .unwrap_or_else(|| reason_phrase(status).to_owned()),
                error_id: payload.error_id,
                timestamp: payload.timestamp,
                retry_after,
            })
        }
        _ => Error::Unexpected {
            status,
            body: truncate_body(&String::from_utf8_lossy(body)),
        },
    }
}

#[cfg(feature = "rest")]
const fn reason_phrase(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Error",
    }
}

#[cfg(all(test, feature = "rest"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn err_body(status: u16) -> Error {
        let body = br#"{"message":"No such pair: BTC-BTC","error_id":"7d85b5e7-d0f0-4696-b7b5-a300d0d03a5e","timestamp":3318215482991}"#;
        classify_error_response(status, None, body)
    }

    #[test]
    fn classifies_known_status_codes() {
        let cases = [
            (400u16, ApiErrorKind::BadRequest),
            (401, ApiErrorKind::Unauthorized),
            (403, ApiErrorKind::Forbidden),
            (404, ApiErrorKind::NotFound),
            (409, ApiErrorKind::Conflict),
            (429, ApiErrorKind::RateLimited),
            (503, ApiErrorKind::Server),
        ];
        for (status, kind) in cases {
            let err = err_body(status);
            let api = err.api_error().expect("structured api error");
            assert_eq!(api.status, status);
            assert_eq!(api.kind, kind);
            assert_eq!(api.message, "No such pair: BTC-BTC");
            assert_eq!(err.status(), Some(status));
        }
    }

    #[test]
    fn detects_rate_limit_and_retry_after() {
        let err = classify_error_response(
            429,
            Some(Duration::from_secs(5)),
            br#"{"message":"Rate Limit Exceeded","error_id":"x"}"#,
        );
        assert!(err.is_rate_limited());
        assert_eq!(err.retry_after(), Some(Duration::from_secs(5)));
    }

    #[test]
    fn detects_auth_errors() {
        assert!(err_body(401).is_auth_error());
        assert!(err_body(403).is_auth_error());
        assert!(Error::MissingCredentials.is_auth_error());
        assert!(!err_body(400).is_auth_error());
    }

    #[test]
    fn non_json_body_becomes_unexpected() {
        let err = classify_error_response(502, None, b"<html>bad gateway</html>");
        assert!(matches!(err, Error::Unexpected { status: 502, .. }));
        assert_eq!(err.status(), Some(502));
        assert!(err.api_error().is_none());
    }
}
