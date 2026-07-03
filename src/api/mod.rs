//! Endpoint groups.
//!
//! Each endpoint group should expose a small domain-oriented surface and defer
//! shared HTTP, auth, and error handling to the client/transport layer.

pub mod balances;
pub mod configuration;
pub mod market_data;
pub mod orders;
pub(crate) mod range;
pub mod trades;

/// Builds the query pairs for an array parameter in the exchange's
/// `form`/`explode=false` style: a single `key=v1,v2,v3` entry (the comma is
/// left literal by the transport's component encoder). An empty iterator yields
/// no pair at all. Shared by the endpoint groups so every array filter
/// serializes the same, spec-conforming way.
pub(crate) fn comma_joined<I, S>(key: &str, values: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let joined = values
        .into_iter()
        .map(|v| v.as_ref().to_owned())
        .collect::<Vec<_>>()
        .join(",");
    if joined.is_empty() {
        Vec::new()
    } else {
        vec![(key.to_owned(), joined)]
    }
}
