//! Public domain models.
//!
//! Models represent trading concepts and API responses in an idiomatic Rust
//! shape. They are handwritten rather than generated, and validated against the
//! `OpenAPI` examples by the fixture tests.

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
/// This is the flat domain shape: it serializes and deserializes to/from its own
/// fields (it round-trips). The API's wire envelope is the separate [`RawPage`],
/// converted here explicitly at the endpoint boundary.
///
/// Pass [`Page::next_cursor`] back into the originating query's `cursor` field
/// to fetch the following page. An empty cursor from the server is normalized
/// to `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    /// The items on this page.
    pub items: Vec<T>,
    /// Cursor for the next page, or `None` when there are no more results.
    pub next_cursor: Option<String>,
    /// Server timestamp for the page.
    pub timestamp: Timestamp,
}

/// The API's raw paginated envelope — `{ "data": [...], "metadata": { ... } }`.
///
/// Deserialized at the endpoint boundary and converted into the flat [`Page`]
/// via [`From`]. Kept separate so [`Page`] stays a clean, round-trippable domain
/// type and the wire shape is explicit rather than hidden in a serde attribute.
#[cfg(feature = "rest")]
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RawPage<T> {
    data: Vec<T>,
    metadata: RawPageMeta,
}

#[cfg(feature = "rest")]
#[derive(Debug, Clone, serde::Deserialize)]
struct RawPageMeta {
    timestamp: Timestamp,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[cfg(feature = "rest")]
impl<T> From<RawPage<T>> for Page<T> {
    fn from(raw: RawPage<T>) -> Self {
        Self {
            items: raw.data,
            next_cursor: raw.metadata.next_cursor.filter(|c| !c.is_empty()),
            timestamp: raw.metadata.timestamp,
        }
    }
}

/// Internal helper for the common `{ "data": T }` response envelope.
#[cfg(feature = "rest")]
#[derive(Deserialize)]
pub(crate) struct Data<T> {
    pub data: T,
}

/// Internal helper for fields the spec types as an object but whose examples
/// sometimes wrap in a single-element array (the order placement/replacement
/// `data` field). Accepts either shape.
#[cfg(feature = "rest")]
#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

#[cfg(feature = "rest")]
impl<T> OneOrMany<T> {
    pub(crate) fn into_first(self) -> Option<T> {
        match self {
            Self::One(value) => Some(value),
            Self::Many(values) => values.into_iter().next(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn page_round_trips_through_its_own_serde() {
        let page = Page {
            items: vec![1_u32, 2, 3],
            next_cursor: Some("cursor".to_owned()),
            timestamp: Timestamp::from_unix_millis(1_700_000_000_000),
        };
        let json = serde_json::to_string(&page).unwrap();
        let back: Page<u32> = serde_json::from_str(&json).unwrap();
        assert_eq!(page, back);
    }

    #[cfg(feature = "rest")]
    #[test]
    fn raw_page_envelope_converts_and_normalizes_empty_cursor() {
        let envelope =
            r#"{"data":[10,20],"metadata":{"timestamp":1700000000000,"next_cursor":""}}"#;
        let raw: RawPage<u32> = serde_json::from_str(envelope).unwrap();
        let page: Page<u32> = raw.into();
        assert_eq!(page.items, [10, 20]);
        assert_eq!(page.next_cursor, None, "empty cursor normalizes to None");
    }
}
