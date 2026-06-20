//! FIX 4.4 client for Revolut X (market data and trading).
//!
//! **Status: not yet implemented.** This module is a placeholder so the `fix`
//! feature and module layout exist ahead of the FIX work. See the `fix` backlog
//! task for the plan.
//!
//! Revolut X exposes a FIX 4.4 gateway (separate from the REST API) with a
//! Market Data FIX API and a Trading FIX API; market-data subscriptions use the
//! standard `V` (Market Data Request) / `W` (Snapshot/Full Refresh) /
//! `X` (Incremental Refresh) messages. Access is typically gated to verified
//! market makers.
//!
//! When implemented, this module will provide a session-based client
//! (logon/logout, sequence numbers, heartbeats) that reuses the shared domain
//! models from [`crate::model`], so callers can opt into FIX with
//! `default-features = false, features = ["fix"]` and avoid the REST / `reqwest`
//! dependency tree entirely.
