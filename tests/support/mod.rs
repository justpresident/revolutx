//! Minimal, dependency-free mock HTTP server for offline endpoint tests.
//!
//! A [`MockServer`] binds an ephemeral loopback port, serves exactly one
//! request with a canned response, and records the request it received so tests
//! can assert the method, path, query, headers (including the auth headers),
//! and body the SDK actually sent.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use ed25519_dalek::{Signature, SigningKey, Verifier};

/// A request captured by the [`MockServer`].
pub struct RecordedRequest {
    pub method: String,
    /// The raw request target, e.g. `/api/1.0/orders/active?limit=100`.
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RecordedRequest {
    /// The request path without the query string.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or(&self.target)
    }

    /// The query string (without the leading `?`), if present.
    pub fn query(&self) -> &str {
        self.target.split_once('?').map(|(_, q)| q).unwrap_or("")
    }

    /// A header value by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn has_header(&self, name: &str) -> bool {
        self.header(name).is_some()
    }

    pub fn body_string(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Reconstructs the canonical signing message and verifies the
    /// `X-Revx-Signature` header against the public key derived from `seed`.
    /// Panics if the headers are missing or the signature does not verify.
    pub fn assert_signed_with(&self, seed: [u8; 32]) {
        let api_key = self.header("X-Revx-API-Key").expect("api key header");
        assert!(!api_key.is_empty(), "api key header is empty");
        let timestamp = self.header("X-Revx-Timestamp").expect("timestamp header");
        assert!(
            timestamp.parse::<i64>().is_ok(),
            "timestamp header is not an integer: {timestamp}"
        );
        let signature_b64 = self.header("X-Revx-Signature").expect("signature header");

        // message = ts + UPPERCASE method + path + query (no '?') + body
        let mut message = Vec::new();
        message.extend_from_slice(timestamp.as_bytes());
        message.extend_from_slice(self.method.as_bytes());
        message.extend_from_slice(self.path().as_bytes());
        message.extend_from_slice(self.query().as_bytes());
        message.extend_from_slice(&self.body);

        let sig_bytes = base64_decode(signature_b64).expect("signature is valid base64");
        let sig_array: [u8; 64] = sig_bytes
            .as_slice()
            .try_into()
            .expect("signature is 64 bytes");
        let signature = Signature::from_bytes(&sig_array);
        let verifying_key = SigningKey::from_bytes(&seed).verifying_key();
        verifying_key
            .verify(&message, &signature)
            .expect("signature verifies against reconstructed message");
    }
}

/// A one-shot mock HTTP server.
pub struct MockServer {
    base_url: String,
    rx: Receiver<RecordedRequest>,
    handle: Option<thread::JoinHandle<()>>,
}

impl MockServer {
    /// Starts a server that replies once with `status` and a JSON `body`.
    pub fn start(status: u16, body: &str) -> Self {
        Self::start_with_headers(status, body, &[])
    }

    /// Like [`MockServer::start`] but adds extra response headers.
    pub fn start_with_headers(status: u16, body: &str, extra_headers: &[(&str, &str)]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let base_url = format!("http://{addr}/api/1.0");
        let (tx, rx) = mpsc::channel();
        let body = body.to_owned();
        let headers: Vec<(String, String)> = extra_headers
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();

        let handle = thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                handle_connection(stream, status, &body, &headers, &tx);
            }
        });

        MockServer {
            base_url,
            rx,
            handle: Some(handle),
        }
    }

    /// Starts a server that answers `responses.len()` consecutive requests, in
    /// order, one connection each (every response carries `Connection: close`,
    /// so the client reconnects between requests). Collect the recorded
    /// requests with [`MockServer::recorded_all`].
    pub fn start_sequence(responses: &[(u16, &str)]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let base_url = format!("http://{addr}/api/1.0");
        let (tx, rx) = mpsc::channel();
        let responses: Vec<(u16, String)> = responses
            .iter()
            .map(|(status, body)| (*status, (*body).to_owned()))
            .collect();

        let handle = thread::spawn(move || {
            for (status, body) in responses {
                match listener.accept() {
                    Ok((stream, _)) => handle_connection(stream, status, &body, &[], &tx),
                    Err(_) => return,
                }
            }
        });

        MockServer {
            base_url,
            rx,
            handle: Some(handle),
        }
    }

    /// The base URL to configure the client with (includes `/api/1.0`).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Blocks until the request has been served, then returns it.
    pub fn recorded(mut self) -> RecordedRequest {
        let request = self
            .rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a request was recorded");
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        request
    }

    /// Blocks until `n` requests have been served, then returns them in order.
    pub fn recorded_all(mut self, n: usize) -> Vec<RecordedRequest> {
        let requests = (0..n)
            .map(|i| {
                self.rx
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap_or_else(|_| panic!("request {} of {n} was recorded", i + 1))
            })
            .collect();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        requests
    }
}

fn handle_connection(
    mut stream: TcpStream,
    status: u16,
    body: &str,
    extra_headers: &[(String, String)],
    tx: &Sender<RecordedRequest>,
) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let header_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return,
        }
    };

    let header_text = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let target = parts.next().unwrap_or("").to_owned();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim();
            let v = v.trim();
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.parse().unwrap_or(0);
            }
            headers.push((k.to_owned(), v.to_owned()));
        }
    }

    let mut body_buf = buf[header_end + 4..].to_vec();
    while body_buf.len() < content_length {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body_buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    body_buf.truncate(content_length);

    let reason = reason_phrase(status);
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in extra_headers {
        response.push_str(&format!("{k}: {v}\r\n"));
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();

    let _ = tx.send(RecordedRequest {
        method,
        target,
        headers,
        body: body_buf,
    });
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Minimal standard-alphabet base64 decoder (no padding requirements beyond the
/// standard), sufficient for decoding signature headers in tests.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for &c in input.as_bytes() {
        if c == b'=' {
            break;
        }
        let v = val(c)? as u32;
        buffer = (buffer << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}
