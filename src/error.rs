//! Error types shared by the SDK.
//!
//! Later tasks should expand this module to classify transport failures,
//! signing errors, API error responses, rate limits, and response decoding
//! failures.

/// Crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Error type returned by the SDK.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Invalid client or request configuration.
    Configuration { message: String },
}

impl Error {
    pub(crate) fn configuration(message: impl Into<String>) -> Self {
        Self::Configuration {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration { message } => write!(f, "configuration error: {message}"),
        }
    }
}

impl std::error::Error for Error {}
