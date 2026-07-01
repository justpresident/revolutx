//! Signing-agent proxy (`agent` feature, unix-only).
//!
//! A long-running **agent** owns the keystore and performs all signing and HTTP.
//! Client-side [`AgentExecutor`]s forward [`RequestSpec`]s to it over a unix socket
//! and receive only response bytes. This is a *full proxy*: neither the private key
//! **nor** the API key ever crosses the socket. The motivating use is a headless
//! client (an MCP, or the market-data collector) with no TTY to prompt for the vault
//! password — it delegates to an agent that was unlocked interactively once.
//!
//! The agent serves **many** connections at once. [`AgentServer`] runs the accept
//! loop; [`AgentControl`] is the operator's handle to it (list connections, grant,
//! deny, shut down) — what the `revolutx agent start` REPL drives.
//!
//! # Wire protocol
//!
//! Each message is a `u32` big-endian length prefix followed by that many bytes of
//! bincode. Requests are [`AgentRequest`] (`Authenticate`, `RequestAuth`, `Ping`,
//! `Execute`); responses are [`AgentResponse`] (`Authenticated`, `AuthPending`,
//! `AuthDenied`, `Pong`, `Executed`, `Failed`).
//!
//! # Authorizing a connection
//!
//! A freshly accepted connection is **unauthorized**: it may only ask to authorize,
//! and every other request is refused until it does. There are two ways:
//!
//! - **Token** — the client presents the one-time [`AuthToken`] the operator was
//!   given at startup (`--auth-token`). It is compared in constant time and consumed
//!   on first use, and grants the agent's ceiling [`AccessLevel`]. One token, one
//!   client (e.g. the MCP).
//! - **Manual** — the client sends [`AgentRequest::RequestAuth`] and becomes
//!   *pending*; the operator sees its peer credentials (uid/gid/pid) and label and
//!   grants it (at an [`AccessLevel`] of their choice, up to the ceiling) or denies
//!   it. Any number of clients can be authorized this way; the client polls until the
//!   operator decides.
//!
//! Access is enforced **per connection** by [`required_access_for`] on every
//! forwarded request — the agent, not the client, is the trust boundary.
//!
//! # Transport security
//!
//! The socket is created world-connectable (`0666`) so a peer with a different UID
//! (e.g. a container) can reach it; nothing is served without a valid token or an
//! explicit operator grant, and the operator sees each peer's uid/gid/pid before
//! deciding. Placing the socket in a private directory (e.g. `$XDG_RUNTIME_DIR`,
//! `0700`) additionally restricts it to the agent's own UID. There is no network
//! transport: a signing agent reachable over TCP would be a "trade as me" oracle.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

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

use crate::access::{AccessLevel, access_denied, required_access_for};
use crate::error::{Error, Result};
use crate::transport::{BoxFuture, RawResponse, RequestExecutor, RequestSpec};

/// Largest request frame the agent will read (64 KiB). A request is a minified
/// JSON order body plus a path/query — kilobytes at most. This is generous
/// headroom that also bounds the agent's exposure to a hostile length prefix.
const MAX_REQUEST_FRAME: u32 = 64 * 1024;

/// Largest response frame a client will read (8 MiB). REST market-data responses
/// (deep candle windows, ticker lists across every pair, order books, history pages)
/// can be several MiB; this is a generous ceiling, not an expected size. The agent
/// refuses a body that would exceed it (see [`MAX_RESPONSE_BODY`]) *before* sending,
/// so a too-large response never overflows the frame and desynchronizes the stream.
const MAX_RESPONSE_FRAME: u32 = 8 * 1024 * 1024;

/// Largest forwarded response body the agent will return. A larger one is refused
/// with a `Failed` reply rather than an oversized frame, keeping the connection
/// framed and reusable. Leaves headroom under [`MAX_RESPONSE_FRAME`] for the envelope.
const MAX_RESPONSE_BODY: usize = MAX_RESPONSE_FRAME as usize - 4096;

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
/// Refusal sent when an unauthorized connection issues a request before it has
/// authorized (by token or operator grant).
const MSG_AUTH_REQUIRED: &str =
    "authorize first: present the agent's token or request operator approval";
/// Refusal sent for a token handshake when the agent was started without a token.
const MSG_NO_TOKEN: &str =
    "this agent has no token; request operator approval (RequestAuth) instead";
/// Told to a connection the operator has denied.
const MSG_DENIED: &str = "the operator denied this connection";

