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
//! of bincode. Requests are [`AgentRequest`] (`Authenticate`, `Ping`, or
//! `Execute`), responses are [`AgentResponse`] (`Authenticated`, `Pong`,
//! `Executed`, or `Failed`).
//!
//! # Peer authentication
//!
//! The socket's `0600` permissions only prove a connecting peer shares the
//! agent's UID — they do **not** stop a *different* same-UID process from
//! connecting to the signing oracle and trading as you. To close that gap, the
//! agent is started with a one-time [`AuthToken`] (generated and printed to the
//! operator's terminal). A freshly accepted connection is **unauthenticated**:
//! the only request it can issue is [`AgentRequest::Authenticate`], and every
//! other request is refused until it presents the token. The token is compared
//! in constant time and **consumed on first valid use**, so exactly one
//! connection can ever authenticate — the single "trade as me" oracle. An
//! unauthenticated peer learns nothing about the agent (not even its base URL or
//! trading policy): the [`Capabilities`] are revealed only inside the
//! [`AgentResponse::Authenticated`] reply.
//!
//! # Transport security
//!
//! The socket is created with `0600` permissions inside `$XDG_RUNTIME_DIR`
//! (itself `0700`, user-private). There is no network transport: a signing agent
//! reachable over TCP would be a "trade as me" oracle, so that is deliberately
//! out of scope.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bincode::{Decode, Encode};
use reqwest::Method;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;
use tokio::task::JoinSet;
use zeroize::Zeroizing;

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

/// Wrong-token attempts tolerated on a single connection before it is dropped. A
/// legitimate client presents the right token on the first try; this only bounds
/// chatter, since the token is high-entropy and single-use (brute force is
/// infeasible regardless).
const MAX_AUTH_ATTEMPTS: u32 = 5;

/// Random bytes in a handshake token: 256 bits of entropy from the OS CSPRNG.
const TOKEN_BYTES: usize = 32;

/// Refusal sent when a connection presents a wrong or already-spent token.
const MSG_AUTH_FAILED: &str = "authentication failed: wrong or already-used token";
/// Refusal sent when an unauthenticated connection issues a non-`Authenticate`
/// request.
const MSG_AUTH_REQUIRED: &str = "authenticate first: present the agent's one-time token";
/// Refusal sent when an already-authenticated connection re-authenticates.
const MSG_ALREADY_AUTHENTICATED: &str = "already authenticated";

/// A one-time, high-entropy token that authenticates the connecting peer before
/// the signing oracle is exposed.
///
/// The agent generates one at startup, prints it **once** to the operator's
/// terminal, and the authenticating client presents it in the
/// [`AgentRequest::Authenticate`] handshake. It is never accepted as a
/// command-line argument value (that would expose it via `/proc/<pid>/cmdline`
/// and `ps` to the very same-UID attacker this defends against): the operator
/// copies the printed value out of band. The token is compared in constant time
/// and consumed on first valid use.
pub struct AuthToken(Zeroizing<String>);

impl AuthToken {
    /// Generates a fresh token from operating-system randomness, URL-safe-base64
    /// encoded so it is easy to copy and paste.
    pub fn generate() -> Result<Self> {
        // The raw bytes are wiped as soon as they are encoded; only the printable
        // form (itself zeroizing) is retained.
        let mut bytes = Zeroizing::new([0u8; TOKEN_BYTES]);
        getrandom::fill(bytes.as_mut_slice())
            .map_err(|e| Error::agent(format!("could not read OS randomness for token: {e}")))?;
        Ok(Self(Zeroizing::new(
            URL_SAFE_NO_PAD.encode(bytes.as_slice()),
        )))
    }

    /// The printable token string — the value to hand to the authenticating
    /// client.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Constant-time comparison against a candidate, so a wrong guess cannot be
    /// refined by timing. Defense in depth atop the token's entropy and
    /// single-use nature.
    fn verify(&self, candidate: &str) -> bool {
        self.0.as_bytes().ct_eq(candidate.as_bytes()).into()
    }
}

impl std::fmt::Debug for AuthToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the secret, even in diagnostics.
        f.debug_tuple("AuthToken").field(&"<redacted>").finish()
    }
}

