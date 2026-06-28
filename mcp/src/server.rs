//! MCP server: configuration and JSON-RPC request dispatch.

use std::path::PathBuf;
use std::sync::Arc;

use revolutx::agent::{AgentExecutor, default_socket_path};
use revolutx::commands::{JsonPresenter, Presenter};
use revolutx::{RequestExecutor, RevolutXClient};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::protocol::{
    INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR, error, success,
};
use crate::tools;

/// MCP protocol version advertised when the client does not request one.
const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "revolutx-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Tool error: the `authenticate` call was missing its token argument.
const MISSING_TOKEN: &str = "authenticate requires a 'token' string argument";
/// Tool error: a tool other than `authenticate` was called before this session has
/// authenticated with the agent.
const NOT_AUTHENTICATED: &str = "authenticate first: call the `authenticate` tool with the one-time token from \
     `revolutx agent start --auth-token`";

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
}

impl Server {
    /// Builds a server targeting `socket`. No connection is made here.
    fn with_socket(socket: PathBuf) -> Self {
        Self {
            socket,
            session: Mutex::new(None),
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
        // Echo the client's requested protocol version when present.
        let protocol_version = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_PROTOCOL_VERSION);

        json!({
            "protocolVersion": protocol_version,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
            "instructions": "Tools for the Revolut X crypto exchange, served via a signing agent. Call `authenticate` FIRST with the one-time token the operator obtained from `revolutx agent start --auth-token`; until you do, every other tool fails with \"authenticate first\". The agent serves a fixed access tier (reported on authenticate): account reads need `--access view` and order placement/cancellation need `--access trading`; tools above the tier return \"access denied\"."
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
            Err(e) => success(id, tool_content(e.to_string(), true)),
        }
    }

    /// Handles the `authenticate` tool: opens a fresh connection to the agent and
    /// presents the one-time token.
    ///
    /// This is the only place the socket is opened, so the agent can be started or
    /// restarted independently of the MCP — re-calling `authenticate` (with the new
    /// token a restarted agent prints) reconnects and supersedes any prior session.
    /// On success the agent reveals its environment and access policy, and every
    /// other tool becomes usable (subject to that policy).
    async fn authenticate(&self, id: Value, args: &Value) -> Value {
        let Some(token) = args.get(tools::ARG_TOKEN).and_then(Value::as_str) else {
            return success(id, tool_content(MISSING_TOKEN, true));
        };
        let executor = match AgentExecutor::connect(&self.socket).await {
            Ok(executor) => Arc::new(executor),
            Err(e) => {
                return success(
                    id,
                    tool_content(
                        format!(
                            "could not connect to the agent at {sock}: {e}. Start it with \
                             `revolutx agent start --auth-token --socket {sock}`.",
                            sock = self.socket.display(),
                        ),
                        true,
                    ),
                );
            }
        };
        match executor.authenticate(token).await {
            Ok(()) => {
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
            Err(e) => success(id, tool_content(e.to_string(), true)),
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
    use super::*;

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
    async fn initialize_echoes_version_and_advertises_tools() {
        let server = public_server();
        let resp = handle(
            &server,
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(resp["result"]["serverInfo"]["name"], "revolutx-mcp");
        assert!(resp["result"]["capabilities"]["tools"].is_object());
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
    async fn authenticate_validates_token_and_reports_connection_failure() {
        // Missing token argument is a tool error, not a protocol error.
        let resp = handle(
            &public_server(),
            json!({
                "jsonrpc": "2.0", "id": 5, "method": "tools/call",
                "params": { "name": "authenticate", "arguments": {} }
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
                .contains("token")
        );

        // With a token but no agent listening on the socket, it is a tool error
        // describing the connection failure (the socket is opened lazily, here).
        let resp = handle(
            &public_server(),
            json!({
                "jsonrpc": "2.0", "id": 6, "method": "tools/call",
                "params": { "name": "authenticate", "arguments": { "token": "x" } }
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp["result"]["isError"], true);
        assert!(
            resp["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("could not connect to the agent")
        );
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