/// A one-time, high-entropy token that authenticates the connecting peer before
/// the signing oracle is exposed.
///
/// The agent generates one at startup, prints it **once** to the operator's
/// terminal, and the authenticating client presents it in the
/// [`AgentRequest::Authenticate`] handshake. It is never accepted as a
/// command-line argument value (that would expose it via `/proc/<pid>/cmdline`
/// and `ps` to a same-UID attacker): the operator copies the printed value out of
/// band. The token is compared in constant time and consumed on first valid use.
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
    /// Present the one-time handshake token (grants the agent's ceiling access).
    Authenticate(String),
    /// Ask the operator to authorize this connection at `requested` access (capped
    /// at the ceiling), tagged with an optional `label`. Idempotent — resend it to
    /// poll: the reply is [`AgentResponse::AuthPending`] until the operator decides,
    /// then [`AgentResponse::Authenticated`] or [`AgentResponse::AuthDenied`].
    RequestAuth {
        /// A human-readable tag the operator sees (e.g. `revolutx-collector`).
        label: Option<String>,
        /// The access tier the client is asking for (capped at the ceiling).
        requested: AccessLevel,
    },
    /// Liveness check (only after authorization).
    Ping,
    /// Execute a forwarded request — the agent signs and sends it (only after
    /// authorization).
    Execute(WireRequest),
}

impl std::fmt::Debug for AgentRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redact the candidate token so it cannot leak through a Debug render.
        match self {
            Self::Authenticate(_) => f.write_str("Authenticate(<redacted>)"),
            Self::RequestAuth { label, requested } => f
                .debug_struct("RequestAuth")
                .field("label", label)
                .field("requested", requested)
                .finish(),
            Self::Ping => f.write_str("Ping"),
            Self::Execute(wire) => f.debug_tuple("Execute").field(wire).finish(),
        }
    }
}

/// A response sent from the agent back to a client.
#[derive(Debug, Encode, Decode)]
pub enum AgentResponse {
    /// Authorization succeeded — carries the [`Capabilities`] (base URL + the access
    /// granted to *this* connection), revealed only now.
    Authenticated(Capabilities),
    /// A manual authorization request is registered and awaiting the operator; the
    /// client should poll (resend [`AgentRequest::RequestAuth`]) shortly.
    AuthPending,
    /// The operator denied this connection. Human-readable.
    AuthDenied(String),
    /// Reply to [`AgentRequest::Ping`].
    Pong,
    /// A completed request's raw response.
    Executed(WireResponse),
    /// The agent could not execute the request, or refused it (not yet authorized,
    /// or a wrong/spent token). Coarse by design — it must not leak credentials.
    Failed(String),
}

