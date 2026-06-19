//! Internal HTTP transport.
//!
//! This layer owns base-URL/path joining, deterministic query and body
//! construction, request signing, sending via `reqwest`, and response
//! classification. Endpoint modules build a [`RequestSpec`] and call
//! [`Transport::send_json`] / [`Transport::send_no_content`]; they never touch
//! auth headers or HTTP details directly.
//!
//! The query string and JSON body are built once and reused for both signing
//! and transmission, guaranteeing the signature covers exactly the bytes sent.

use std::time::Duration;

use reqwest::header::{CONTENT_TYPE, HeaderMap, RETRY_AFTER};
use reqwest::{Client as HttpClient, Method, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::auth::{
    API_KEY_HEADER, Credentials, SIGNATURE_HEADER, TIMESTAMP_HEADER, signing_message,
};
use crate::error::{Error, Result, classify_error_response};

const HEX: &[u8; 16] = b"0123456789ABCDEF";
const MAX_BODY_PREVIEW: usize = 2048;

/// A fully-described request, independent of HTTP transport details.
pub(crate) struct RequestSpec {
    method: Method,
    /// Path relative to the configured base URL (which already includes
    /// `/api/1.0`), e.g. `/orders/active`. Path parameters must be
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
    pub(crate) fn public(mut self) -> Self {
        self.requires_auth = false;
        self
    }
}

/// Shared HTTP transport for the SDK.
#[derive(Debug, Clone)]
pub(crate) struct Transport {
    http: HttpClient,
    base_url: Url,
    /// Path portion of the base URL with no trailing slash, e.g. `/api/1.0`.
    base_path: String,
    credentials: Option<Credentials>,
}

impl Transport {
    /// Builds a transport for the given base URL.
    pub(crate) fn new(
        base_url: &str,
        http: HttpClient,
        credentials: Option<Credentials>,
    ) -> Result<Self> {
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
            credentials,
        })
    }

    /// Returns the configured base URL as a string.
    pub(crate) fn base_url(&self) -> &str {
        self.base_url.as_str()
    }

    /// Returns whether the transport was configured with API credentials.
    pub(crate) fn has_credentials(&self) -> bool {
        self.credentials.is_some()
    }

    /// Sends a request and deserializes a successful JSON response into `T`.
    pub(crate) async fn send_json<T: DeserializeOwned>(&self, spec: RequestSpec) -> Result<T> {
        let method = spec.method.as_str().to_owned();
        let full_path = format!("{}{}", self.base_path, spec.path);
        let response = self.send(&spec).await?;
        let status = response.status();
        let retry_after = parse_retry_after(response.headers());
        let bytes = response.bytes().await.map_err(Error::Transport)?;

        if status.is_success() {
            serde_json::from_slice::<T>(bytes.as_ref()).map_err(|source| Error::Deserialize {
                method,
                path: full_path,
                source,
                body: preview(bytes.as_ref()),
            })
        } else {
            Err(classify_error_response(
                status.as_u16(),
                retry_after,
                bytes.as_ref(),
            ))
        }
    }

    /// Sends a request that is expected to return no content (HTTP 204).
    pub(crate) async fn send_no_content(&self, spec: RequestSpec) -> Result<()> {
        let response = self.send(&spec).await?;
        let status = response.status();
        let retry_after = parse_retry_after(response.headers());
        if status.is_success() {
            return Ok(());
        }
        let bytes = response.bytes().await.map_err(Error::Transport)?;
        Err(classify_error_response(
            status.as_u16(),
            retry_after,
            bytes.as_ref(),
        ))
    }

    async fn send(&self, spec: &RequestSpec) -> Result<reqwest::Response> {
        let method_token = spec.method.as_str();
        let full_path = format!("{}{}", self.base_path, spec.path);
        let query = build_query(&spec.query);
        let body: &[u8] = spec.body.as_deref().unwrap_or(&[]);

        let mut url = self.base_url.clone();
        url.set_path(&full_path);
        url.set_query(if query.is_empty() { None } else { Some(&query) });

        // Our pre-encoding must survive `url` normalization unchanged, so that
        // the bytes we signed are exactly the bytes on the wire.
        debug_assert_eq!(url.path(), full_path, "path changed during URL assembly");
        debug_assert_eq!(
            url.query().unwrap_or(""),
            query,
            "query changed during URL assembly"
        );

        let mut request = self.http.request(spec.method.clone(), url);

        if spec.requires_auth {
            let credentials = self.credentials.as_ref().ok_or(Error::MissingCredentials)?;
            let timestamp = now_unix_millis();
            let message = signing_message(timestamp, method_token, &full_path, &query, body);
            let signature = credentials.sign(&message);
            request = request
                .header(API_KEY_HEADER, credentials.api_key())
                .header(TIMESTAMP_HEADER, timestamp.to_string())
                .header(SIGNATURE_HEADER, signature);
        }

        if let Some(body) = &spec.body {
            request = request
                .header(CONTENT_TYPE, "application/json")
                .body(body.clone());
        }

        request.send().await.map_err(Error::Transport)
    }
}

/// Percent-encodes a single path or query component, encoding every byte that
/// is not an RFC 3986 unreserved character. Reused for path parameters.
pub(crate) fn encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
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
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
    fn base_path_strips_trailing_slash() {
        let t =
            Transport::new("https://revx.revolut.com/api/1.0/", HttpClient::new(), None).unwrap();
        assert_eq!(t.base_path, "/api/1.0");
        assert!(!t.has_credentials());
    }

    #[test]
    fn rejects_invalid_base_url() {
        assert!(Transport::new("not a url", HttpClient::new(), None).is_err());
    }
}
