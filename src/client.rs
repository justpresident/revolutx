//! Client configuration and endpoint entry points.
//!
//! [`RevolutXClient`] is the top-level handle. It is built with
//! [`RevolutXClient::builder`], owns the shared HTTP transport, and exposes the
//! endpoint groups: [`balances`](RevolutXClient::balances),
//! [`configuration`](RevolutXClient::configuration),
//! [`market_data`](RevolutXClient::market_data),
//! [`orders`](RevolutXClient::orders), and [`trades`](RevolutXClient::trades).
//!
//! A client may be built without credentials to access only the public market
//! data endpoints; calling an authenticated endpoint in that case returns
//! [`crate::Error::MissingCredentials`].

use std::time::Duration;

use crate::api::balances::BalancesApi;
use crate::api::configuration::ConfigurationApi;
use crate::api::market_data::MarketDataApi;
use crate::api::orders::OrdersApi;
use crate::api::trades::TradesApi;
use crate::auth::Credentials;
use crate::error::{Error, Result};
use crate::transport::Transport;

/// Default request timeout when none is configured.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Revolut X API environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    /// Production server using live account data.
    Production,
    /// Dev server using test data.
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

/// The main SDK client.
///
/// Cloning is cheap: the underlying HTTP connection pool and credentials are
/// shared.
#[derive(Debug, Clone)]
pub struct RevolutXClient {
    transport: Transport,
}

impl RevolutXClient {
    /// Starts configuring a new client.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Returns the configured base URL.
    pub fn base_url(&self) -> &str {
        self.transport.base_url()
    }

    /// Returns whether the client was configured with API credentials.
    pub fn is_authenticated(&self) -> bool {
        self.transport.has_credentials()
    }

    /// Account balance endpoints.
    pub fn balances(&self) -> BalancesApi<'_> {
        BalancesApi::new(self)
    }

    /// Exchange configuration endpoints (currencies and pairs).
    pub fn configuration(&self) -> ConfigurationApi<'_> {
        ConfigurationApi::new(self)
    }

    /// Market data endpoints (order books, candles, tickers, public trades).
    pub fn market_data(&self) -> MarketDataApi<'_> {
        MarketDataApi::new(self)
    }

    /// Order placement and management endpoints.
    pub fn orders(&self) -> OrdersApi<'_> {
        OrdersApi::new(self)
    }

    /// Trade history endpoints.
    pub fn trades(&self) -> TradesApi<'_> {
        TradesApi::new(self)
    }

    /// Internal accessor for endpoint modules.
    pub(crate) fn transport(&self) -> &Transport {
        &self.transport
    }
}

/// Source of the Ed25519 private key used for signing.
#[derive(Clone)]
enum KeySource {
    Pem(String),
    Seed([u8; 32]),
}

/// Builder for [`RevolutXClient`].
#[derive(Default)]
pub struct ClientBuilder {
    api_key: Option<String>,
    key: Option<KeySource>,
    environment: Option<Environment>,
    base_url: Option<String>,
    timeout: Option<Duration>,
    http_client: Option<reqwest::Client>,
}

impl ClientBuilder {
    /// Sets the API key (the `X-Revx-API-Key` value).
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Loads the Ed25519 private key from a PKCS#8 PEM string, as produced by
    /// `openssl genpkey -algorithm ed25519 -out private.pem`.
    pub fn private_key_pem(mut self, pem: impl Into<String>) -> Self {
        self.key = Some(KeySource::Pem(pem.into()));
        self
    }

    /// Sets the Ed25519 private key from its raw 32-byte seed (for advanced
    /// users and tests).
    pub fn private_key_bytes(mut self, seed: [u8; 32]) -> Self {
        self.key = Some(KeySource::Seed(seed));
        self
    }

    /// Selects a Revolut X environment. Ignored if a custom base URL is set.
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = Some(environment);
        self
    }

    /// Overrides the base URL (primarily for tests and advanced deployments).
    /// Must include the `/api/1.0` path prefix used by the signature.
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Sets the request timeout. Ignored if a custom HTTP client is supplied.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Supplies a preconfigured [`reqwest::Client`], taking full control of
    /// transport settings (TLS, proxies, timeouts, connection pooling).
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = Some(client);
        self
    }

    /// Returns the environment that will be used unless a custom base URL is
    /// set. Defaults to [`Environment::Production`].
    pub fn selected_environment(&self) -> Environment {
        self.environment.unwrap_or(Environment::Production)
    }

    /// Builds the client, validating configuration and credentials.
    pub fn build(self) -> Result<RevolutXClient> {
        let default_base_url = self.selected_environment().base_url().to_owned();
        let base_url = self.base_url.unwrap_or(default_base_url);
        if base_url.trim().is_empty() {
            return Err(Error::configuration("base URL must not be empty"));
        }

        let credentials = match (self.api_key, self.key) {
            (Some(api_key), Some(KeySource::Pem(pem))) => {
                Some(Credentials::from_pem(api_key, &pem)?)
            }
            (Some(api_key), Some(KeySource::Seed(seed))) => {
                Some(Credentials::from_seed(api_key, seed))
            }
            (None, None) => None,
            (Some(_), None) => {
                return Err(Error::configuration(
                    "an API key was provided but no private key; call private_key_pem or private_key_bytes",
                ));
            }
            (None, Some(_)) => {
                return Err(Error::configuration(
                    "a private key was provided but no API key; call api_key",
                ));
            }
        };

        let http_client = match self.http_client {
            Some(client) => client,
            None => reqwest::Client::builder()
                .timeout(self.timeout.unwrap_or(DEFAULT_TIMEOUT))
                .build()
                .map_err(|e| Error::configuration(format!("could not build HTTP client: {e}")))?,
        };

        let transport = Transport::new(&base_url, http_client, credentials)?;
        Ok(RevolutXClient { transport })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SEED: [u8; 32] = [7u8; 32];

    #[test]
    fn default_environment_is_production() {
        let client = RevolutXClient::builder().build().unwrap();
        assert_eq!(client.base_url(), Environment::Production.base_url());
        assert!(!client.is_authenticated());
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
        assert_eq!(client.base_url(), "http://127.0.0.1:8080/api/1.0");
    }

    #[test]
    fn credentials_make_client_authenticated() {
        let client = RevolutXClient::builder()
            .api_key("key")
            .private_key_bytes(TEST_SEED)
            .build()
            .unwrap();
        assert!(client.is_authenticated());
    }

    #[test]
    fn partial_credentials_are_rejected() {
        let err = RevolutXClient::builder()
            .api_key("key")
            .build()
            .unwrap_err();
        assert!(matches!(err, Error::Configuration { .. }));

        let err = RevolutXClient::builder()
            .private_key_bytes(TEST_SEED)
            .build()
            .unwrap_err();
        assert!(matches!(err, Error::Configuration { .. }));
    }

    #[test]
    fn empty_custom_base_url_is_rejected() {
        let err = RevolutXClient::builder()
            .base_url("  ")
            .build()
            .unwrap_err();
        assert!(matches!(err, Error::Configuration { .. }));
    }

    #[test]
    fn invalid_pem_surfaces_key_error() {
        let err = RevolutXClient::builder()
            .api_key("key")
            .private_key_pem("not a pem")
            .build()
            .unwrap_err();
        assert!(matches!(err, Error::Key { .. }));
    }
}
