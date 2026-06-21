//! MCP server: configuration and JSON-RPC request dispatch.

use std::sync::Arc;

use revolutx::agent::AgentExecutor;
use revolutx::{Environment, RevolutXClient};
use serde_json::{Value, json};

use crate::protocol::{
    INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR, error, success,
};
use crate::tools;

/// MCP protocol version advertised when the client does not request one.
const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "revolutx-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A configured MCP server over a `revolutx` client.
pub struct Server {
    client: RevolutXClient,
    trading_enabled: bool,
}

impl Server {
    #[cfg(test)]
    pub const fn new(client: RevolutXClient, trading_enabled: bool) -> Self {
        Self {
            client,
            trading_enabled,
        }
    }

    /// Builds the server from environment variables:
    ///
    /// - `REVOLUTX_AGENT_SOCKET` — path to a running `revolutx agent`'s unix
    ///   socket. **The secure path:** the agent holds the keystore and does all
    ///   signing + HTTP, so the MCP keeps no key material of its own. When set,
    ///   the credential variables below are ignored.
    /// - `REVOLUTX_API_KEY` + (`REVOLUTX_PRIVATE_KEY_PEM` or
    ///   `REVOLUTX_PRIVATE_KEY_PATH`) — **plaintext credentials, a dev fallback**
    ///   used only when `REVOLUTX_AGENT_SOCKET` is unset (optional; without them
    ///   only the public tools work).
    /// - `REVOLUTX_ENVIRONMENT` — `production` (default) or `dev`.
    /// - `REVOLUTX_MCP_ENABLE_TRADING` — set to a truthy value to expose the
    ///   order-mutating tools.
    pub async fn from_env() -> Result<Self, String> {
        let trading_enabled = env_flag("REVOLUTX_MCP_ENABLE_TRADING");
        let client = Self::build_client().await?;
        Ok(Self {
            client,
            trading_enabled,
        })
    }

    async fn build_client() -> Result<RevolutXClient, String> {
        if let Some(socket) = env_nonempty("REVOLUTX_AGENT_SOCKET") {
            // Secure path: delegate all signing + HTTP to the agent. The MCP
            // holds no secrets, so it needs no process hardening of its own.
            let executor = AgentExecutor::connect(&socket, environment_from_env().base_url())
                .await
                .map_err(|e| format!("could not connect to the agent at {socket}: {e}"))?;
            Ok(RevolutXClient::with_executor(Arc::new(executor)))
        } else {
            // Explicit dev fallback: plaintext credentials from REVOLUTX_*.
            revolutx::client_from_env().map_err(|e| e.to_string())
        }
    }

    pub fn is_authenticated(&self) -> bool {
        self.client.is_authenticated()
    }

    pub const fn trading_enabled(&self) -> bool {
        self.trading_enabled
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
            "tools/list" => success(id, json!({ "tools": tools::list(self.trading_enabled) })),
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
            "instructions": "Tools for the Revolut X crypto exchange. Read-only market data and account tools are always available; order placement and cancellation require the server to be started with REVOLUTX_MCP_ENABLE_TRADING=1."
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

        match tools::call(&self.client, self.trading_enabled, name, &args).await {
            Ok(text) => success(
                id,
                json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
            ),
            Err(message) => success(
                id,
                json!({ "content": [{ "type": "text", "text": message }], "isError": true }),
            ),
        }
    }
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// The target environment from `REVOLUTX_ENVIRONMENT` (default production). Only
/// used for the agent executor's reported base URL; the agent owns the real one.
fn environment_from_env() -> Environment {
    match env_nonempty("REVOLUTX_ENVIRONMENT").as_deref() {
        Some(v)
            if matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "dev" | "development"
            ) =>
        {
            Environment::Dev
        }
        _ => Environment::Production,
    }
}

fn env_flag(name: &str) -> bool {
    matches!(
        env_nonempty(name).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn public_server(trading: bool) -> Server {
        // No credentials: authenticated tools would fail, but the protocol and
        // gating logic under test never reach the network.
        let client = RevolutXClient::builder()
            .base_url("http://127.0.0.1:1/api/1.0")
            .build()
            .unwrap();
        Server::new(client, trading)
    }

    async fn handle(server: &Server, msg: Value) -> Option<Value> {
        server
            .handle_line(&msg.to_string())
            .await
            .map(|s| serde_json::from_str(&s).unwrap())
    }

    #[tokio::test]
    async fn initialize_echoes_version_and_advertises_tools() {
        let server = public_server(false);
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
        let server = public_server(false);
        let out = server
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await;
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let server = public_server(false);
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
        let server = public_server(false);
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
    async fn tools_list_reflects_trading_gate() {
        let resp = handle(
            &public_server(false),
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
        assert!(names.contains(&"get_balances"));
        assert!(!names.contains(&"place_limit_order"));
    }

    #[tokio::test]
    async fn calling_trading_tool_while_disabled_is_iserror() {
        let resp = handle(
            &public_server(false),
            json!({
                "jsonrpc": "2.0", "id": 5, "method": "tools/call",
                "params": { "name": "cancel_all_orders", "arguments": {} }
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("trading is disabled"));
    }

    #[tokio::test]
    async fn unknown_tool_is_iserror_not_protocol_error() {
        let resp = handle(
            &public_server(true),
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
        let server = public_server(false);
        let resp: Value =
            serde_json::from_str(&server.handle_line("{not json").await.unwrap()).unwrap();
        assert_eq!(resp["error"]["code"], PARSE_ERROR);
        assert!(resp["id"].is_null());
    }
}
