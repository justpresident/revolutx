//! MCP server: configuration and JSON-RPC request dispatch.

use std::path::PathBuf;
use std::sync::Arc;

use revolutx::agent::{AgentExecutor, AuthOutcome, default_socket_path};
use revolutx::commands::{JsonPresenter, Presenter};
use revolutx::{AccessLevel, RequestExecutor, RevolutXClient};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::protocol::{
    INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR, error, success,
};
use crate::tools;

/// MCP protocol version advertised when the client does not request one (the
/// server's preferred revision).
const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";
/// Protocol revisions this server actually implements. A client asking for one of
/// these gets it back; anything else is answered with [`DEFAULT_PROTOCOL_VERSION`]
/// so the client can decide, rather than blindly echoing an unsupported version.
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 1] = [DEFAULT_PROTOCOL_VERSION];
const SERVER_NAME: &str = "revolutx-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Label this connection presents to the agent's operator console.
const MCP_LABEL: &str = "revolutx-mcp";
/// The access tier interactive authorization requests when none is given.
const DEFAULT_REQUEST_ACCESS: AccessLevel = AccessLevel::View;
/// Tool error: a tool other than `authenticate` was called before this session has
/// authenticated with the agent.
const NOT_AUTHENTICATED: &str = "authenticate first: call the `authenticate` tool — with the one-time token \
     from `revolutx agent start --auth-token`, or with no token to request interactive operator approval";

/// An MCP server that proxies to a `revolutx` signing agent over a unix socket.
///
/// The MCP and the agent are **independent processes**: this server resolves the
/// socket path at startup but does not touch it until the `authenticate` tool is
/// called, and it reconnects on every authenticate. So the agent can be started,
/// stopped, or restarted at any time — after a restart the LLM simply calls
/// `authenticate` again with the new token and the session is re-established. The
/// MCP holds no key material; the agent owns the keystore, environment, and the
/// access policy and does all signing and HTTP.
pub struct Server {
    /// The agent's unix socket path. Read once at startup; only ever opened by
    /// `authenticate` (the socket file need not exist before then).
    socket: PathBuf,
    /// The current authenticated connection, established by `authenticate` and
    /// replaced on each re-authenticate. `None` until the first successful
    /// authentication.
    session: Mutex<Option<Arc<AgentExecutor>>>,
    /// A connection awaiting interactive operator approval, held across
    /// `authenticate` polls so re-calling reuses the *same* console entry rather
    /// than creating a new pending one. Cleared once approved, denied, or dropped.
    pending: Mutex<Option<Arc<AgentExecutor>>>,
}

impl Server {
    /// Builds a server targeting `socket`. No connection is made here.
    fn with_socket(socket: PathBuf) -> Self {
        Self {
            socket,
            session: Mutex::new(None),
            pending: Mutex::new(None),
        }
    }

    /// Builds the server from the environment. Does **not** connect to the agent —
    /// the connection is made lazily on `authenticate`, so the MCP starts cleanly
    /// whether or not an agent is running yet, and survives the agent restarting.
    /// Configuration is a single, optional, non-sensitive variable:
    ///
    /// - `REVOLUTX_AGENT_SOCKET` — the agent's unix socket path (default
    ///   `$XDG_RUNTIME_DIR/revolutx-agent.sock`).
    pub fn from_env() -> Self {
        let socket =
            env_nonempty("REVOLUTX_AGENT_SOCKET").map_or_else(default_socket_path, Into::into);
        Self::with_socket(socket)
    }

    /// Handles one JSON-RPC message line. Returns the serialized response line,
    /// or `None` for notifications (which must not be answered).
    pub async fn handle_line(&self, line: &str) -> Option<String> {
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(e) => {
                return Some(
                    error(Value::Null, PARSE_ERROR, format!("parse error: {e}")).to_string(),
                );
            }
        };