/// What a connection is allowed. Reported in the authorization reply so a client
/// (e.g. the MCP) reflects the policy without owning any of it.
#[derive(Debug, Clone, Encode, Decode)]
pub struct Capabilities {
    /// The base URL the agent targets (it owns the real environment; clients use
    /// this only for display).
    pub base_url: String,
    /// The [`AccessLevel`] granted to this connection. The agent enforces it
    /// regardless of what the client believes; it is reported only so a client can
    /// reflect the policy.
    pub access: AccessLevel,
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
/// subdirectory of the system temp dir.
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

/// The result of an [`AgentExecutor::request_authorization`] poll.
#[derive(Debug)]
pub enum AuthOutcome {
    /// The operator granted this connection; carries the granted capabilities.
    Authorized(Capabilities),
    /// Still awaiting the operator's decision — poll again shortly.
    Pending,
    /// The operator denied this connection; carries a human-readable reason.
    Denied(String),
}

/// A [`RequestExecutor`] that forwards every request to a running agent.
///
/// It holds a **single persistent connection** for its lifetime and serializes
/// requests over it, so concurrent [`execute`](RequestExecutor::execute) calls
/// are safe but run one at a time. The connection starts **unauthorized**: call
/// [`authenticate`](Self::authenticate) (token) or
/// [`request_authorization`](Self::request_authorization) (manual) before issuing
/// any other request, or the agent refuses them.
#[derive(Debug)]
pub struct AgentExecutor {
    conn: tokio::sync::Mutex<UnixStream>,
    /// The agent's base URL and this connection's access — learned only on
    /// successful authorization, so unset until then.
    caps: OnceLock<Capabilities>,
    authenticated: AtomicBool,
    /// Set once a write/read transport error leaves the stream in an unknown state.
    /// A desynchronized stream can never be trusted again, so further requests fail
    /// fast (with a clear "reconnect" error) instead of reading misframed garbage.
    broken: AtomicBool,
}

impl AgentExecutor {
    /// Opens the connection to the agent at `socket_path`. No request is sent
    /// yet: the connection is unauthorized until [`authenticate`](Self::authenticate)
    /// or [`request_authorization`](Self::request_authorization) succeeds, and the
    /// agent reveals nothing (not even its base URL) before then.
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
            broken: AtomicBool::new(false),
        })
    }

    /// Presents the agent's one-time token. On success the agent reveals this
    /// connection's [`Capabilities`] (base URL + access), which are cached for
    /// [`base_url`](RequestExecutor::base_url) and [`access`](Self::access). The
    /// token is single-use: a second connection presenting it is refused. A wrong
    /// token returns an error but leaves the connection open to retry.
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
            AgentResponse::Failed(message) | AgentResponse::AuthDenied(message) => {
                Err(Error::agent(message))
            }
            AgentResponse::AuthPending | AgentResponse::Pong | AgentResponse::Executed(_) => Err(
                Error::agent("agent did not answer the authentication handshake"),
            ),
        }
    }

    /// Requests interactive operator authorization for this connection at
    /// `requested` access, tagged with `label` for the operator's display. This is
    /// the alternative to [`authenticate`](Self::authenticate) for clients with no
    /// token. Idempotent: call it repeatedly (e.g. once a second) to poll until the
    /// operator decides. On [`AuthOutcome::Authorized`] the granted [`Capabilities`]
    /// are cached for [`access`](Self::access) and [`base_url`](RequestExecutor::base_url).
    pub async fn request_authorization(
        &self,
        label: Option<&str>,
        requested: AccessLevel,
    ) -> Result<AuthOutcome> {
        let request = AgentRequest::RequestAuth {
            label: label.map(str::to_owned),
            requested,
        };
        match self.round_trip(&request).await? {
            AgentResponse::Authenticated(caps) => {
                let _ = self.caps.set(caps.clone());
                self.authenticated.store(true, Ordering::Release);
                Ok(AuthOutcome::Authorized(caps))
            }
            AgentResponse::AuthPending => Ok(AuthOutcome::Pending),
            AgentResponse::AuthDenied(message) => Ok(AuthOutcome::Denied(message)),
            AgentResponse::Failed(message) => Err(Error::agent(message)),
            AgentResponse::Pong | AgentResponse::Executed(_) => {
                Err(Error::agent("unexpected agent response to a RequestAuth"))
            }
        }
    }

    /// Whether this connection has authorized.
    #[must_use]
    pub fn is_session_authenticated(&self) -> bool {
        self.authenticated.load(Ordering::Acquire)
    }

    /// The [`AccessLevel`] granted to this connection, reported on authorization.
    /// Defaults to the most restrictive ([`Market`](AccessLevel::Market)) until
    /// then, so an unauthorized connection never appears more privileged than it is.
    #[must_use]
    pub fn access(&self) -> AccessLevel {
        self.caps
            .get()
            .map_or(AccessLevel::Market, |caps| caps.access)
    }

    /// Checks that the agent is responding (sent over the established
    /// connection). Only meaningful after authorization.
    pub async fn ping(&self) -> Result<()> {
        match self.round_trip(&AgentRequest::Ping).await? {
            AgentResponse::Pong => Ok(()),
            AgentResponse::Failed(message) => Err(Error::agent(message)),
            _ => Err(Error::agent("agent did not answer ping with Pong")),
        }
    }

    async fn round_trip(&self, request: &AgentRequest) -> Result<AgentResponse> {
        if self.broken.load(Ordering::Acquire) {
            return Err(Error::agent(
                "agent connection is unusable after a transport error; reconnect to continue",
            ));
        }
        // Hold the connection lock only for the write/read critical section, so a
        // request fully completes before the next one starts on the shared
        // stream; decoding happens after the guard is released. A write or read
        // error leaves the stream desynchronized, so mark it broken rather than
        // reuse it. (A decode failure is *not* a transport error — the frame was
        // read whole, so the stream stays in sync.)
        let bytes = {
            let mut conn = self.conn.lock().await;
            if let Err(e) = write_frame(&mut conn, request).await {
                self.broken.store(true, Ordering::Release);
                return Err(Error::agent(format!(
                    "failed to send request to agent: {e}"
                )));
            }
            match read_frame_bytes(&mut conn, MAX_RESPONSE_FRAME).await {
                Ok(bytes) => bytes,
                Err(e) => {
                    self.broken.store(true, Ordering::Release);
                    return Err(Error::agent(format!("failed to read agent response: {e}")));
                }
            }
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
                AgentResponse::Pong
                | AgentResponse::Authenticated(_)
                | AgentResponse::AuthPending
                | AgentResponse::AuthDenied(_) => {
                    Err(Error::agent("unexpected agent response to an Execute"))
                }
            }
        })
    }

    fn base_url(&self) -> &str {
        // Known only after authorization; empty until then (the agent path uses
        // this for display only — the agent itself joins paths to the real URL).
        self.caps.get().map_or("", |caps| caps.base_url.as_str())
    }

    fn is_authenticated(&self) -> bool {
        // The agent holds the credentials, so once this connection has authorized
        // it can sign on our behalf.
        self.is_session_authenticated()
    }
}

// --- server side -----------------------------------------------------------

/// Identifier assigned to each accepted connection, shown in the operator UI and
/// used to grant/deny it.
pub type ConnId = u64;

/// Operating-system credentials of a connecting peer.
///
/// Read from the socket and shown to the operator, so they can evaluate a connection
/// before granting it now that the socket is not restricted to the agent's own UID.
#[derive(Debug, Clone, Copy)]
pub struct PeerCred {
    /// Peer user id (`u32::MAX` if it could not be read).
    pub uid: u32,
    /// Peer group id (`u32::MAX` if it could not be read).
    pub gid: u32,
    /// Peer process id, when the platform reports it.
    pub pid: Option<i32>,
}

/// How a connection authorized (or is trying to).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    /// Presented the one-time token.
    Token,
    /// Interactive operator approval.
    Manual,
}

impl AuthMethod {
    /// The lowercase name of this method (`token` or `manual`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Token => "token",
            Self::Manual => "manual",
        }
    }
}

