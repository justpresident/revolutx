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
//! of bincode. Requests are [`AgentRequest`] (`Ping`, `Capabilities`, or
//! `Execute`), responses are [`AgentResponse`] (`Pong`, `Capabilities`,
//! `Executed`, or `Failed`). On connect, a client first asks for
//! [`Capabilities`] — the agent reports its base URL and whether order mutations
//! are allowed, and **enforces** that policy on every `Execute` (a non-`GET`
//! request is refused unless trading is enabled).
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

/// Largest request frame the agent will read (64 KiB). A request is a minified
/// JSON order body plus a path/query — kilobytes at most. This is generous
/// headroom that also bounds the agent's exposure to a hostile length prefix.
const MAX_REQUEST_FRAME: u32 = 64 * 1024;

/// Largest response frame a client will read (1 MiB). REST responses are
/// paginated (order books, candle history, ticker lists, history pages) and stay
/// well under this; it is a defensive ceiling, not an expected size.
const MAX_RESPONSE_FRAME: u32 = 1024 * 1024;

/// Once a frame's length prefix has been read, the rest of the frame must arrive
/// within this window. A stalled partial frame must not pin a connection (and
/// the agent's unlocked vault) open. The length prefix itself is *not* timed: a
/// healthy idle client legitimately sends nothing between requests.
const FRAME_BODY_TIMEOUT: Duration = Duration::from_secs(10);

/// A request sent from a client to the agent.
#[derive(Debug, Encode, Decode)]
pub enum AgentRequest {
    /// Liveness check.
    Ping,
    /// Ask the agent what it allows (target base URL, whether trading is on).
    Capabilities,
    /// Execute a forwarded request (the agent signs and sends it).
    Execute(WireRequest),
}

/// A response sent from the agent back to a client.
#[derive(Debug, Encode, Decode)]
pub enum AgentResponse {
    /// Reply to [`AgentRequest::Ping`].
    Pong,
    /// Reply to [`AgentRequest::Capabilities`].
    Capabilities(Capabilities),
    /// A completed request's raw response.
    Executed(WireResponse),
    /// The agent could not execute the request, or refused it (the message is
    /// human-readable and intentionally coarse — it must not leak credential
    /// material).
    Failed(String),
}

/// What an agent is configured to allow. Reported during the connection
/// handshake so a client (e.g. the MCP) reflects the agent's policy without
/// owning any of it.
#[derive(Debug, Clone, Encode, Decode)]
pub struct Capabilities {
    /// The base URL the agent targets (it owns the real environment; clients use
    /// this only for display).
    pub base_url: String,
    /// Whether the agent will execute order-mutating (non-`GET`) requests. The
    /// agent enforces this regardless of what any client believes.
    pub trading_enabled: bool,
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

/// The default socket path: `$XDG_RUNTIME_DIR/revolutx-agent.sock`.
///
/// When `XDG_RUNTIME_DIR` is unset, falls back to a `revolutx-agent`
/// subdirectory of the system temp dir — the daemon creates that subdirectory
/// `0700`, so the socket is private even when the temp dir itself is shared.
#[must_use]
pub fn default_socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(
        || {
            std::env::temp_dir()
                .join("revolutx-agent")
                .join("agent.sock")
        },
        |dir| PathBuf::from(dir).join("revolutx-agent.sock"),
    )
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

async fn read_frame_bytes(reader: &mut UnixStream, max_len: u32) -> std::io::Result<Vec<u8>> {
    // Untimed: between requests an idle client legitimately sends nothing.
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > max_len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame exceeds the maximum size",
        ));
    }
    let mut buf = vec![0u8; len as usize];
    // Timed: a started frame whose body stalls must not hold the connection open.
    tokio::time::timeout(FRAME_BODY_TIMEOUT, reader.read_exact(&mut buf))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "frame body timed out"))??;
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
/// It holds a **single persistent connection** for its lifetime — the agent
/// accepts exactly one client and refuses the rest — and serializes requests
/// over it, so concurrent [`execute`](RequestExecutor::execute) calls are safe
/// but run one at a time.
#[derive(Debug)]
pub struct AgentExecutor {
    base_url: String,
    trading_enabled: bool,
    conn: tokio::sync::Mutex<UnixStream>,
}

impl AgentExecutor {
    /// Connects to the agent at `socket_path` and performs the capabilities
    /// handshake. The agent reports its own base URL and trading policy, so the
    /// client needs no environment configuration of its own.
    pub async fn connect(socket_path: impl AsRef<Path>) -> Result<Self> {
        let socket_path = socket_path.as_ref();
        let mut stream = UnixStream::connect(socket_path).await.map_err(|e| {
            Error::agent(format!(
                "cannot connect to agent at {}: {e}",
                socket_path.display()
            ))
        })?;

        // Handshake: learn the agent's base URL and trading policy up front.
        write_frame(&mut stream, &AgentRequest::Capabilities)
            .await
            .map_err(|e| Error::agent(format!("capabilities handshake failed: {e}")))?;
        let bytes = read_frame_bytes(&mut stream, MAX_RESPONSE_FRAME)
            .await
            .map_err(|e| Error::agent(format!("capabilities handshake failed: {e}")))?;
        let AgentResponse::Capabilities(caps) = decode::<AgentResponse>(&bytes)
            .map_err(|e| Error::agent(format!("invalid capabilities response: {e}")))?
        else {
            return Err(Error::agent("agent did not answer with capabilities"));
        };

        Ok(Self {
            base_url: caps.base_url,
            trading_enabled: caps.trading_enabled,
            conn: tokio::sync::Mutex::new(stream),
        })
    }

