//! Request execution and the pluggable transport seam.
//!
//! Endpoint modules build a [`RequestSpec`]; a [`RequestExecutor`] turns it into
//! a [`RawResponse`]. [`LocalExecutor`] (the default) owns base-URL/path joining,
//! deterministic query/body construction, signing (via a [`crate::Signer`]), and
//! sending over `reqwest`. A custom executor can forward the request elsewhere
//! (e.g. to a signing agent), which is how a thin client delegates all signing
//! and HTTP to another process.
//!
//! The query string and JSON body are built once and reused for both signing
//! and transmission, guaranteeing the signature covers exactly the bytes sent.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{CONTENT_TYPE, HeaderMap, RETRY_AFTER};
use reqwest::{Client as HttpClient, Method, Url};
use serde::Serialize;

use crate::auth::{API_KEY_HEADER, SIGNATURE_HEADER, Signer, TIMESTAMP_HEADER, signing_message};
use crate::error::{Error, Result};

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// A fully-described request, independent of how it is executed.
///
/// Built by the endpoint modules and consumed by a [`RequestExecutor`]. The
/// accessors let a custom executor inspect or forward the request.
pub struct RequestSpec {
    method: Method,
    /// Path relative to the configured base URL (which already includes
    /// `/api/1.0`), e.g. `/orders/active`. Path parameters are
    /// percent-encoded by the caller.
    path: String,
    query: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    requires_auth: bool,
}

impl RequestSpec {
    /// An authenticated `GET` request.
    pub(crate) fn get(path: impl Into<String>) -> Self {
        Self::new(Method::GET, path)
    }

    /// An authenticated `DELETE` request.
    pub(crate) fn delete(path: impl Into<String>) -> Self {
        Self::new(Method::DELETE, path)
    }

    /// An authenticated `POST` request with a JSON body.
    pub(crate) fn post_json<T: Serialize>(path: impl Into<String>, body: &T) -> Result<Self> {
        Self::new(Method::POST, path).with_json_body(body)
    }

    /// An authenticated `PUT` request with a JSON body.
    pub(crate) fn put_json<T: Serialize>(path: impl Into<String>, body: &T) -> Result<Self> {
        Self::new(Method::PUT, path).with_json_body(body)
    }

    fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            query: Vec::new(),
            body: None,
            requires_auth: true,
        }
    }

    /// Reassembles a spec from its raw parts. Used by the signing agent to
    /// reconstruct a request forwarded over the wire.
    #[cfg(feature = "agent")]
    pub(crate) const fn from_parts(
        method: Method,
        path: String,
        query: Vec<(String, String)>,
        body: Option<Vec<u8>>,
        requires_auth: bool,
    ) -> Self {
        Self {
            method,
            path,
            query,
            body,
            requires_auth,
        }
    }

    fn with_json_body<T: Serialize>(mut self, body: &T) -> Result<Self> {
        // `serde_json::to_vec` produces minified JSON: these exact bytes are
        // both signed and transmitted.
        self.body = Some(serde_json::to_vec(body).map_err(Error::Serialize)?);
        Ok(self)
    }

    /// Attaches ordered query parameters (already in their final string form).
    pub(crate) fn with_query(mut self, query: Vec<(String, String)>) -> Self {
        self.query = query;
        self
    }

    /// Marks the request as a public (unauthenticated) endpoint.
    pub(crate) const fn public(mut self) -> Self {
        self.requires_auth = false;
        self
    }

    /// The HTTP method.
    pub const fn method(&self) -> &Method {
        &self.method
    }

    /// The path relative to the base URL.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The ordered query parameters.
    pub fn query(&self) -> &[(String, String)] {
        &self.query
    }

    /// The request body, if any.
    pub fn body(&self) -> Option<&[u8]> {
        self.body.as_deref()
    }

    /// Whether the request must be authenticated.
    pub const fn requires_auth(&self) -> bool {
        self.requires_auth
    }
}

/// A raw response from a [`RequestExecutor`]: status, parsed `Retry-After`, and
/// body bytes. The client layer turns this into typed results or classified
/// errors.
#[derive(Debug, Clone)]
pub struct RawResponse {
    /// HTTP status code.
    pub status: u16,
    /// `Retry-After` delay, parsed from the header on rate-limit responses.
    pub retry_after: Option<Duration>,
    /// Raw response body.
    pub body: Vec<u8>,
}

/// A boxed, `Send` future — the return type of [`RequestExecutor::execute`].
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Executes a [`RequestSpec`] and returns a [`RawResponse`] — the pluggable
/// transport seam.
///
/// [`LocalExecutor`] signs and sends over HTTP; a remote/agent executor can
/// forward the request to another process. A [`crate::RevolutXClient`] is built
/// over an `Arc<dyn RequestExecutor>`.
pub trait RequestExecutor: Send + Sync {
    /// Executes the request.
    fn execute(&self, request: RequestSpec) -> BoxFuture<'_, Result<RawResponse>>;
    /// The base URL requests target.
    fn base_url(&self) -> &str;
    /// Whether this executor can authenticate (has a signer / credentials).
    fn is_authenticated(&self) -> bool;
}

/// The default executor: signs each request (via a [`Signer`]) and sends it over
/// HTTP with `reqwest`.
#[derive(Clone)]
pub struct LocalExecutor {
    http: HttpClient,
    base_url: Url,
    /// Path portion of the base URL with no trailing slash, e.g. `/api/1.0`.
    base_path: String,
    signer: Option<Arc<dyn Signer>>,
}