/// A request sent from a client to the agent.
#[derive(Encode, Decode)]
pub enum AgentRequest {
    /// Present the one-time handshake token. The only request an unauthenticated
    /// connection may issue; on success the agent replies with
    /// [`AgentResponse::Authenticated`].
    Authenticate(String),
    /// Liveness check (only after authentication).
    Ping,
    /// Execute a forwarded request — the agent signs and sends it (only after
    /// authentication).
    Execute(WireRequest),
}

impl std::fmt::Debug for AgentRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redact the candidate token so it cannot leak through a Debug render.
        match self {
            Self::Authenticate(_) => f.write_str("Authenticate(<redacted>)"),
            Self::Ping => f.write_str("Ping"),
            Self::Execute(wire) => f.debug_tuple("Execute").field(wire).finish(),
        }
    }
}

/// A response sent from the agent back to a client.
#[derive(Debug, Encode, Decode)]
pub enum AgentResponse {
    /// Authentication succeeded — carries the [`Capabilities`] (revealed only
    /// now, never to an unauthenticated peer).
    Authenticated(Capabilities),
    /// Reply to [`AgentRequest::Ping`].
    Pong,
    /// A completed request's raw response.
    Executed(WireResponse),
    /// The agent could not execute the request, or refused it (e.g. the
    /// connection has not authenticated, or the token was wrong/already used).
    /// The message is human-readable and intentionally coarse — it must not leak
    /// credential material.
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
/// It holds a **single persistent connection** for its lifetime and serializes
/// requests over it, so concurrent [`execute`](RequestExecutor::execute) calls
/// are safe but run one at a time. The connection starts **unauthenticated**:
/// call [`authenticate`](Self::authenticate) with the agent's one-time token
/// before issuing any other request, or the agent refuses them.
#[derive(Debug)]
pub struct AgentExecutor {
    conn: tokio::sync::Mutex<UnixStream>,
    /// The agent's base URL and trading policy — learned only on successful
    /// authentication, so unset until then.
    caps: OnceLock<Capabilities>,
    authenticated: AtomicBool,
}

impl AgentExecutor {
    /// Opens the connection to the agent at `socket_path`. No request is sent
    /// yet: the connection is unauthenticated until
    /// [`authenticate`](Self::authenticate) succeeds, and the agent reveals
    /// nothing (not even its base URL) before then.
    pub async fn connect(socket_path: impl AsRef<Path>) -> Result<Self> {
        let socket_path = socket_path.as_ref();
        let stream = UnixStream::connect(socket_path).await.map_err(|e| {
            Error::agent(format!(
                "cannot connect to agent at {}: {e}",
                socket_path.display()
            ))
        })?;
        Ok(Self {
            conn: tokio::sync::Mutex::new(stream),
            caps: OnceLock::new(),
            authenticated: AtomicBool::new(false),
        })
    }

    /// Presents the agent's one-time token. On success the agent reveals its
    /// [`Capabilities`] (base URL + trading policy), which are cached for
    /// [`base_url`](RequestExecutor::base_url) and
    /// [`trading_enabled`](Self::trading_enabled), and the connection becomes
    /// authenticated. The token is single-use: a second connection presenting it
    /// is refused. A wrong token returns an error but leaves the connection open
    /// to retry.
    pub async fn authenticate(&self, token: &str) -> Result<()> {
        match self
            .round_trip(&AgentRequest::Authenticate(token.to_owned()))
            .await?
        {
            AgentResponse::Authenticated(caps) => {
                // Set-once; a redundant authenticate cannot clobber the policy.
                let _ = self.caps.set(caps);
                self.authenticated.store(true, Ordering::Release);
                Ok(())
            }
            AgentResponse::Failed(message) => Err(Error::agent(message)),
            AgentResponse::Pong | AgentResponse::Executed(_) => Err(Error::agent(
                "agent did not answer the authentication handshake",
            )),
        }
    }

    /// Whether this connection has authenticated.
    #[must_use]
    pub fn is_session_authenticated(&self) -> bool {
        self.authenticated.load(Ordering::Acquire)
    }

    /// Whether the agent will execute order-mutating requests. Reported by the
    /// agent on authentication; `false` until then.
    #[must_use]
    pub fn trading_enabled(&self) -> bool {
        self.caps.get().is_some_and(|caps| caps.trading_enabled)
    }