        // Reject anything that isn't a single JSON-RPC request object (arrays /
        // batches, scalars) with an error rather than silently dropping it, which
        // would hang a client waiting for a response.
        if !value.is_object() {
            return Some(
                error(
                    Value::Null,
                    INVALID_REQUEST,
                    "expected a single JSON-RPC request object",
                )
                .to_string(),
            );
        }

        // A request has an `id`; a notification does not.
        let id = value.get("id").cloned();
        let method = value.get("method").and_then(Value::as_str);

        let Some(id) = id else {
            // Notification (e.g. notifications/initialized): no response.
            return None;
        };

        let Some(method) = method else {
            return Some(error(id, INVALID_REQUEST, "missing 'method'").to_string());
        };

        let params = value.get("params").cloned().unwrap_or(Value::Null);

        let response = match method {
            "initialize" => success(id, Self::initialize_result(&params)),
            "ping" => success(id, json!({})),
            "tools/list" => success(id, json!({ "tools": tools::list() })),
            "tools/call" => self.handle_tools_call(id, &params).await,
            other => error(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
        };

        Some(response.to_string())
    }

    fn initialize_result(params: &Value) -> Value {
        // Negotiate: honor the client's requested version only if we actually
        // implement it; otherwise answer with our preferred version and let the
        // client decide, rather than echoing a version we don't speak.
        let protocol_version = match params.get("protocolVersion").and_then(Value::as_str) {
            Some(v) if SUPPORTED_PROTOCOL_VERSIONS.contains(&v) => v,
            _ => DEFAULT_PROTOCOL_VERSION,
        };

        json!({
            "protocolVersion": protocol_version,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
            "instructions": "Tools for the Revolut X crypto exchange, served via a signing agent. Call `authenticate` FIRST — either with the one-time `token` from `revolutx agent start --auth-token`, or with NO token to request interactive operator approval (the operator grants this connection in the agent console; you get an \"awaiting operator approval\" reply, then call `authenticate` again). Until you do, every other tool fails with \"authenticate first\". The agent enforces a per-connection access tier (reported on authenticate): account reads need `view` and order placement/cancellation need `trading`; tools above the granted tier return \"access denied\"."
        })
    }

    async fn handle_tools_call(&self, id: Value, params: &Value) -> Value {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return error(id, INVALID_PARAMS, "tools/call requires a 'name'");
        };
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        // `authenticate` is handled here (it acts on the agent connection, which
        // the shared command layer does not see); every other tool maps to a
        // `Command`, runs through the shared dispatcher, and is presented as JSON —
        // the same path the CLI uses. The agent still enforces both gates.
        if name == tools::AUTHENTICATE {
            return self.authenticate(id, &args).await;
        }

