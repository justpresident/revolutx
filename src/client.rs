//! Client configuration and endpoint entry points.
//!
//! [`RevolutXClient`] is the top-level handle. It is built with
//! [`RevolutXClient::builder`], owns a shared [`RequestExecutor`], and exposes
//! the endpoint groups: [`balances`](RevolutXClient::balances),
//! [`configuration`](RevolutXClient::configuration),
//! [`market_data`](RevolutXClient::market_data),
//! [`orders`](RevolutXClient::orders), and [`trades`](RevolutXClient::trades).
//!
//! A client may be built without credentials to access only the public market
//! data endpoints; calling an authenticated endpoint in that case returns
//! [`crate::Error::MissingCredentials`].
//!
//! Two seams make the client pluggable:
//! [`ClientBuilder::signer`] swaps *how* requests are signed (e.g. an encrypted
//! keystore), and [`ClientBuilder::executor`] swaps *where* they execute (e.g. a
//! signing agent in another process).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::api::balances::BalancesApi;
use crate::api::configuration::ConfigurationApi;
use crate::api::market_data::MarketDataApi;
use crate::api::orders::OrdersApi;
use crate::api::trades::TradesApi;
use crate::auth::{Ed25519Signer, Signer};
use crate::error::{Error, Result, classify_error_response};
use crate::transport::{LocalExecutor, RequestExecutor, RequestSpec};

/// Default request timeout when none is configured.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY_PREVIEW: usize = 2048;

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
/// Cloning is cheap: the underlying executor (HTTP pool + credentials, or an
/// agent connection) is shared behind an `Arc`.
#[derive(Clone)]
pub struct RevolutXClient {
    executor: Arc<dyn RequestExecutor>,
}

impl RevolutXClient {
    /// Starts configuring a new client.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Builds a client over a custom [`RequestExecutor`].
    pub fn with_executor(executor: Arc<dyn RequestExecutor>) -> Self {
        Self { executor }
    }

    /// Returns the configured base URL.
    pub fn base_url(&self) -> &str {
        self.executor.base_url()
    }

    /// Returns whether the client can authenticate requests.
    pub fn is_authenticated(&self) -> bool {
        self.executor.is_authenticated()
    }

    /// Account balance endpoints.
    pub const fn balances(&self) -> BalancesApi<'_> {
        BalancesApi::new(self)
    }

    /// Exchange configuration endpoints (currencies and pairs).
    pub const fn configuration(&self) -> ConfigurationApi<'_> {
        ConfigurationApi::new(self)
    }

    /// Market data endpoints (order books, candles, tickers, public trades).
    pub const fn market_data(&self) -> MarketDataApi<'_> {
        MarketDataApi::new(self)
    }

    /// Order placement and management endpoints.
    pub const fn orders(&self) -> OrdersApi<'_> {
        OrdersApi::new(self)
    }

    /// Trade history endpoints.
    pub const fn trades(&self) -> TradesApi<'_> {
        TradesApi::new(self)
    }

    /// Executes a request and deserializes a successful JSON response into `T`.
    pub(crate) async fn send_json<T: DeserializeOwned>(&self, spec: RequestSpec) -> Result<T> {
        let method = spec.method().as_str().to_owned();
        let path = spec.path().to_owned();
        let response = self.executor.execute(spec).await?;

        if (200..300).contains(&response.status) {
            serde_json::from_slice::<T>(&response.body).map_err(|source| Error::Deserialize {
                method,
                path,
                source,
                body: preview(&response.body),
            })
        } else {
            Err(classify_error_response(
                response.status,
                response.retry_after,
                &response.body,
            ))
        }
    }

    /// Executes a request expected to return no content (HTTP 204).
    pub(crate) async fn send_no_content(&self, spec: RequestSpec) -> Result<()> {
        let response = self.executor.execute(spec).await?;
        if (200..300).contains(&response.status) {
            Ok(())
        } else {
            Err(classify_error_response(
                response.status,
                response.retry_after,
                &response.body,
            ))
        }
    }
}

impl fmt::Debug for RevolutXClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevolutXClient")
            .field("base_url", &self.executor.base_url())
            .field("authenticated", &self.executor.is_authenticated())
            .finish()
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
    signer: Option<Arc<dyn Signer>>,
    executor: Option<Arc<dyn RequestExecutor>>,
    environment: Option<Environment>,
    base_url: Option<String>,
    timeout: Option<Duration>,
    http_client: Option<reqwest::Client>,
}

