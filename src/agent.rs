//! Signing-agent proxy (`agent` feature, unix-only).
//!
//! A long-running **agent** owns the keystore and performs all signing and HTTP.
//! A client-side [`AgentExecutor`] forwards [`RequestSpec`]s to it over a unix
//! socket and receives only response bytes. This is a *full proxy*: neither the
//! private key **nor** the API key ever crosses the socket. The motivating use
//! is a headless server (e.g. an MCP) that has no TTY to prompt for the vault
//! password — it delegates to an agent that was unlocked interactively once.
//!
//! - The daemon side is [`serve`], driven by the `revolutx agent start` CLI
//!   subcommand: it unlocks the vault, builds a normal client, and forwards each
//!   request to that client's executor.
//! - The client side is [`AgentExecutor`], an [`Arc<dyn RequestExecutor>`] you
//!   plug into [`crate::ClientBuilder::executor`].
//!
//! # Wire protocol
//!
//! Each message is a `u32` big-endian length prefix followed by that many bytes
//! of bincode. Requests are [`AgentRequest`] (`Ping` or `Execute`), responses
//! are [`AgentResponse`] (`Pong`, `Executed`, or `Failed`).
//!
//! # Transport security
//!
//! The socket is created with `0600` permissions inside `$XDG_RUNTIME_DIR`
//! (itself `0700`, user-private). There is no network transport: a signing agent
//! reachable over TCP would be a "trade as me" oracle, so that is deliberately
//! out of scope.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bincode::{Decode, Encode};
use reqwest::Method;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use crate::error::{Error, Result};
use crate::transport::{BoxFuture, RawResponse, RequestExecutor, RequestSpec};

/// Largest accepted frame (16 MiB) — a sanity bound against a malformed or
/// hostile length prefix.
const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

/// A request sent from a client to the agent.
#[derive(Debug, Encode, Decode)]
pub enum AgentRequest {
    /// Liveness check.
    Ping,
    /// Execute a forwarded request (the agent signs and sends it).
    Execute(WireRequest),
}

/// A response sent from the agent back to a client.
#[derive(Debug, Encode, Decode)]
pub enum AgentResponse {
    /// Reply to [`AgentRequest::Ping`].
    Pong,
    /// A completed request's raw response.
    Executed(WireResponse),
    /// The agent could not execute the request (the message is human-readable
    /// and intentionally coarse — it must not leak credential material).
    Failed(String),
}

/// The wire form of a [`RequestSpec`]: method as a token, plus the path, query,
/// optional body, and the auth flag.
#[derive(Debug, Encode, Decode)]
pub struct WireRequest {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    requires_auth: bool,
}

/// The wire form of a [`RawResponse`].
#[derive(Debug, Encode, Decode)]
pub struct WireResponse {
    status: u16,
    retry_after_millis: Option<u64>,
    body: Vec<u8>,
}

/// The default socket path: `$XDG_RUNTIME_DIR/revolutx-agent.sock`, falling back
/// to the system temp directory when `XDG_RUNTIME_DIR` is unset.
#[must_use]
pub fn default_socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("revolutx-agent.sock")
}

// --- framing ---------------------------------------------------------------

async fn write_frame<T: Encode + Sync>(stream: &mut UnixStream, value: &T) -> std::io::Result<()> {
    let bytes = bincode::encode_to_vec(value, bincode::config::standard())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let len = u32::try_from(bytes.len()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large to encode")
    })?;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

async fn read_frame_bytes(reader: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame exceeds the maximum size",
        ));
    }
    let mut buf = vec![0u8; len as usize];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

