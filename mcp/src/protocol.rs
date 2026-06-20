//! Minimal JSON-RPC 2.0 helpers for the MCP stdio transport.
//!
//! MCP over stdio frames each JSON-RPC message as a single line of JSON
//! (newline-delimited, no embedded newlines). `serde_json`'s compact output
//! escapes any newlines inside string values, so serialized messages are always
//! safe to emit followed by a single `\n`.

use serde_json::{Map, Value};

pub const JSONRPC_VERSION: &str = "2.0";

// Standard JSON-RPC 2.0 error codes.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

/// Builds a JSON-RPC success response. Takes `id`/`result` by value because they
/// are moved straight into the response object (no clone).
pub fn success(id: Value, result: Value) -> Value {
    let mut map = Map::with_capacity(3);
    map.insert("jsonrpc".to_owned(), JSONRPC_VERSION.into());
    map.insert("id".to_owned(), id);
    map.insert("result".to_owned(), result);
    Value::Object(map)
}

/// Builds a JSON-RPC error response. `id` is moved into the response object.
pub fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    let mut error = Map::with_capacity(2);
    error.insert("code".to_owned(), code.into());
    error.insert("message".to_owned(), Value::String(message.into()));

    let mut map = Map::with_capacity(3);
    map.insert("jsonrpc".to_owned(), JSONRPC_VERSION.into());
    map.insert("id".to_owned(), id);
    map.insert("error".to_owned(), Value::Object(error));
    Value::Object(map)
}