    /// Checks that the agent is responding (sent over the established
    /// connection). Only meaningful after authentication.
    pub async fn ping(&self) -> Result<()> {
        match self.round_trip(&AgentRequest::Ping).await? {
            AgentResponse::Pong => Ok(()),
            AgentResponse::Failed(message) => Err(Error::agent(message)),
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
                AgentResponse::Pong | AgentResponse::Authenticated(_) => {
                    Err(Error::agent("unexpected agent response to an Execute"))
                }
            }
        })
    }

    fn base_url(&self) -> &str {
        // Known only after authentication; empty until then (the agent path uses
        // this for display only — the agent itself joins paths to the real URL).
        self.caps.get().map_or("", |caps| caps.base_url.as_str())
    }

    fn is_authenticated(&self) -> bool {
        // The agent holds the credentials, so once this connection has
        // authenticated it can sign on our behalf.
        self.is_session_authenticated()
    }
}

// --- server side -----------------------------------------------------------

/// Serves the agent protocol on a unix socket at `socket_path` until the
/// authenticated client disconnects, then returns.
///
/// Connections are accepted **concurrently** and each starts unauthenticated: it
/// may only issue [`AgentRequest::Authenticate`], and every other request is
/// refused. The first connection to present the matching `token` consumes it
/// (atomically, constant-time compared) and becomes the single authenticated
/// client — the "trade as me" oracle. `on_connect` fires once, at that moment
/// (**not** on TCP accept), so the daemon's pre-connection idle auto-lock is
/// cancelled only by a genuinely authenticated peer, never by an attacker merely
/// connecting. When that client disconnects, this returns and the daemon exits,
/// re-locking the vault; the token is already spent, so no later connection can
/// authenticate.
///
/// `trading_enabled` is the authoritative order-mutation policy: when `false`,
/// the agent refuses every non-`GET` request regardless of what the client
/// believes. It is reported to the client inside the authentication reply.
///
/// The socket is created with `0600` permissions. If a live agent is already
/// listening on `socket_path` this returns an error; a stale socket file (no
/// listener) is removed and replaced.
pub async fn serve(
    executor: Arc<dyn RequestExecutor>,
    socket_path: &Path,
    trading_enabled: bool,
    token: AuthToken,
    on_connect: impl FnOnce() + Send,
) -> Result<()> {
    let listener = bind(socket_path).await?;

    let gate = Arc::new(tokio::sync::Mutex::new(Gate {
        token,
        consumed: false,
    }));
    // Fired once, by the connection that authenticates.
    let authenticated = Arc::new(Notify::new());
    // Fired when the authenticated client's session ends (it disconnected).
    let session_ended = Arc::new(Notify::new());

    let accept = tokio::spawn(accept_loop(
        listener,
        gate,
        executor,
        trading_enabled,
        Arc::clone(&authenticated),
        Arc::clone(&session_ended),
    ));

    // Block until someone authenticates, then cancel the idle auto-lock; then
    // block until that client disconnects. Unauthenticated peers coming and going
    // never end the daemon — the watchdog's idle timeout covers "nobody ever
    // authenticated".
    authenticated.notified().await;
    on_connect();
    session_ended.notified().await;

    accept.abort();
    Ok(())
}

/// The one-time authentication gate shared by every connection: the token is
/// consumed by the first connection to prove it.
struct Gate {
    token: AuthToken,
    consumed: bool,
}

/// Atomically checks a candidate against the unspent token. Returns `true` and
/// marks the token spent on the first valid presentation; `false` afterwards, or
/// for a wrong candidate (which does **not** spend it).
async fn try_consume(gate: &tokio::sync::Mutex<Gate>, candidate: &str) -> bool {
    let mut gate = gate.lock().await;
    if gate.consumed {
        return false;
    }
    if gate.token.verify(candidate) {
        gate.consumed = true;
        true
    } else {
        false
    }
}