impl ClientBuilder {
    /// Sets the API key (the `X-Revx-API-Key` value).
    #[must_use]
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Loads the Ed25519 private key from a PKCS#8 PEM string, as produced by
    /// `openssl genpkey -algorithm ed25519 -out private.pem`.
    #[must_use]
    pub fn private_key_pem(mut self, pem: impl Into<String>) -> Self {
        self.key = Some(KeySource::Pem(pem.into()));
        self
    }

    /// Sets the Ed25519 private key from its raw 32-byte seed (for advanced
    /// users and tests).
    #[must_use]
    pub fn private_key_bytes(mut self, seed: [u8; 32]) -> Self {
        self.key = Some(KeySource::Seed(seed));
        self
    }

    /// Supplies a custom [`Signer`] (e.g. an encrypted keystore or hardware
    /// token), overriding `api_key` / `private_key_*`. Combined with the
    /// configured base URL and HTTP client.
    #[must_use]
    pub fn signer(mut self, signer: Arc<dyn Signer>) -> Self {
        self.signer = Some(signer);
        self
    }

    /// Supplies a fully custom [`RequestExecutor`] (e.g. an agent-backed
    /// transport). When set, the base URL, timeout, HTTP client, and signer are
    /// ignored — the executor is self-contained.
    #[must_use]
    pub fn executor(mut self, executor: Arc<dyn RequestExecutor>) -> Self {
        self.executor = Some(executor);
        self
    }

    /// Selects a Revolut X environment. Ignored if a custom base URL is set.
    #[must_use]
    pub const fn environment(mut self, environment: Environment) -> Self {
        self.environment = Some(environment);
        self
    }

    /// Overrides the base URL (primarily for tests and advanced deployments).
    /// Must include the `/api/1.0` path prefix used by the signature.
    #[must_use]
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Sets the request timeout. Ignored if a custom HTTP client is supplied.
    #[must_use]
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Supplies a preconfigured [`reqwest::Client`], taking full control of
    /// transport settings (TLS, proxies, timeouts, connection pooling).
    #[must_use]
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
        let Self {
            api_key,
            key,
            signer,
            executor,
            environment,
            base_url,
            timeout,
            http_client,
        } = self;

        // A fully custom executor is self-contained.
        if let Some(executor) = executor {
            return Ok(RevolutXClient { executor });
        }

        let environment = environment.unwrap_or(Environment::Production);
        let base_url = base_url.unwrap_or_else(|| environment.base_url().to_owned());
        if base_url.trim().is_empty() {
            return Err(Error::configuration("base URL must not be empty"));
        }

        let signer: Option<Arc<dyn Signer>> = match signer {
            Some(signer) => Some(signer),
            None => match (api_key, key) {
                (Some(api_key), Some(KeySource::Pem(pem))) => {
                    Some(Arc::new(Ed25519Signer::from_pem(api_key, &pem)?))
                }
                (Some(api_key), Some(KeySource::Seed(seed))) => {
                    Some(Arc::new(Ed25519Signer::from_seed(api_key, seed)))
                }
                (None, None) => None,
                (Some(_), None) => {
                    return Err(Error::configuration(
                        "an API key was provided but no private key; call private_key_pem, private_key_bytes, or signer",
                    ));
                }
                (None, Some(_)) => {
                    return Err(Error::configuration(
                        "a private key was provided but no API key; call api_key",
                    ));
                }
            },
        };

        let http_client = match http_client {
            Some(client) => client,
            None => reqwest::Client::builder()
                .timeout(timeout.unwrap_or(DEFAULT_TIMEOUT))
                .build()
                .map_err(|e| Error::configuration(format!("could not build HTTP client: {e}")))?,
        };

        let executor = LocalExecutor::new(&base_url, http_client, signer)?;
        Ok(RevolutXClient {
            executor: Arc::new(executor),
        })
    }
}

fn preview(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= MAX_BODY_PREVIEW {
        return text.into_owned();
    }
    let mut end = MAX_BODY_PREVIEW;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… ({} bytes total)", &text[..end], bytes.len())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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
    fn custom_signer_makes_client_authenticated() {
        let signer = Arc::new(Ed25519Signer::from_seed("key", TEST_SEED));
        let client = RevolutXClient::builder().signer(signer).build().unwrap();
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