impl LocalExecutor {
    /// Builds an executor for the given base URL and optional signer. Without a
    /// signer, only public (unauthenticated) requests succeed.
    pub fn new(base_url: &str, http: HttpClient, signer: Option<Arc<dyn Signer>>) -> Result<Self> {
        let url = Url::parse(base_url)
            .map_err(|e| Error::configuration(format!("invalid base URL '{base_url}': {e}")))?;
        if url.cannot_be_a_base() {
            return Err(Error::configuration(format!(
                "base URL '{base_url}' cannot be a base URL"
            )));
        }
        let base_path = url.path().trim_end_matches('/').to_owned();
        Ok(Self {
            http,
            base_url: url,
            base_path,
            signer,
        })
    }

    async fn send(&self, spec: RequestSpec) -> Result<RawResponse> {
        let method_token = spec.method.as_str();
        let full_path = format!("{}{}", self.base_path, spec.path);
        let query = build_query(&spec.query);
        let body: &[u8] = spec.body.as_deref().unwrap_or(&[]);

        let mut url = self.base_url.clone();
        url.set_path(&full_path);
        url.set_query(if query.is_empty() { None } else { Some(&query) });

        // Our pre-encoding must survive `url` normalization unchanged, so the
        // bytes we signed are exactly the bytes on the wire.
        debug_assert_eq!(url.path(), full_path, "path changed during URL assembly");
        debug_assert_eq!(
            url.query().unwrap_or(""),
            query,
            "query changed during URL assembly"
        );

        let mut request = self.http.request(spec.method.clone(), url);

        if spec.requires_auth {
            let signer = self.signer.as_ref().ok_or(Error::MissingCredentials)?;
            let timestamp = now_unix_millis();
            let message = signing_message(timestamp, method_token, &full_path, &query, body);
            // One call → one decrypt for a keystore signer. `auth.api_key` is
            // `Zeroizing` and is wiped when this block ends, after reqwest has
            // copied it into the header.
            let auth = signer.authenticate(&message)?;
            request = request
                .header(API_KEY_HEADER, auth.api_key.as_str())
                .header(TIMESTAMP_HEADER, timestamp.to_string())
                .header(SIGNATURE_HEADER, auth.signature);
        }

        if let Some(body) = &spec.body {
            request = request
                .header(CONTENT_TYPE, "application/json")
                .body(body.clone());
        }

        let response = request.send().await.map_err(Error::Transport)?;
        let status = response.status().as_u16();
        let retry_after = parse_retry_after(response.headers());
        let bytes = response.bytes().await.map_err(Error::Transport)?;
        Ok(RawResponse {
            status,
            retry_after,
            body: bytes.to_vec(),
        })
    }
}

impl RequestExecutor for LocalExecutor {
    fn execute(&self, request: RequestSpec) -> BoxFuture<'_, Result<RawResponse>> {
        Box::pin(async move { self.send(request).await })
    }

    fn base_url(&self) -> &str {
        self.base_url.as_str()
    }

    fn is_authenticated(&self) -> bool {
        self.signer.is_some()
    }
}

impl std::fmt::Debug for LocalExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The HTTP client and signer are intentionally omitted (the signer can
        // hold credential material); `finish_non_exhaustive` records that.
        f.debug_struct("LocalExecutor")
            .field("base_url", &self.base_url.as_str())
            .field("authenticated", &self.signer.is_some())
            .finish_non_exhaustive()
    }
}

/// Percent-encodes a single path or query component, encoding every byte that
/// is not an RFC 3986 unreserved character. Reused for path parameters.
pub(crate) fn encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0x0f) as usize] as char);
            }
        }
    }
    out
}

/// Builds a `key=value&key=value` query string from ordered pairs. Repeated
/// keys (used for array parameters) are preserved in order.
fn build_query(params: &[(String, String)]) -> String {
    let mut out = String::new();
    for (key, value) in params {
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(&encode_component(key));
        out.push('=');
        out.push_str(&encode_component(value));
    }
    out
}

fn now_unix_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_millis)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn encodes_reserved_query_characters() {
        // base64 cursors contain +, /, = which must be percent-encoded.
        assert_eq!(encode_component("a+b/c=d"), "a%2Bb%2Fc%3Dd");
        assert_eq!(encode_component("BTC-USD"), "BTC-USD");
        assert_eq!(encode_component("BTC/USD"), "BTC%2FUSD");
    }

    #[test]
    fn builds_query_with_repeated_keys_in_order() {
        let q = build_query(&[
            ("symbols".into(), "BTC-USD".into()),
            ("symbols".into(), "ETH-USD".into()),
            ("limit".into(), "100".into()),
        ]);
        assert_eq!(q, "symbols=BTC-USD&symbols=ETH-USD&limit=100");
    }

    #[test]
    fn empty_query_is_blank() {
        assert_eq!(build_query(&[]), "");
    }

    #[test]
    fn base_path_strips_trailing_slash_and_reports_unauthenticated() {
        let executor =
            LocalExecutor::new("https://revx.revolut.com/api/1.0/", HttpClient::new(), None)
                .unwrap();
        assert_eq!(executor.base_path, "/api/1.0");
        assert!(!executor.is_authenticated());
        assert_eq!(executor.base_url(), "https://revx.revolut.com/api/1.0/");
    }

    #[test]
    fn rejects_invalid_base_url() {
        assert!(LocalExecutor::new("not a url", HttpClient::new(), None).is_err());
    }
}
