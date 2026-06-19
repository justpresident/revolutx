//! Public domain models.
//!
//! Models represent trading concepts and API responses in an idiomatic Rust
//! shape. They are handwritten rather than generated, and validated against the
//! OpenAPI examples by the fixture tests.

pub mod balances;
pub mod common;
pub mod configuration;
pub mod market_data;
pub mod orders;
pub mod trades;

use serde::{Deserialize, Serialize};

use common::Timestamp;

/// A single page of a cursor-paginated list (active/historical orders, public
/// and private trades).
///
/// Pass [`Page::next_cursor`] back into the originating query's `cursor` field
/// to fetch the following page. An empty cursor from the server is normalized
/// to `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawPage<T>")]
pub struct Page<T> {
    /// The items on this page.
    pub items: Vec<T>,
    /// Cursor for the next page, or `None` when there are no more results.
    pub next_cursor: Option<String>,
    /// Server timestamp for the page.
    pub timestamp: Timestamp,
}

#[derive(Deserialize)]
struct RawPage<T> {
    data: Vec<T>,
    metadata: RawPageMeta,
}

#[derive(Deserialize)]
struct RawPageMeta {
    timestamp: Timestamp,
    #[serde(default)]
    next_cursor: Option<String>,
}

impl<T> From<RawPage<T>> for Page<T> {
    fn from(raw: RawPage<T>) -> Self {
        let next_cursor = raw.metadata.next_cursor.filter(|c| !c.is_empty());
        Page {
            items: raw.data,
            next_cursor,
            timestamp: raw.metadata.timestamp,
        }
    }
}

/// Internal helper for the common `{ "data": T }` response envelope.
#[derive(Deserialize)]
pub(crate) struct Data<T> {
    pub data: T,
}

/// Internal helper for fields the spec types as an object but whose examples
/// sometimes wrap in a single-element array (the order placement/replacement
/// `data` field). Accepts either shape.
#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    pub(crate) fn into_first(self) -> Option<T> {
        match self {
            OneOrMany::One(value) => Some(value),
            OneOrMany::Many(values) => values.into_iter().next(),
        }
    }
}