        let command = match tools::to_command(name, &args) {
            Ok(command) => command,
            Err(message) => return success(id, tool_content(message, true)),
        };
        // Run on the connection `authenticate` established; without one nothing can
        // run yet. The agent itself still enforces auth + the access gate on top.
        let Some(agent) = self.session.lock().await.clone() else {
            return success(id, tool_content(NOT_AUTHENTICATED, true));
        };
        let client = RevolutXClient::with_executor(agent as Arc<_>);
        match revolutx::commands::execute(&client, command).await {
            Ok(output) => match JsonPresenter.present(&output) {
                Ok(text) => success(id, tool_content(text, false)),
                Err(e) => success(id, tool_content(e.to_string(), true)),
            },
            // A broken agent connection never recovers on the same executor:
            // drop the dead session so the next call reconnects, and tell the
            // caller to re-authenticate rather than repeating the same error.
            Err(e) if e.is_connection_unusable() => {
                *self.session.lock().await = None;
                success(
                    id,
                    tool_content(
                        format!("{e}. The agent connection was lost — call `authenticate` again."),
                        true,
                    ),
                )
            }
            Err(e) => success(id, tool_content(e.to_string(), true)),
        }
    }

    /// Handles the `authenticate` tool. With a `token` it presents it for immediate
    /// authorization; without one it requests interactive operator approval and
    /// reports the status (call again once the operator grants it).
    ///
    /// This is the only place the socket is opened, so the agent can be started or
    /// restarted independently of the MCP. On success every other tool becomes usable,
    /// subject to the agent's per-connection access policy.
    async fn authenticate(&self, id: Value, args: &Value) -> Value {
        // A present-but-non-string token is a caller mistake, not a request for
        // interactive auth — surface it rather than silently switching modes.
        match args.get(tools::ARG_TOKEN) {
            Some(Value::String(t)) if !t.trim().is_empty() => {
                return self.authenticate_with_token(id, t).await;
            }
            None | Some(Value::Null | Value::String(_)) => {} // absent/empty → interactive
            Some(_) => {
                return success(
                    id,
                    tool_content(
                        "argument 'token' must be a string; omit it to request interactive approval",
                        true,
                    ),
                );
            }
        }
        match requested_access(args) {
            Ok(requested) => self.authenticate_interactively(id, requested).await,
            Err(message) => success(id, tool_content(message, true)),
        }
    }

    /// Token path: connect fresh and present the one-time token (grants the ceiling).
    async fn authenticate_with_token(&self, id: Value, token: &str) -> Value {
        let executor = match self.connect().await {
            Ok(executor) => executor,
            Err(message) => return success(id, tool_content(message, true)),
        };
        match executor.authenticate(token).await {
            Ok(()) => {
                *self.pending.lock().await = None;
                self.store_session(id, executor).await
            }
            Err(e) => success(id, tool_content(e.to_string(), true)),
        }
    }

    /// Interactive path: request operator approval at `requested` and report status.
    /// The pending connection is kept between calls so re-polling reuses the same
    /// console entry; call again after the operator grants it.
    async fn authenticate_interactively(&self, id: Value, requested: AccessLevel) -> Value {
        // Bind to a statement so the guard drops here — re-locking `pending` below
        // (a non-reentrant mutex) while still holding it would deadlock.
        let existing = self.pending.lock().await.clone();
        let executor = match existing {
            Some(executor) => executor,
            None => match self.connect().await {
                Ok(executor) => {
                    *self.pending.lock().await = Some(Arc::clone(&executor));
                    executor
                }
                Err(message) => return success(id, tool_content(message, true)),
            },
        };
        match executor
            .request_authorization(Some(MCP_LABEL), requested)
            .await
        {
            Ok(AuthOutcome::Authorized(_)) => {
                *self.pending.lock().await = None;
                self.store_session(id, executor).await
            }
            Ok(AuthOutcome::Pending) => success(
                id,
                tool_content(
                    format!(
                        "Awaiting operator approval. In the agent console, run `list` and \
                         `grant <id> {}` for the `{MCP_LABEL}` connection, then call \
                         `authenticate` again.",
                        requested.as_str()
                    ),
                    false,
                ),
            ),
            Ok(AuthOutcome::Denied(reason)) => {
                *self.pending.lock().await = None;
                success(
                    id,
                    tool_content(
                        format!("The operator denied this connection: {reason}"),
                        true,
                    ),
                )
            }
            Err(e) => {
                // A stale/closed connection: drop it so the next call reconnects.
                *self.pending.lock().await = None;
                success(
                    id,
                    tool_content(
                        format!("agent connection failed: {e}. Call `authenticate` again."),
                        true,
                    ),
                )
            }
        }
    }

    /// Opens a fresh connection to the agent, or returns a human-readable error.
    async fn connect(&self) -> Result<Arc<AgentExecutor>, String> {
        AgentExecutor::connect(&self.socket)
            .await
            .map(Arc::new)
            .map_err(|e| {
                format!(
                    "could not connect to the agent at {sock}: {e}. Start it with \
                     `revolutx agent start --socket {sock}`.",
                    sock = self.socket.display(),
                )
            })
    }

    /// Records `executor` as the active session and reports the granted policy.
    async fn store_session(&self, id: Value, executor: Arc<AgentExecutor>) -> Value {
        let text = format!(
            "Authenticated with the signing agent. Environment: {}; access: {} \
             (the agent enforces this — tools above this tier are refused).",
            executor.base_url(),
            executor.access().as_str(),
        );
        // Supersede any prior connection (e.g. to a now-restarted agent).
        *self.session.lock().await = Some(executor);
        success(id, tool_content(text, false))
    }
}

