//! Client configuration and shared transport entry points.
//!
//! Endpoint modules should depend on `RevolutXClient` and shared internal
//! request helpers from this module instead of duplicating HTTP or signing
//! behavior.

use crate::error::{Error, Result};

/// Revolut X API environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    /// Production server using live account data.
    Production,
    /// Dev server for test data.
    Dev,
}

impl Environment {
    /// Returns the default base URL for the environment.
    pub const fn base_url(self) -> &'static str {
        match self {
            Self::Production => "https://revx.revolut.com/api/1.0",
            Self::Dev => "https://revx.revolut.codes/api/1.0",
        }
    }
}

/// Main SDK client.
#[derive(Debug, Clone)]
pub struct RevolutXClient {
    environment: Environment,
    base_url: String,
}

impl RevolutXClient {
    /// Starts configuring a new client.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Returns the configured API environment.
    pub const fn environment(&self) -> Environment {
        self.environment
    }

    /// Returns the configured base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

/// Builder for [`RevolutXClient`].
#[derive(Debug, Clone)]
pub struct ClientBuilder {
    environment: Environment,
    base_url: Option<String>,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            environment: Environment::Production,
            base_url: None,
        }
    }
}

impl ClientBuilder {
    /// Returns the currently selected environment.
    ///
    /// This exists mostly to make configuration defaults testable while the
    /// transport/auth layers are still being implemented.
    pub const fn selected_environment(&self) -> Environment {
        self.environment
    }

    /// Selects a Revolut X environment.
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = environment;
        self
    }

    /// Overrides the base URL.
    ///
    /// This is primarily for tests and advanced users. Normal callers should
    /// prefer [`ClientBuilder::environment`].
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Builds the client.
    pub fn build(self) -> Result<RevolutXClient> {
        let base_url = self
            .base_url
            .unwrap_or_else(|| self.environment.base_url().to_owned());

        if base_url.trim().is_empty() {
            return Err(Error::configuration("base URL must not be empty"));
        }

        Ok(RevolutXClient {
            environment: self.environment,
            base_url,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_environment_is_production() {
        let client = RevolutXClient::builder().build().unwrap();
        assert_eq!(client.environment(), Environment::Production);
        assert_eq!(client.base_url(), Environment::Production.base_url());
    }

    #[test]
    fn dev_environment_uses_dev_base_url() {
        let client = RevolutXClient::builder()
            .environment(Environment::Dev)
            .build()
            .unwrap();

        assert_eq!(client.base_url(), Environment::Dev.base_url());
    }

    #[test]
    fn custom_base_url_overrides_environment() {
        let client = RevolutXClient::builder()
            .environment(Environment::Dev)
            .base_url("http://127.0.0.1:8080/api/1.0")
            .build()
            .unwrap();

        assert_eq!(client.environment(), Environment::Dev);
        assert_eq!(client.base_url(), "http://127.0.0.1:8080/api/1.0");
    }

    #[test]
    fn empty_custom_base_url_is_rejected() {
        let err = RevolutXClient::builder()
            .base_url("  ")
            .build()
            .unwrap_err();
        assert!(matches!(err, Error::Configuration { .. }));
    }
}