/// The lifecycle state of a connection in the agent's registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// Accepted, but has not asked to authorize yet.
    Connected,
    /// Asked for manual authorization at `requested`; awaiting the operator.
    Pending {
        /// The access the client asked for (already capped at the ceiling).
        requested: AccessLevel,
    },
    /// Authorized to serve requests up to `access`.
    Authorized {
        /// The granted tier, enforced on every forwarded request.
        access: AccessLevel,
    },
    /// Denied by the operator; refused everything.
    Denied,
}

/// A snapshot of one connection for the operator UI (see [`AgentControl::list`]).
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    /// The connection's id.
    pub id: ConnId,
    /// The peer's OS credentials.
    pub peer: PeerCred,
    /// The client-supplied label, if any (e.g. `revolutx-collector`).
    pub label: Option<String>,
    /// How it authorized, once it has attempted to.
    pub method: Option<AuthMethod>,
    /// Its current state.
    pub state: ConnState,
    /// When it connected.
    pub since: Instant,
}

/// The one-time token gate: consumed by the first connection to present it.
struct Gate {
    token: AuthToken,
    consumed: bool,
}

/// One tracked connection.
struct Entry {
    peer: PeerCred,
    label: Option<String>,
    method: Option<AuthMethod>,
    state: ConnState,
    since: Instant,
}

/// Shared, mutable state: every live connection plus the auth policy. Guarded by a
/// std mutex; critical sections are short and never span an `.await`.
struct Registry {
    conns: BTreeMap<ConnId, Entry>,
    next_id: ConnId,
    /// The maximum grantable [`AccessLevel`] (and the level a token grants).
    ceiling: AccessLevel,
    /// The optional one-time token; `None` when the agent is manual-only.
    token: Option<Gate>,
}

/// Locks a mutex, recovering the guard if a previous holder panicked. Poisoning is
/// not fatal here — the invariants are re-established on each short critical section.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The operator's handle to a running [`AgentServer`]: inspect connections and
/// grant/deny/quit. The `revolutx agent start` REPL drives these; it owns none of
/// the credentials.
#[derive(Clone)]
pub struct AgentControl {
    registry: Arc<Mutex<Registry>>,
    shutdown: Arc<Notify>,
}

impl AgentControl {
    /// A snapshot of every live connection, ordered by id.
    #[must_use]
    pub fn list(&self) -> Vec<ConnectionInfo> {
        let reg = lock(&self.registry);
        reg.conns
            .iter()
            .map(|(&id, e)| ConnectionInfo {
                id,
                peer: e.peer,
                label: e.label.clone(),
                method: e.method,
                state: e.state,
                since: e.since,
            })
            .collect()
    }

    /// Authorizes connection `id`. `access` defaults to what the connection
    /// requested; it must not exceed the agent's ceiling. Returns the granted level.
    pub fn grant(&self, id: ConnId, access: Option<AccessLevel>) -> Result<AccessLevel> {
        let mut reg = lock(&self.registry);
        let ceiling = reg.ceiling;
        let entry = reg
            .conns
            .get_mut(&id)
            .ok_or_else(|| Error::agent(format!("no connection #{id}")))?;
        let requested = match entry.state {
            ConnState::Pending { requested } => Some(requested),
            _ => None,
        };
        let level = access.or(requested).ok_or_else(|| {
            Error::agent(format!(
                "connection #{id} has not requested an access level; specify one, e.g. `grant {id} market`"
            ))
        })?;
        if level > ceiling {
            return Err(Error::agent(format!(
                "cannot grant `{}`: the agent's ceiling is `{}`",
                level.as_str(),
                ceiling.as_str()
            )));
        }
        entry.state = ConnState::Authorized { access: level };
        entry.method = Some(AuthMethod::Manual);
        drop(reg);
        Ok(level)
    }

    /// Denies connection `id`; it is refused everything until it disconnects.
    pub fn deny(&self, id: ConnId) -> Result<()> {
        let mut reg = lock(&self.registry);
        let entry = reg
            .conns
            .get_mut(&id)
            .ok_or_else(|| Error::agent(format!("no connection #{id}")))?;
        entry.state = ConnState::Denied;
        drop(reg);
        Ok(())
    }

    /// How many connections are currently authorized — the signal the daemon's idle
    /// auto-lock watches (it locks when this stays zero).
    #[must_use]
    pub fn active_count(&self) -> usize {
        let reg = lock(&self.registry);
        reg.conns
            .values()
            .filter(|e| matches!(e.state, ConnState::Authorized { .. }))
            .count()
    }

    /// Signals [`AgentServer::run`] to stop accepting and return.
    pub fn shutdown(&self) {
        self.shutdown.notify_one();
    }
}

/// A persistent, multi-client signing agent. Build it with [`new`](Self::new),
/// then [`run`](Self::run) the accept loop; drive authorization out of band through
/// the returned [`AgentControl`].
pub struct AgentServer {
    executor: Arc<dyn RequestExecutor>,
    registry: Arc<Mutex<Registry>>,
    shutdown: Arc<Notify>,
}