/// Accepts connections forever, serving each in its own task. The [`JoinSet`] is
/// dropped — aborting any still-open connections — when this future is dropped,
/// which `serve` does once the authenticated session ends.
async fn accept_loop(
    listener: UnixListener,
    gate: Arc<tokio::sync::Mutex<Gate>>,
    executor: Arc<dyn RequestExecutor>,
    trading_enabled: bool,
    authenticated: Arc<Notify>,
    session_ended: Arc<Notify>,
) {
    let mut sessions = JoinSet::new();
    loop {
        // Reap finished connections so the set does not grow with churn.
        while sessions.try_join_next().is_some() {}
        let Ok((stream, _addr)) = listener.accept().await else {
            return;
        };
        let gate = Arc::clone(&gate);
        let executor = Arc::clone(&executor);
        let authenticated = Arc::clone(&authenticated);
        let session_ended = Arc::clone(&session_ended);
        sessions.spawn(async move {
            handle_connection(
                stream,
                &gate,
                executor.as_ref(),
                trading_enabled,
                &authenticated,
                &session_ended,
            )
            .await;
        });
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

/// Serves one connection. It is unauthenticated until it presents the token;
/// before then only [`AgentRequest::Authenticate`] is honored (every other
/// request is refused without revealing anything about the agent). After
/// authentication it serves `Ping`/`Execute` until the client disconnects, then
/// signals `session_ended` so the daemon can re-lock and exit.
async fn handle_connection(
    mut stream: UnixStream,
    gate: &tokio::sync::Mutex<Gate>,
    executor: &dyn RequestExecutor,
    trading_enabled: bool,
    authenticated: &Notify,
    session_ended: &Notify,
) {
    let mut is_authenticated = false;
    let mut attempts: u32 = 0;
    loop {
        // A closed connection or a malformed frame just ends this session.
        let Ok(bytes) = read_frame_bytes(&mut stream, MAX_REQUEST_FRAME).await else {
            break;
        };
        let request: AgentRequest = match decode(&bytes) {
            Ok(request) => request,
            Err(_) => break,
        };

        if !is_authenticated {
            match request {
                AgentRequest::Authenticate(candidate) => {
                    // Wipe the candidate after comparison — it is the token.
                    let candidate = Zeroizing::new(candidate);
                    if try_consume(gate, candidate.as_str()).await {
                        is_authenticated = true;
                        authenticated.notify_one();
                        let caps = Capabilities {
                            base_url: executor.base_url().to_owned(),
                            trading_enabled,
                        };
                        if write_frame(&mut stream, &AgentResponse::Authenticated(caps))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    } else {
                        attempts += 1;
                        let refused = AgentResponse::Failed(MSG_AUTH_FAILED.to_owned());
                        // Keep the connection open to retry, up to the attempt cap.
                        if write_frame(&mut stream, &refused).await.is_err()
                            || attempts >= MAX_AUTH_ATTEMPTS
                        {
                            break;
                        }
                    }
                }
                AgentRequest::Ping | AgentRequest::Execute(_) => {
                    // Refuse, but keep the connection open so a client that issued
                    // a request early can still authenticate and retry on it.
                    let refused = AgentResponse::Failed(MSG_AUTH_REQUIRED.to_owned());
                    if write_frame(&mut stream, &refused).await.is_err() {
                        break;
                    }
                }
            }
            continue;
        }

        let response = match request {
            AgentRequest::Ping => AgentResponse::Pong,
            AgentRequest::Execute(wire) => execute_forwarded(executor, wire, trading_enabled).await,
            // Re-authenticating an already-authenticated connection is a no-op.
            AgentRequest::Authenticate(_) => {
                AgentResponse::Failed(MSG_ALREADY_AUTHENTICATED.to_owned())
            }
        };

        if write_frame(&mut stream, &response).await.is_err() {
            break;
        }
    }

    if is_authenticated {
        // The one authenticated client has gone: end the daemon (re-lock vault).
        session_ended.notify_one();
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

    /// Spawns an echo agent and returns its one-time token alongside the handle.
    fn spawn_echo(socket: PathBuf, trading: bool) -> (String, tokio::task::JoinHandle<Result<()>>) {
        let token = AuthToken::generate().unwrap();
        let secret = token.as_str().to_owned();
        let handle = tokio::spawn(async move {
            serve(Arc::new(EchoExecutor), &socket, trading, token, || {}).await
        });
        (secret, handle)
    }

    #[test]
    fn token_generate_is_unique_and_verifies_in_constant_time() {
        let a = AuthToken::generate().unwrap();
        let b = AuthToken::generate().unwrap();
        assert_ne!(a.as_str(), b.as_str(), "tokens must be unique");
        assert!(!a.as_str().is_empty());
        assert!(a.verify(a.as_str()));
        assert!(!a.verify(b.as_str()));
        assert!(!a.verify(""));
        // Debug never leaks the secret.
        assert!(!format!("{a:?}").contains(a.as_str()));
    }

    #[tokio::test]
    async fn authenticated_round_trips_and_reports_capabilities() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (token, server) = spawn_echo(socket.clone(), false);
        wait_for(&socket).await;

        let executor = AgentExecutor::connect(&socket).await.unwrap();
        // Before authentication the agent reveals nothing and refuses requests.
        assert!(!executor.is_session_authenticated());
        assert_eq!(executor.base_url(), "");
        let err = executor
            .execute(RequestSpec::get("/balances"))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Agent { .. }));

        // Authenticating reveals the agent's base URL and trading policy.
        executor.authenticate(&token).await.unwrap();
        assert!(executor.is_session_authenticated());
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
    async fn wrong_token_is_refused_then_correct_token_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (token, server) = spawn_echo(socket.clone(), false);
        wait_for(&socket).await;

        let executor = AgentExecutor::connect(&socket).await.unwrap();
        // A wrong guess is rejected and does NOT spend the token; the connection
        // stays open so the right token still authenticates it.
        assert!(executor.authenticate("not-the-token").await.is_err());
        assert!(!executor.is_session_authenticated());
        executor.authenticate(&token).await.unwrap();
        assert!(executor.is_session_authenticated());

        server.abort();
    }

    #[tokio::test]
    async fn token_is_single_use_across_connections() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (token, server) = spawn_echo(socket.clone(), false);
        wait_for(&socket).await;

        // First client consumes the token and owns the oracle.
        let first = AgentExecutor::connect(&socket).await.unwrap();
        first.authenticate(&token).await.unwrap();
        first.ping().await.unwrap();

        // A second client connects but the token is already spent.
        let second = AgentExecutor::connect(&socket).await.unwrap();
        assert!(second.authenticate(&token).await.is_err());
        assert!(!second.is_session_authenticated());

        // The first client is unaffected.
        first.ping().await.unwrap();

        server.abort();
    }

    #[tokio::test]
    async fn agent_refuses_mutations_when_trading_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (token, server) = spawn_echo(socket.clone(), false);
        wait_for(&socket).await;

        let executor = AgentExecutor::connect(&socket).await.unwrap();
        executor.authenticate(&token).await.unwrap();
        assert!(!executor.trading_enabled());

        // Reads are allowed.
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
        let (token, server) = spawn_echo(socket.clone(), true);
        wait_for(&socket).await;

        let executor = AgentExecutor::connect(&socket).await.unwrap();
        executor.authenticate(&token).await.unwrap();
        assert!(executor.trading_enabled());
        let raw = executor
            .execute(RequestSpec::delete("/orders/abc"))
            .await
            .unwrap();
        assert_eq!(raw.body, b"/orders/abc");

        server.abort();
    }

    #[tokio::test]
    async fn on_connect_fires_once_on_authentication_not_on_accept() {
        use std::sync::atomic::{AtomicU64, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");

        let token = AuthToken::generate().unwrap();
        let secret = token.as_str().to_owned();
        let connects = Arc::new(AtomicU64::new(0));
        let on_connect = {
            let connects = Arc::clone(&connects);
            move || {
                connects.fetch_add(1, Ordering::Relaxed);
            }
        };
        let server = {
            let socket = socket.clone();
            tokio::spawn(async move {
                serve(Arc::new(EchoExecutor), &socket, false, token, on_connect).await
            })
        };
        wait_for(&socket).await;

        // Merely connecting must NOT fire on_connect.
        let executor = AgentExecutor::connect(&socket).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(connects.load(Ordering::Relaxed), 0, "not authenticated yet");

        // Authenticating fires it exactly once, regardless of later requests.
        executor.authenticate(&secret).await.unwrap();
        executor.ping().await.unwrap();
        executor.ping().await.unwrap();
        assert_eq!(connects.load(Ordering::Relaxed), 1);

        server.abort();
    }

    #[tokio::test]
    async fn serve_returns_when_the_authenticated_client_disconnects() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (token, server) = spawn_echo(socket.clone(), false);
        wait_for(&socket).await;

        let executor = AgentExecutor::connect(&socket).await.unwrap();
        executor.authenticate(&token).await.unwrap();
        executor.ping().await.unwrap();
        // Dropping the authenticated client ends its session, so `serve` returns.
        drop(executor);
        let outcome = tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("serve should return after the authenticated client leaves");
        assert!(outcome.unwrap().is_ok());
    }
}
