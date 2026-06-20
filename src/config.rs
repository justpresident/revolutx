//! Build a [`RevolutXClient`] from credentials and a target environment, with a
//! fallback to the `REVOLUTX_*` environment variables.
//!
//! All values come from explicit fields (e.g. CLI flags) first, falling back to
//! the environment. Building with no credentials yields a public-only client
//! (only the unauthenticated market-data endpoints work). This is the loader
//! shared by the interface crates (MCP, CLI) and the examples.

use crate::{Environment, RevolutXClient};

const ENV_API_KEY: &str = "REVOLUTX_API_KEY";
const ENV_PRIVATE_KEY_PEM: &str = "REVOLUTX_PRIVATE_KEY_PEM";
const ENV_PRIVATE_KEY_PATH: &str = "REVOLUTX_PRIVATE_KEY_PATH";
const ENV_ENVIRONMENT: &str = "REVOLUTX_ENVIRONMENT";

/// Error building a client from configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The private key file could not be read.
    #[error("could not read private key file '{path}': {source}")]
    KeyFile {
        /// Path that failed to read.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// An API key was provided without a private key.
    #[error(
        "an API key was provided but no private key (set {ENV_PRIVATE_KEY_PEM} or {ENV_PRIVATE_KEY_PATH})"
    )]
    MissingPrivateKey,
    /// A private key was provided without an API key.
    #[error("a private key was provided but no API key (set {ENV_API_KEY})")]
    MissingApiKey,
    /// The underlying client builder rejected the configuration.
    #[error(transparent)]
    Client(#[from] crate::Error),
}

/// Resolved configuration for building a [`RevolutXClient`].
///
/// Construct from CLI flags, then call [`ClientConfig::or_env`] to fill any
/// unset field from the environment, or use [`ClientConfig::from_env`] /
/// [`client_from_env`] to read everything from the environment.
#[derive(Debug, Default, Clone)]
pub struct ClientConfig {
    /// Target environment (defaults to [`Environment::Production`] when unset).
    pub environment: Option<Environment>,
    /// API key.
    pub api_key: Option<String>,
    /// Private key PEM contents (takes precedence over `private_key_path`).
    pub private_key_pem: Option<String>,
    /// Path to a private key PEM file.
    pub private_key_path: Option<String>,
}

impl ClientConfig {
    /// Reads every field from the `REVOLUTX_*` environment variables.
    pub fn from_env() -> Self {
        Self {
            environment: env_var(ENV_ENVIRONMENT).and_then(|v| parse_environment(&v)),
            api_key: env_var(ENV_API_KEY),
            private_key_pem: env_var(ENV_PRIVATE_KEY_PEM),
            private_key_path: env_var(ENV_PRIVATE_KEY_PATH),
        }
    }

    /// Fills any field left `None` from the environment. Explicit values win.
    #[must_use]
    pub fn or_env(mut self) -> Self {
        let env = Self::from_env();
        self.environment = self.environment.or(env.environment);
        self.api_key = self.api_key.or(env.api_key);
        self.private_key_pem = self.private_key_pem.or(env.private_key_pem);
        self.private_key_path = self.private_key_path.or(env.private_key_path);
        self
    }

    /// Builds a [`RevolutXClient`]. With no credentials a public-only client is
    /// returned; with exactly one half of the credentials, an error.
    pub fn build(self) -> Result<RevolutXClient, ConfigError> {
        let environment = self.environment.unwrap_or(Environment::Production);
        let mut builder = RevolutXClient::builder().environment(environment);

        let pem = match self.private_key_pem {
            Some(pem) => Some(pem),
            None => match self.private_key_path {
                Some(path) => Some(
                    std::fs::read_to_string(&path)
                        .map_err(|source| ConfigError::KeyFile { path, source })?,
                ),
                None => None,
            },
        };

        match (self.api_key, pem) {
            (Some(api_key), Some(pem)) => {
                builder = builder.api_key(api_key).private_key_pem(pem);
            }
            (None, None) => {}
            (Some(_), None) => return Err(ConfigError::MissingPrivateKey),
            (None, Some(_)) => return Err(ConfigError::MissingApiKey),
        }

        Ok(builder.build()?)
    }
}

/// Convenience: build a client entirely from the environment.
pub fn client_from_env() -> Result<RevolutXClient, ConfigError> {
    ClientConfig::from_env().build()
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Parses `REVOLUTX_ENVIRONMENT`. Unrecognized values are treated as unset.
fn parse_environment(value: &str) -> Option<Environment> {
    match value.trim().to_ascii_lowercase().as_str() {
        "dev" | "development" => Some(Environment::Dev),
        "prod" | "production" => Some(Environment::Production),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_environment_aliases() {
        assert_eq!(parse_environment("dev"), Some(Environment::Dev));
        assert_eq!(parse_environment("DEVELOPMENT"), Some(Environment::Dev));
        assert_eq!(parse_environment("prod"), Some(Environment::Production));
        assert_eq!(
            parse_environment("Production"),
            Some(Environment::Production)
        );
        assert_eq!(parse_environment("mainnet"), None);
    }

    #[test]
    fn no_credentials_builds_public_client() {
        let client = ClientConfig::default().build().unwrap();
        assert!(!client.is_authenticated());
        assert_eq!(client.base_url(), Environment::Production.base_url());
    }

    #[test]
    fn dev_environment_is_applied() {
        let client = ClientConfig {
            environment: Some(Environment::Dev),
            ..Default::default()
        }
        .build()
        .unwrap();
        assert_eq!(client.base_url(), Environment::Dev.base_url());
    }

    #[test]
    fn half_credentials_are_rejected() {
        let err = ClientConfig {
            api_key: Some("k".into()),
            ..Default::default()
        }
        .build()
        .unwrap_err();
        assert!(matches!(err, ConfigError::MissingPrivateKey));

        let err = ClientConfig {
            private_key_pem: Some("pem".into()),
            ..Default::default()
        }
        .build()
        .unwrap_err();
        assert!(matches!(err, ConfigError::MissingApiKey));
    }

    #[test]
    fn unreadable_key_path_is_reported() {
        let err = ClientConfig {
            api_key: Some("k".into()),
            private_key_path: Some("/nonexistent/revolutx-key.pem".into()),
            ..Default::default()
        }
        .build()
        .unwrap_err();
        assert!(matches!(err, ConfigError::KeyFile { .. }));
    }
}