impl AgentServer {
    /// Builds an agent over `executor` with ceiling `access` and an optional
    /// one-time `token`. Returns the server and the operator [`AgentControl`].
    #[must_use]
    pub fn new(
        executor: Arc<dyn RequestExecutor>,
        access: AccessLevel,
        token: Option<AuthToken>,
    ) -> (Self, AgentControl) {
        let registry = Arc::new(Mutex::new(Registry {
            conns: BTreeMap::new(),
            next_id: 1,
            ceiling: access,
            token: token.map(|token| Gate {
                token,
                consumed: false,
            }),
        }));
        let shutdown = Arc::new(Notify::new());
        let control = AgentControl {
            registry: Arc::clone(&registry),
            shutdown: Arc::clone(&shutdown),
        };
        (
            Self {
                executor,
                registry,
                shutdown,
            },
            control,
        )
    }

    /// Accepts connections on `socket_path` and serves each until
    /// [`AgentControl::shutdown`] is called (or the future is dropped). Each
    /// connection is registered with its peer credentials and served in its own
    /// task; it is removed when it disconnects.
    ///
    /// The socket is created world-connectable (`0666`); if a live agent is already
    /// listening this errors, and a stale socket file is replaced.
    pub async fn run(self, socket_path: &Path) -> Result<()> {
        let listener = bind(socket_path).await?;
        let mut sessions = JoinSet::new();
        loop {
            tokio::select! {
                () = self.shutdown.notified() => break,
                accepted = listener.accept() => {
                    // Reap finished connections so the set does not grow with churn.
                    while sessions.try_join_next().is_some() {}
                    let Ok((stream, _addr)) = accepted else { continue };
                    let peer = read_peer_cred(&stream);
                    let id = register(&self.registry, peer);
                    let registry = Arc::clone(&self.registry);
                    let executor = Arc::clone(&self.executor);
                    sessions.spawn(async move {
                        handle_connection(stream, id, &registry, executor.as_ref()).await;
                        lock(&registry).conns.remove(&id);
                    });
                }
            }
        }
        Ok(())
    }
}

/// Reads the peer's credentials, falling back to a sentinel if unavailable.
fn read_peer_cred(stream: &UnixStream) -> PeerCred {
    stream.peer_cred().map_or(
        PeerCred {
            uid: u32::MAX,
            gid: u32::MAX,
            pid: None,
        },
        |c| PeerCred {
            uid: c.uid(),
            gid: c.gid(),
            pid: c.pid(),
        },
    )
}

/// Registers a freshly accepted connection as [`ConnState::Connected`] and returns
/// its new id.
fn register(registry: &Mutex<Registry>, peer: PeerCred) -> ConnId {
    let mut reg = lock(registry);
    let id = reg.next_id;
    reg.next_id += 1;
    reg.conns.insert(
        id,
        Entry {
            peer,
            label: None,
            method: None,
            state: ConnState::Connected,
            since: Instant::now(),
        },
    );
    id
}

/// Binds the socket, refusing to clobber a live agent and cleaning up a stale
/// socket file, then makes it connectable by any peer (`0666`) — the token /
/// operator-approval gate, not the file mode, is the trust boundary. Placing the
/// socket in a private directory further limits who can reach it.
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
    ensure_parent_dir(socket_path)?;
    let listener = UnixListener::bind(socket_path)
        .map_err(|e| Error::agent(format!("could not bind {}: {e}", socket_path.display())))?;
    set_socket_permissions(socket_path)?;
    Ok(listener)
}

#[cfg(unix)]
fn ensure_parent_dir(socket_path: &Path) -> Result<()> {
    let Some(parent) = socket_path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() || parent.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(parent)
        .map_err(|e| Error::agent(format!("could not create {}: {e}", parent.display())))
}

#[cfg(not(unix))]
fn ensure_parent_dir(_socket_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_socket_permissions(socket_path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o666))
        .map_err(|e| Error::agent(format!("could not set socket permissions: {e}")))
}

#[cfg(not(unix))]
fn set_socket_permissions(_socket_path: &Path) -> Result<()> {
    Ok(())
}