fn decode<T: Decode<()>>(bytes: &[u8]) -> std::io::Result<T> {
    bincode::decode_from_slice(bytes, bincode::config::standard())
        .map(|(value, _)| value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

// --- client side -----------------------------------------------------------

/// A [`RequestExecutor`] that forwards every request to a running agent.
///
/// A fresh unix-socket connection is opened per request, so it is cheap to clone
/// and safe to use concurrently.
#[derive(Debug, Clone)]
pub struct AgentExecutor {
    socket_path: PathBuf,
    base_url: String,
}

impl AgentExecutor {
    /// Targets the agent at `socket_path`. `base_url` is informational only (the
    /// agent owns the real base URL); pass the environment's base URL so
    /// [`crate::RevolutXClient::base_url`] reports something meaningful.
    pub fn new(socket_path: impl Into<PathBuf>, base_url: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
            base_url: base_url.into(),
        }
    }

    /// Checks that an agent is responding on the socket.
    pub async fn ping(&self) -> Result<()> {
        match self.round_trip(&AgentRequest::Ping).await? {
            AgentResponse::Pong => Ok(()),
            _ => Err(Error::agent("agent did not answer ping with Pong")),
        }
    }

    async fn round_trip(&self, request: &AgentRequest) -> Result<AgentResponse> {
        let mut stream = UnixStream::connect(&self.socket_path).await.map_err(|e| {
            Error::agent(format!(
                "cannot connect to agent at {}: {e}",
                self.socket_path.display()
            ))
        })?;
        write_frame(&mut stream, request)
            .await
            .map_err(|e| Error::agent(format!("failed to send request to agent: {e}")))?;
        let bytes = read_frame_bytes(&mut stream)
            .await
            .map_err(|e| Error::agent(format!("failed to read agent response: {e}")))?;
        decode(&bytes).map_err(|e| Error::agent(format!("invalid agent response: {e}")))
    }
}

impl RequestExecutor for AgentExecutor {
    fn execute(&self, request: RequestSpec) -> BoxFuture<'_, Result<RawResponse>> {
        let wire = WireRequest {
            method: request.method().as_str().to_owned(),
            path: request.path().to_owned(),
            query: request.query().to_vec(),
            body: request.body().map(<[u8]>::to_vec),
            requires_auth: request.requires_auth(),
        };
        Box::pin(async move {
            match self.round_trip(&AgentRequest::Execute(wire)).await? {
                AgentResponse::Executed(w) => Ok(RawResponse {
                    status: w.status,
                    retry_after: w.retry_after_millis.map(Duration::from_millis),
                    body: w.body,
                }),
                AgentResponse::Failed(message) => Err(Error::agent(message)),
                AgentResponse::Pong => Err(Error::agent("agent returned Pong to an Execute")),
            }
        })
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn is_authenticated(&self) -> bool {
        // The agent holds the credentials; from the client's view it is always
        // capable of signing.
        true
    }
}

// --- server side -----------------------------------------------------------

/// Serves the agent protocol on a unix socket at `socket_path`.
///
/// Each `Execute` is dispatched to `executor` (which should be a
/// credential-holding [`crate::transport::LocalExecutor`]). `on_request` is
/// invoked once per received request — the daemon uses it to track activity for
/// its idle auto-lock.
///
/// The socket is created with `0600` permissions. If a live agent is already
/// listening on `socket_path` this returns an error; a stale socket file (no
/// listener) is removed and replaced. Runs until the listener errors.
pub async fn serve(
    executor: Arc<dyn RequestExecutor>,
    socket_path: &Path,
    on_request: Arc<dyn Fn() + Send + Sync>,
) -> Result<()> {
    let listener = bind(socket_path).await?;
    loop {
        let (stream, _addr) = listener
            .accept()
            .await
            .map_err(|e| Error::agent(format!("agent accept failed: {e}")))?;
        let executor = Arc::clone(&executor);
        let on_request = Arc::clone(&on_request);
        tokio::spawn(handle_connection(executor, stream, on_request));
    }
}

/// Binds the socket, refusing to clobber a live agent and cleaning up a stale
/// socket file, then tightens permissions to `0600`.
async fn bind(socket_path: &Path) -> Result<UnixListener> {
    if socket_path.exists() {
        // If something is actually listening, another agent owns this socket.
        if UnixStream::connect(socket_path).await.is_ok() {
            return Err(Error::agent(format!(
                "an agent is already listening on {}",
                socket_path.display()
            )));
        }
        // Stale socket from a previous run: remove it before rebinding.
        std::fs::remove_file(socket_path)
            .map_err(|e| Error::agent(format!("could not remove stale socket: {e}")))?;
    }

    let listener = UnixListener::bind(socket_path)
        .map_err(|e| Error::agent(format!("could not bind {}: {e}", socket_path.display())))?;
    set_socket_permissions(socket_path)?;
    Ok(listener)
}

#[cfg(unix)]
fn set_socket_permissions(socket_path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| Error::agent(format!("could not set socket permissions: {e}")))
}

#[cfg(not(unix))]
fn set_socket_permissions(_socket_path: &Path) -> Result<()> {
    Ok(())
}