    /// Whether the agent will execute order-mutating requests. Determined by the
    /// agent at startup and read during the connection handshake.
    #[must_use]
    pub const fn trading_enabled(&self) -> bool {
        self.trading_enabled
    }

    /// Checks that the agent is responding (sent over the established
    /// connection).
    pub async fn ping(&self) -> Result<()> {
        match self.round_trip(&AgentRequest::Ping).await? {
            AgentResponse::Pong => Ok(()),
            _ => Err(Error::agent("agent did not answer ping with Pong")),
        }
    }

    async fn round_trip(&self, request: &AgentRequest) -> Result<AgentResponse> {
        // Hold the connection lock only for the write/read critical section, so a
        // request fully completes before the next one starts on the shared
        // stream; decoding happens after the guard is released.
        let bytes = {
            let mut conn = self.conn.lock().await;
            write_frame(&mut conn, request)
                .await
                .map_err(|e| Error::agent(format!("failed to send request to agent: {e}")))?;
            read_frame_bytes(&mut conn, MAX_RESPONSE_FRAME)
                .await
                .map_err(|e| Error::agent(format!("failed to read agent response: {e}")))?
        };
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
                AgentResponse::Pong | AgentResponse::Capabilities(_) => {
                    Err(Error::agent("unexpected agent response to an Execute"))
                }
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

/// Serves the agent protocol on a unix socket at `socket_path` for **exactly one
/// client**, then returns.
///
/// The first connection is accepted and served (every `Execute` is dispatched to
/// `executor`, a credential-holding [`crate::transport::LocalExecutor`]). Any
/// further connection attempt is accepted and immediately closed — the agent is
/// a single "trade as me" oracle, so concurrent clients and reconnects are
/// refused. When the one client disconnects, this returns and the daemon exits,
/// re-locking the vault. `on_connect` fires once, when that client connects — the
/// daemon uses it to cancel its pre-connection idle timeout.
///
/// `trading_enabled` is the authoritative order-mutation policy: when `false`,
/// the agent refuses every non-`GET` request regardless of what the client
/// believes. It is reported to the client during the capabilities handshake.
///
/// The socket is created with `0600` permissions. If a live agent is already
/// listening on `socket_path` this returns an error; a stale socket file (no
/// listener) is removed and replaced.
pub async fn serve(
    executor: Arc<dyn RequestExecutor>,
    socket_path: &Path,
    trading_enabled: bool,
    on_connect: impl FnOnce() + Send,
) -> Result<()> {
    let listener = bind(socket_path).await?;

    let (mut stream, _addr) = listener
        .accept()
        .await
        .map_err(|e| Error::agent(format!("agent accept failed: {e}")))?;
    on_connect();

    // Keep the listener alive (so a second daemon's liveness probe still sees us)
    // but refuse every further connection by closing it immediately.
    let rejector = tokio::spawn(async move {
        while let Ok((extra, _addr)) = listener.accept().await {
            drop(extra);
        }
    });

    handle_connection(executor.as_ref(), &mut stream, trading_enabled).await;
    rejector.abort();
    Ok(())
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

    // Keep the socket's parent directory private (0700). This protects the brief
    // window between `bind` (which creates the socket honoring the ambient umask,
    // possibly group/other-readable) and the chmod below: a 0700 parent means no
    // other user can reach the socket during that window. `$XDG_RUNTIME_DIR` is
    // already 0700; this also covers the temp-dir fallback's private subdir.
    ensure_private_parent(socket_path)?;
    let listener = UnixListener::bind(socket_path)
        .map_err(|e| Error::agent(format!("could not bind {}: {e}", socket_path.display())))?;
    set_socket_permissions(socket_path)?;
    Ok(listener)
}

#[cfg(unix)]
fn ensure_private_parent(socket_path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let Some(parent) = socket_path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    if !parent.exists() {
        // We are creating it, so make it private from the start (mode honors the
        // umask, hence the explicit set_permissions belt below).
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|e| Error::agent(format!("could not create {}: {e}", parent.display())))?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| Error::agent(format!("could not secure {}: {e}", parent.display())))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_parent(_socket_path: &Path) -> Result<()> {
    Ok(())
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
    executor: &dyn RequestExecutor,
    stream: &mut UnixStream,
    trading_enabled: bool,
) {
    loop {
        // A closed connection or a malformed frame just ends this session.
        let Ok(bytes) = read_frame_bytes(stream, MAX_REQUEST_FRAME).await else {
            return;
        };
        let request: AgentRequest = match decode(&bytes) {
            Ok(request) => request,
            Err(_) => return,
        };

        let response = match request {
            AgentRequest::Ping => AgentResponse::Pong,
            AgentRequest::Capabilities => AgentResponse::Capabilities(Capabilities {
                base_url: executor.base_url().to_owned(),
                trading_enabled,
            }),
            AgentRequest::Execute(wire) => execute_forwarded(executor, wire, trading_enabled).await,
        };

        if write_frame(stream, &response).await.is_err() {
            return;
        }
    }
}

async fn execute_forwarded(
    executor: &dyn RequestExecutor,
    wire: WireRequest,
    trading_enabled: bool,
) -> AgentResponse {
    let Ok(method) = Method::from_bytes(wire.method.as_bytes()) else {
        return AgentResponse::Failed(format!("invalid HTTP method '{}'", wire.method));
    };
    // Authoritative gate: any state-changing (non-GET) request is an order
    // mutation, refused unless the agent was started with trading enabled. The
    // agent — not the client — is the trust boundary for this.
    if !trading_enabled && method != Method::GET {
        return AgentResponse::Failed(
            "trading is disabled on this agent; restart it with --enable-trading to allow orders"
                .to_owned(),
        );
    }
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

    fn spawn_echo(socket: PathBuf, trading: bool) -> tokio::task::JoinHandle<Result<()>> {
        tokio::spawn(async move { serve(Arc::new(EchoExecutor), &socket, trading, || {}).await })
    }

    #[tokio::test]
    async fn round_trips_and_reports_capabilities() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let server = spawn_echo(socket.clone(), false);
        wait_for(&socket).await;

        // The handshake learns the agent's base URL and trading policy.
        let executor = AgentExecutor::connect(&socket).await.unwrap();
        assert_eq!(executor.base_url(), "http://stub/api/1.0");
        assert!(!executor.trading_enabled());

        executor.ping().await.unwrap();
        // A single persistent connection carries many requests.
        let raw = executor
            .execute(RequestSpec::get("/balances"))
            .await
            .unwrap();
        assert_eq!(raw.status, 200);
        assert_eq!(raw.body, b"/balances");
        let raw = executor
            .execute(RequestSpec::get("/orders/active"))
            .await
            .unwrap();
        assert_eq!(raw.body, b"/orders/active");

        server.abort();
    }

    #[tokio::test]
    async fn agent_refuses_mutations_when_trading_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let server = spawn_echo(socket.clone(), false);
        wait_for(&socket).await;

        let executor = AgentExecutor::connect(&socket).await.unwrap();
        assert!(!executor.trading_enabled());

        // Reads are always allowed.
        executor
            .execute(RequestSpec::get("/orders/active"))
            .await
            .unwrap();
        // A non-GET (order mutation) is refused by the agent itself.
        let err = executor
            .execute(RequestSpec::delete("/orders/abc"))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Agent { .. }));

        server.abort();
    }

    #[tokio::test]
    async fn agent_allows_mutations_when_trading_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let server = spawn_echo(socket.clone(), true);
        wait_for(&socket).await;

        let executor = AgentExecutor::connect(&socket).await.unwrap();
        assert!(executor.trading_enabled());
        let raw = executor
            .execute(RequestSpec::delete("/orders/abc"))
            .await
            .unwrap();
        assert_eq!(raw.body, b"/orders/abc");

        server.abort();
    }

    #[tokio::test]
    async fn on_connect_fires_once_when_the_client_connects() {
        use std::sync::atomic::{AtomicU64, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");

        let connects = Arc::new(AtomicU64::new(0));
        let on_connect = {
            let connects = Arc::clone(&connects);
            move || {
                connects.fetch_add(1, Ordering::Relaxed);
            }
        };
        let server = {
            let socket = socket.clone();
            tokio::spawn(
                async move { serve(Arc::new(EchoExecutor), &socket, false, on_connect).await },
            )
        };
        wait_for(&socket).await;
        assert_eq!(connects.load(Ordering::Relaxed), 0, "not yet connected");

        // connect() does the capabilities handshake, then two pings — still one
        // accepted connection, so `on_connect` fires exactly once.
        let executor = AgentExecutor::connect(&socket).await.unwrap();
        executor.ping().await.unwrap();
        executor.ping().await.unwrap();
        assert_eq!(connects.load(Ordering::Relaxed), 1);

        server.abort();
    }

    #[tokio::test]
    async fn refuses_a_second_client_connection() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let server = spawn_echo(socket.clone(), false);
        wait_for(&socket).await;

        // First client owns the connection.
        let first = AgentExecutor::connect(&socket).await.unwrap();
        first.ping().await.unwrap();

        // A second client is accepted at the socket layer but closed immediately
        // by the rejector, so even its handshake fails.
        assert!(AgentExecutor::connect(&socket).await.is_err());

        // The first client is unaffected.
        first.ping().await.unwrap();

        server.abort();
    }
}