/// Serves one connection: it is unauthorized until it presents a valid token or the
/// operator grants it, and only auth requests are honored before then. After
/// authorization it serves `Ping`/`Execute` (gated per connection) until it
/// disconnects.
async fn handle_connection(
    mut stream: UnixStream,
    id: ConnId,
    registry: &Mutex<Registry>,
    executor: &dyn RequestExecutor,
) {
    let mut token_attempts: u32 = 0;
    loop {
        // A closed connection or a malformed frame just ends this session.
        let Ok(bytes) = read_frame_bytes(&mut stream, MAX_REQUEST_FRAME).await else {
            break;
        };
        let request: AgentRequest = match decode(&bytes) {
            Ok(request) => request,
            Err(_) => break,
        };

        let response = match request {
            AgentRequest::Authenticate(candidate) => {
                // Wipe the candidate after comparison — it is the token.
                let candidate = Zeroizing::new(candidate);
                match authorize_token(registry, id, executor, candidate.as_str()) {
                    TokenOutcome::Granted(caps) => AgentResponse::Authenticated(caps),
                    TokenOutcome::NoToken => AgentResponse::Failed(MSG_NO_TOKEN.to_owned()),
                    TokenOutcome::Bad => {
                        token_attempts += 1;
                        // Keep the connection open to retry, up to the attempt cap.
                        if token_attempts >= MAX_AUTH_ATTEMPTS {
                            let _ = write_frame(
                                &mut stream,
                                &AgentResponse::Failed(MSG_AUTH_FAILED.to_owned()),
                            )
                            .await;
                            break;
                        }
                        AgentResponse::Failed(MSG_AUTH_FAILED.to_owned())
                    }
                }
            }
            AgentRequest::RequestAuth { label, requested } => {
                request_manual(registry, id, executor, label, requested)
            }
            AgentRequest::Ping => match authorized_access(registry, id) {
                Some(_) => AgentResponse::Pong,
                None => AgentResponse::Failed(MSG_AUTH_REQUIRED.to_owned()),
            },
            AgentRequest::Execute(wire) => match authorized_access(registry, id) {
                Some(access) => execute_forwarded(executor, wire, access).await,
                None => AgentResponse::Failed(MSG_AUTH_REQUIRED.to_owned()),
            },
        };

        if write_frame(&mut stream, &response).await.is_err() {
            break;
        }
    }
}

/// The current authorized access of connection `id`, or `None` if it is not
/// authorized. Re-read on each request so an operator grant/deny takes effect at once.
fn authorized_access(registry: &Mutex<Registry>, id: ConnId) -> Option<AccessLevel> {
    let reg = lock(registry);
    match reg.conns.get(&id)?.state {
        ConnState::Authorized { access } => Some(access),
        _ => None,
    }
}

/// Outcome of a token handshake.
enum TokenOutcome {
    Granted(Capabilities),
    NoToken,
    Bad,
}

/// Verifies and consumes the one-time token for connection `id`, authorizing it at
/// the agent's ceiling on success.
fn authorize_token(
    registry: &Mutex<Registry>,
    id: ConnId,
    executor: &dyn RequestExecutor,
    candidate: &str,
) -> TokenOutcome {
    let ceiling;
    {
        let mut reg = lock(registry);
        ceiling = reg.ceiling;
        match reg.token.as_mut() {
            None => return TokenOutcome::NoToken,
            Some(gate) => {
                if gate.consumed || !gate.token.verify(candidate) {
                    return TokenOutcome::Bad;
                }
                gate.consumed = true;
            }
        }
        if let Some(entry) = reg.conns.get_mut(&id) {
            entry.state = ConnState::Authorized { access: ceiling };
            entry.method = Some(AuthMethod::Token);
        }
    }
    // Build the reply outside the lock (don't call the executor while holding it).
    TokenOutcome::Granted(Capabilities {
        base_url: executor.base_url().to_owned(),
        access: ceiling,
    })
}

/// Registers/refreshes a manual authorization request for connection `id` and
/// reports its current status (so a polling client sees a grant/denial take effect).
fn request_manual(
    registry: &Mutex<Registry>,
    id: ConnId,
    executor: &dyn RequestExecutor,
    label: Option<String>,
    requested: AccessLevel,
) -> AgentResponse {
    // Read the executor's base URL before locking, so we never call it under the lock.
    let base_url = executor.base_url().to_owned();
    let mut reg = lock(registry);
    let ceiling = reg.ceiling;
    let response = {
        let Some(entry) = reg.conns.get_mut(&id) else {
            return AgentResponse::Failed(MSG_AUTH_REQUIRED.to_owned());
        };
        match entry.state {
            ConnState::Authorized { access } => {
                AgentResponse::Authenticated(Capabilities { base_url, access })
            }
            ConnState::Denied => AgentResponse::AuthDenied(MSG_DENIED.to_owned()),
            ConnState::Connected | ConnState::Pending { .. } => {
                entry.state = ConnState::Pending {
                    requested: requested.min(ceiling),
                };
                if label.is_some() {
                    entry.label = label;
                }
                entry.method = Some(AuthMethod::Manual);
                AgentResponse::AuthPending
            }
        }
    };
    drop(reg);
    response
}