/// The access tier an interactive `authenticate` requests (default view). An
/// unrecognized or non-string tier is an error, not a silent downgrade to the
/// default.
fn requested_access(args: &Value) -> Result<AccessLevel, String> {
    match args.get(tools::ARG_ACCESS) {
        None | Some(Value::Null) => Ok(DEFAULT_REQUEST_ACCESS),
        Some(Value::String(s)) => s.parse::<AccessLevel>().map_err(|e| e.to_string()),
        Some(_) => {
            Err("argument 'access' must be a string tier (market, view, or trading)".to_owned())
        }
    }
}

/// Builds an MCP `tools/call` result body with a single text content block.
fn tool_content(text: impl Into<String>, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }], "isError": is_error })
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use revolutx::agent::AgentServer;
    use revolutx::transport::BoxFuture;
    use revolutx::{RawResponse, RequestSpec, Result as RxResult};

    use super::*;

    /// A stub agent executor that echoes the request path — enough for a live
    /// in-process [`AgentServer`] to exercise the MCP's authorization wiring.
    #[derive(Debug)]
    struct StubExec;

    impl RequestExecutor for StubExec {
        fn execute(&self, request: RequestSpec) -> BoxFuture<'_, RxResult<RawResponse>> {
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

    async fn wait_for_socket(path: &Path) {
        for _ in 0..200 {
            if path.exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("socket never appeared");
    }

    /// Invokes one `tools/call` and returns the parsed response.
    async fn call(server: &Server, name: &str, args: Value) -> Value {
        handle(
            server,
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": name, "arguments": args }
            }),
        )
        .await
        .unwrap()
    }

    fn text_of(resp: &Value) -> String {
        resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn public_server() -> Server {
        // Points at a socket with no agent listening: `authenticate` reports a
        // connection failure and other tools report "authenticate first". The
        // protocol dispatch under test never reaches a live agent.
        Server::with_socket("/nonexistent/revolutx-agent.sock".into())
    }

    async fn handle(server: &Server, msg: Value) -> Option<Value> {
        server
            .handle_line(&msg.to_string())
            .await
            .map(|s| serde_json::from_str(&s).unwrap())
    }

    #[tokio::test]
    async fn initialize_negotiates_version_and_advertises_tools() {
        let server = public_server();

        // A supported version is honored.
        let resp = handle(
            &server,
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(resp["result"]["serverInfo"]["name"], "revolutx-mcp");
        assert!(resp["result"]["capabilities"]["tools"].is_object());

        // An unsupported version is answered with the server's own, not echoed.
        let resp = handle(
            &server,
            json!({
                "jsonrpc": "2.0", "id": 2, "method": "initialize",
                "params": { "protocolVersion": "1999-01-01", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    }

    #[tokio::test]
    async fn notifications_get_no_response() {
        let server = public_server();
        let out = server
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await;
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let server = public_server();
        let resp = handle(
            &server,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "frobnicate" }),
        )
        .await
        .unwrap();
        assert_eq!(resp["error"]["code"], METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn ping_returns_empty_result() {
        let server = public_server();
        let resp = handle(
            &server,
            json!({ "jsonrpc": "2.0", "id": 3, "method": "ping" }),
        )
        .await
        .unwrap();
        assert!(resp["result"].is_object());
        assert!(resp.get("error").is_none());
    }

    #[tokio::test]
    async fn tools_list_advertises_authenticate_and_all_tools() {
        let resp = handle(
            &public_server(),
            json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/list" }),
        )
        .await
        .unwrap();
        let names: Vec<&str> = resp["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        // The catalog is fixed; the agent gates auth + trading at call time.
        assert!(names.contains(&"authenticate"));
        assert!(names.contains(&"get_balances"));
        assert!(names.contains(&"place_limit_order"));
    }

    #[tokio::test]
    async fn authenticate_reports_connection_failure_when_no_agent() {
        // Both paths open the socket lazily; with no agent listening, each is a tool
        // error describing the connection failure — token OR interactive (no token).
        let server = public_server();
        for args in [json!({}), json!({ "token": "x" })] {
            let resp = call(&server, "authenticate", args).await;
            assert!(resp.get("error").is_none());
            assert_eq!(resp["result"]["isError"], true);
            assert!(text_of(&resp).contains("could not connect to the agent"));
        }
    }

    #[tokio::test]
    async fn interactive_authenticate_pending_then_operator_grant() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (agent, control) = AgentServer::new(Arc::new(StubExec), AccessLevel::View, None);
        let server_task = tokio::spawn({
            let socket = socket.clone();
            async move { agent.run(&socket).await }
        });
        wait_for_socket(&socket).await;

        let mcp = Server::with_socket(socket.clone());

        // No token -> registers a pending connection and reports "awaiting approval".
        let r1 = call(&mcp, "authenticate", json!({})).await;
        assert_eq!(r1["result"]["isError"], false);
        assert!(text_of(&r1).contains("Awaiting operator approval"));

        // The operator sees the labelled connection and grants it.
        let info = control
            .list()
            .into_iter()
            .find(|c| c.label.as_deref() == Some("revolutx-mcp"))
            .expect("pending mcp connection");
        control.grant(info.id, None).unwrap();

        // Re-calling authenticate reuses the SAME connection and completes.
        let r2 = call(&mcp, "authenticate", json!({})).await;
        assert_eq!(r2["result"]["isError"], false);
        assert!(text_of(&r2).contains("access: view"));

        // A real tool now runs on the session — it is past "authenticate first".
        let r3 = call(&mcp, "get_tickers", json!({})).await;
        assert!(!text_of(&r3).contains("authenticate first"));

        server_task.abort();
    }

    #[tokio::test]
    async fn interactive_authenticate_reports_denial() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (agent, control) = AgentServer::new(Arc::new(StubExec), AccessLevel::Market, None);
        let server_task = tokio::spawn({
            let socket = socket.clone();
            async move { agent.run(&socket).await }
        });
        wait_for_socket(&socket).await;

        let mcp = Server::with_socket(socket.clone());
        call(&mcp, "authenticate", json!({})).await; // register pending
        let id = control.list()[0].id;
        control.deny(id).unwrap();

        let denied = call(&mcp, "authenticate", json!({})).await;
        assert_eq!(denied["result"]["isError"], true);
        assert!(text_of(&denied).contains("denied"));

        server_task.abort();
    }

    #[tokio::test]
    async fn tool_before_authenticate_is_authenticate_first() {
        // A real tool (not `authenticate`) before any session reports "authenticate
        // first" — it never touches the socket.
        let resp = handle(
            &public_server(),
            json!({
                "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                "params": { "name": "get_tickers", "arguments": {} }
            }),
        )
        .await
        .unwrap();
        assert!(resp.get("error").is_none());
        assert_eq!(resp["result"]["isError"], true);
        assert!(
            resp["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("authenticate first")
        );
    }

    #[tokio::test]
    async fn unknown_tool_is_iserror_not_protocol_error() {
        let resp = handle(
            &public_server(),
            json!({
                "jsonrpc": "2.0", "id": 6, "method": "tools/call",
                "params": { "name": "nope", "arguments": {} }
            }),
        )
        .await
        .unwrap();
        assert!(resp.get("error").is_none());
        assert_eq!(resp["result"]["isError"], true);
        assert!(
            resp["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("unknown tool")
        );
    }

    #[tokio::test]
    async fn invalid_json_is_parse_error() {
        let server = public_server();
        let resp: Value =
            serde_json::from_str(&server.handle_line("{not json").await.unwrap()).unwrap();
        assert_eq!(resp["error"]["code"], PARSE_ERROR);
        assert!(resp["id"].is_null());
    }
}
