//! Minimal JSON-RPC 2.0 helpers for the MCP stdio transport.
//!
//! MCP over stdio frames each JSON-RPC message as a single line of JSON
//! (newline-delimited, no embedded newlines). `serde_json`'s compact output
//! escapes any newlines inside string values, so serialized messages are always
//! safe to emit followed by a single `\n`.

use serde_json::{Value, json};

pub const JSONRPC_VERSION: &str = "2.0";

// Standard JSON-RPC 2.0 error codes.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

/// Builds a JSON-RPC success response.
pub fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": JSONRPC_VERSION, "id": id, "result": result })
}

/// Builds a JSON-RPC error response.
pub fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "error": { "code": code, "message": message.into() }
    })
}