async fn execute_forwarded(
    executor: &dyn RequestExecutor,
    wire: WireRequest,
    access: AccessLevel,
) -> AgentResponse {
    let Ok(method) = Method::from_bytes(wire.method.as_bytes()) else {
        return AgentResponse::Failed(format!("invalid HTTP method '{}'", wire.method));
    };
    // Authoritative gate: classify the request by method+path (market data,
    // account read, or order mutation) and refuse anything above this connection's
    // granted tier. The agent — not the client — is the trust boundary.
    let required = required_access_for(method.as_str(), &wire.path);
    if !access.permits(required) {
        return AgentResponse::Failed(access_denied(required, access));
    }
    let spec =
        RequestSpec::from_parts(method, wire.path, wire.query, wire.body, wire.requires_auth);
    match executor.execute(spec).await {
        // Refuse an oversized body gracefully — sending it would overflow the client's
        // response frame and desync the connection for every later request.
        Ok(raw) if raw.body.len() > MAX_RESPONSE_BODY => AgentResponse::Failed(format!(
            "response too large: {} bytes exceeds the agent's {} MiB limit; narrow the request",
            raw.body.len(),
            MAX_RESPONSE_FRAME / (1024 * 1024),
        )),
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

    /// A stub executor that echoes the request path back as the response body, so a
    /// round-trip proves the request crossed the socket intact.
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

    fn spawn_agent(
        socket: PathBuf,
        ceiling: AccessLevel,
        token: Option<AuthToken>,
    ) -> (AgentControl, tokio::task::JoinHandle<Result<()>>) {
        let (server, control) = AgentServer::new(Arc::new(EchoExecutor), ceiling, token);
        let handle = tokio::spawn(async move { server.run(&socket).await });
        (control, handle)
    }

    /// Id of the single live connection (asserts there is exactly one).
    fn only_id(control: &AgentControl) -> ConnId {
        let conns = control.list();
        assert_eq!(conns.len(), 1, "expected exactly one connection");
        conns[0].id
    }

    #[test]
    fn token_generate_is_unique_and_verifies_in_constant_time() {
        let a = AuthToken::generate().unwrap();
        let b = AuthToken::generate().unwrap();
        assert_ne!(a.as_str(), b.as_str());
        assert!(a.verify(a.as_str()));
        assert!(!a.verify(b.as_str()));
        assert!(!a.verify(""));
        // Debug never leaks the secret.
        assert!(!format!("{a:?}").contains(a.as_str()));
    }

    #[tokio::test]
    async fn token_auth_round_trips_and_reports_capabilities() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let token = AuthToken::generate().unwrap();
        let secret = token.as_str().to_owned();
        let (control, server) = spawn_agent(socket.clone(), AccessLevel::View, Some(token));
        wait_for(&socket).await;

        let exec = AgentExecutor::connect(&socket).await.unwrap();
        assert!(!exec.is_session_authenticated());
        // Refused before authorizing.
        assert!(exec.execute(RequestSpec::get("/tickers")).await.is_err());

        exec.authenticate(&secret).await.unwrap();
        assert!(exec.is_session_authenticated());
        assert_eq!(exec.base_url(), "http://stub/api/1.0");
        assert_eq!(exec.access(), AccessLevel::View);

        let raw = exec
            .execute(RequestSpec::get("/orders/active"))
            .await
            .unwrap();
        assert_eq!(raw.body, b"/orders/active");
        // The registry shows it token-authorized at the ceiling.
        let info = &control.list()[0];
        assert!(matches!(
            info.state,
            ConnState::Authorized {
                access: AccessLevel::View
            }
        ));
        assert_eq!(info.method, Some(AuthMethod::Token));
        server.abort();
    }

    #[tokio::test]
    async fn token_is_single_use_across_connections() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let token = AuthToken::generate().unwrap();
        let secret = token.as_str().to_owned();
        let (_control, server) = spawn_agent(socket.clone(), AccessLevel::View, Some(token));
        wait_for(&socket).await;

        let first = AgentExecutor::connect(&socket).await.unwrap();
        first.authenticate(&secret).await.unwrap();
        // A second client presenting the spent token is refused.
        let second = AgentExecutor::connect(&socket).await.unwrap();
        assert!(second.authenticate(&secret).await.is_err());
        first.ping().await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn manual_pending_then_grant_authorizes_and_enforces_access() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (control, server) = spawn_agent(socket.clone(), AccessLevel::Trading, None);
        wait_for(&socket).await;

        // No token configured -> the token path is refused.
        let exec = AgentExecutor::connect(&socket).await.unwrap();
        assert!(exec.authenticate("anything").await.is_err());

        // Request manual auth -> pending, visible with peer creds + label.
        let outcome = exec
            .request_authorization(Some("collector"), AccessLevel::Market)
            .await
            .unwrap();
        assert!(matches!(outcome, AuthOutcome::Pending));
        let info = control.list()[0].clone();
        assert_eq!(info.label.as_deref(), Some("collector"));
        assert!(matches!(
            info.state,
            ConnState::Pending {
                requested: AccessLevel::Market
            }
        ));
        assert_ne!(info.peer.uid, u32::MAX, "peer uid should be readable");

        // Grant at the requested level; the next poll is authorized.
        assert_eq!(control.grant(info.id, None).unwrap(), AccessLevel::Market);
        let outcome = exec
            .request_authorization(Some("collector"), AccessLevel::Market)
            .await
            .unwrap();
        assert!(matches!(outcome, AuthOutcome::Authorized(_)));
        assert_eq!(exec.access(), AccessLevel::Market);

        // Market access: public data ok, account read refused.
        assert!(exec.execute(RequestSpec::get("/tickers")).await.is_ok());
        assert!(exec.execute(RequestSpec::get("/balances")).await.is_err());
        server.abort();
    }

    #[tokio::test]
    async fn manual_deny_is_reported_and_refuses_requests() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (control, server) = spawn_agent(socket.clone(), AccessLevel::Market, None);
        wait_for(&socket).await;

        let exec = AgentExecutor::connect(&socket).await.unwrap();
        exec.request_authorization(None, AccessLevel::Market)
            .await
            .unwrap();
        control.deny(only_id(&control)).unwrap();
        let outcome = exec
            .request_authorization(None, AccessLevel::Market)
            .await
            .unwrap();
        assert!(matches!(outcome, AuthOutcome::Denied(_)));
        assert!(exec.execute(RequestSpec::get("/tickers")).await.is_err());
        server.abort();
    }

    #[tokio::test]
    async fn two_clients_authorized_at_different_levels() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (control, server) = spawn_agent(socket.clone(), AccessLevel::View, None);
        wait_for(&socket).await;

        let a = AgentExecutor::connect(&socket).await.unwrap();
        let b = AgentExecutor::connect(&socket).await.unwrap();
        a.request_authorization(Some("a"), AccessLevel::Market)
            .await
            .unwrap();
        b.request_authorization(Some("b"), AccessLevel::View)
            .await
            .unwrap();

        // Grant each by looking up its id via label.
        for info in control.list() {
            let level = if info.label.as_deref() == Some("a") {
                AccessLevel::Market
            } else {
                AccessLevel::View
            };
            control.grant(info.id, Some(level)).unwrap();
        }
        a.request_authorization(Some("a"), AccessLevel::Market)
            .await
            .unwrap();
        b.request_authorization(Some("b"), AccessLevel::View)
            .await
            .unwrap();

        // a (market) cannot read the account; b (view) can. Per-connection access.
        assert!(a.execute(RequestSpec::get("/balances")).await.is_err());
        assert!(b.execute(RequestSpec::get("/balances")).await.is_ok());
        assert_eq!(control.active_count(), 2);
        server.abort();
    }

    #[tokio::test]
    async fn grant_above_ceiling_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (control, server) = spawn_agent(socket.clone(), AccessLevel::Market, None);
        wait_for(&socket).await;

        let exec = AgentExecutor::connect(&socket).await.unwrap();
        exec.request_authorization(None, AccessLevel::Market)
            .await
            .unwrap();
        let id = only_id(&control);
        assert!(control.grant(id, Some(AccessLevel::Trading)).is_err());
        // A refused grant leaves the connection pending.
        assert!(matches!(control.list()[0].state, ConnState::Pending { .. }));
        server.abort();
    }

    #[tokio::test]
    async fn active_count_drops_when_the_client_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (control, server) = spawn_agent(socket.clone(), AccessLevel::Market, None);
        wait_for(&socket).await;

        let exec = AgentExecutor::connect(&socket).await.unwrap();
        exec.request_authorization(None, AccessLevel::Market)
            .await
            .unwrap();
        control.grant(only_id(&control), None).unwrap();
        exec.request_authorization(None, AccessLevel::Market)
            .await
            .unwrap();
        assert_eq!(control.active_count(), 1);

        // The handler removes the entry on disconnect, so the idle signal drops.
        drop(exec);
        for _ in 0..200 {
            if control.active_count() == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(control.active_count(), 0);
        server.abort();
    }

    #[tokio::test]
    async fn shutdown_stops_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (control, server) = spawn_agent(socket.clone(), AccessLevel::Market, None);
        wait_for(&socket).await;
        control.shutdown();
        let outcome = tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("server should return after shutdown");
        assert!(outcome.unwrap().is_ok());
    }

    /// Returns an oversized body for `/candles/*` (a market-tier path) and a small
    /// echo otherwise — to exercise the response-size guard.
    #[derive(Debug)]
    struct BigBodyExecutor;

    impl RequestExecutor for BigBodyExecutor {
        fn execute(&self, request: RequestSpec) -> BoxFuture<'_, Result<RawResponse>> {
            let body = if request.path().starts_with("/candles") {
                vec![b'x'; MAX_RESPONSE_BODY + 1]
            } else {
                request.path().as_bytes().to_vec()
            };
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

    #[tokio::test]
    async fn oversized_response_is_refused_and_the_connection_survives() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (server, control) =
            AgentServer::new(Arc::new(BigBodyExecutor), AccessLevel::Market, None);
        let task = tokio::spawn({
            let socket = socket.clone();
            async move { server.run(&socket).await }
        });
        wait_for(&socket).await;

        let exec = AgentExecutor::connect(&socket).await.unwrap();
        exec.request_authorization(None, AccessLevel::Market)
            .await
            .unwrap();
        control.grant(control.list()[0].id, None).unwrap();
        exec.request_authorization(None, AccessLevel::Market)
            .await
            .unwrap();

        // The oversized candle response is refused gracefully, not sent as a giant frame.
        let err = exec
            .execute(RequestSpec::get("/candles/BTC-USD"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("too large"), "got: {err}");

        // The connection is NOT poisoned — normal requests still round-trip.
        let raw = exec.execute(RequestSpec::get("/tickers")).await.unwrap();
        assert_eq!(raw.body, b"/tickers");
        assert!(exec.execute(RequestSpec::get("/tickers")).await.is_ok());

        task.abort();
    }
}