async fn handle_connection(
    executor: Arc<dyn RequestExecutor>,
    mut stream: UnixStream,
    on_request: Arc<dyn Fn() + Send + Sync>,
) {
    loop {
        // A closed connection or a malformed frame just ends this session.
        let Ok(bytes) = read_frame_bytes(&mut stream).await else {
            return;
        };
        let request: AgentRequest = match decode(&bytes) {
            Ok(request) => request,
            Err(_) => return,
        };
        on_request();

        let response = match request {
            AgentRequest::Ping => AgentResponse::Pong,
            AgentRequest::Execute(wire) => execute_forwarded(executor.as_ref(), wire).await,
        };

        if write_frame(&mut stream, &response).await.is_err() {
            return;
        }
    }
}

async fn execute_forwarded(executor: &dyn RequestExecutor, wire: WireRequest) -> AgentResponse {
    let Ok(method) = Method::from_bytes(wire.method.as_bytes()) else {
        return AgentResponse::Failed(format!("invalid HTTP method '{}'", wire.method));
    };
    let spec =
        RequestSpec::from_parts(method, wire.path, wire.query, wire.body, wire.requires_auth);
    match executor.execute(spec).await {
        Ok(raw) => AgentResponse::Executed(WireResponse {
            status: raw.status,
            retry_after_millis: raw
                .retry_after
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
            body: raw.body,
        }),
        Err(e) => AgentResponse::Failed(e.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// A stub executor that echoes the request path back as the response body,
    /// so a round-trip proves the request crossed the socket intact.
    #[derive(Debug)]
    struct EchoExecutor;

    impl RequestExecutor for EchoExecutor {
        fn execute(&self, request: RequestSpec) -> BoxFuture<'_, Result<RawResponse>> {
            let body = request.path().as_bytes().to_vec();
            Box::pin(async move {
                Ok(RawResponse {
                    status: 200,
                    retry_after: None,
                    body,
                })
            })
        }
        #[allow(clippy::unnecessary_literal_bound)]
        fn base_url(&self) -> &str {
            "http://stub/api/1.0"
        }
        fn is_authenticated(&self) -> bool {
            true
        }
    }

    async fn wait_for(path: &Path) {
        for _ in 0..200 {
            if path.exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("socket never appeared");
    }

    #[tokio::test]
    async fn executor_round_trips_through_serve() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");

        let noop: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let server = {
            let socket = socket.clone();
            tokio::spawn(async move { serve(Arc::new(EchoExecutor), &socket, noop).await })
        };
        wait_for(&socket).await;

        let executor = AgentExecutor::new(&socket, "http://stub/api/1.0");
        executor.ping().await.unwrap();

        let spec = RequestSpec::get("/balances");
        let raw = executor.execute(spec).await.unwrap();
        assert_eq!(raw.status, 200);
        assert_eq!(raw.body, b"/balances");

        server.abort();
    }

    #[tokio::test]
    async fn refuses_a_second_agent_on_the_same_socket() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");

        let noop: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let server = {
            let socket = socket.clone();
            let noop = Arc::clone(&noop);
            tokio::spawn(async move { serve(Arc::new(EchoExecutor), &socket, noop).await })
        };
        wait_for(&socket).await;

        let err = bind(&socket).await.unwrap_err();
        assert!(matches!(err, Error::Agent { .. }));

        server.abort();
    }

    #[tokio::test]
    async fn activity_callback_fires_per_request() {
        use std::sync::atomic::{AtomicU64, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");

        let count = Arc::new(AtomicU64::new(0));
        let on_request: Arc<dyn Fn() + Send + Sync> = {
            let count = Arc::clone(&count);
            Arc::new(move || {
                count.fetch_add(1, Ordering::Relaxed);
            })
        };
        let server = {
            let socket = socket.clone();
            tokio::spawn(async move { serve(Arc::new(EchoExecutor), &socket, on_request).await })
        };
        wait_for(&socket).await;

        let executor = AgentExecutor::new(&socket, "http://stub/api/1.0");
        executor.ping().await.unwrap();
        executor
            .execute(RequestSpec::get("/orders/active"))
            .await
            .unwrap();

        // Give the spawned connection task a moment to record the activity.
        for _ in 0..200 {
            if count.load(Ordering::Relaxed) >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(count.load(Ordering::Relaxed) >= 2);

        server.abort();
    }
}
