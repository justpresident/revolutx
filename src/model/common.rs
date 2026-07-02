//! Shared domain primitives used across the SDK.
//!
//! These types give the public API trading-domain meaning instead of bare
//! strings and floats. Monetary and quantity values are represented with
//! [`rust_decimal::Decimal`] (re-exported at the crate root as
//! [`crate::Decimal`]) so prices, sizes, balances, and fees are never subject
//! to binary floating-point rounding.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use rust_decimal::Decimal;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::error::{Error, Result};

/// A trading pair or asset symbol, e.g. `BTC-USD`, `BTC/USD`, or `BTC`.
///
/// The exchange uses the dash form (`BTC-USD`) in request paths and order
/// placement, and the slash form (`BTC/USD`) in some responses (and the
/// [`CurrencyPairs`](crate::model::configuration::CurrencyPairs) map key).
/// [`Symbol`] is a validated, non-empty wrapper;
/// [`from_pair`](Self::from_pair) builds one from a pair's base and quote, and
/// [`dashed`](Self::dashed) / [`slashed`](Self::slashed) convert between the two
/// forms so callers never hand-roll the separator swap.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Symbol(String);

impl Symbol {
    /// Creates a symbol, rejecting empty or whitespace-only input.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(Error::invalid_request("symbol must not be empty"));
        }
        Ok(Self(value))
    }

    /// Builds a symbol from a pair's `base` and `quote` assets in the dash form the
    /// exchange uses in request paths — `Symbol::from_pair("BTC", "USD")` is
    /// `BTC-USD`. The convenient way to turn a
    /// [`CurrencyPair`](crate::model::configuration::CurrencyPair) into the
    /// path/placement symbol.
    pub fn from_pair(base: &str, quote: &str) -> Self {
        Self(format!("{base}-{quote}"))
    }

    /// Returns the symbol as a string slice (in whatever form it was created with).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// This symbol in the **dash** form (`BTC-USD`) the exchange uses in request
    /// paths and order placement. Borrows when already dashed; converts a slash
    /// otherwise.
    pub fn dashed(&self) -> Cow<'_, str> {
        if self.0.contains('/') {
            Cow::Owned(self.0.replace('/', "-"))
        } else {
            Cow::Borrowed(&self.0)
        }
    }

    /// This symbol in the **slash** form (`BTC/USD`) the exchange uses in some
    /// responses and that reads naturally for humans. Borrows when already
    /// slashed; converts a dash otherwise.
    pub fn slashed(&self) -> Cow<'_, str> {
        if self.0.contains('-') {
            Cow::Owned(self.0.replace('-', "/"))
        } else {
            Cow::Borrowed(&self.0)
        }
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad` (not `write_str`) so width/fill/alignment like `{symbol:<12}` work.
        f.pad(&self.0)
    }
}

/// Returns `symbol` in the dash form (`BTC-USD`) the exchange uses in request
/// paths and array query filters, converting the slash form (`BTC/USD`) — the
/// shape some responses and the pairs-map key use — without allocating when it
/// is already dashed. The endpoint layer applies this so a symbol taken straight
/// from `configuration().pairs()` reaches the wire in the accepted form.
///
/// `rest`-only: it exists for the REST endpoint layer (the models compile without
/// `rest`, where nothing calls it).
#[cfg(feature = "rest")]
pub(crate) fn dash_symbol(symbol: &str) -> Cow<'_, str> {
    if symbol.contains('/') {
        Cow::Owned(symbol.replace('/', "-"))
    } else {
        Cow::Borrowed(symbol)
    }
}

impl FromStr for Symbol {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::new(s)
    }
}

impl TryFrom<&str> for Symbol {
    type Error = Error;
    fn try_from(value: &str) -> Result<Self> {
        Self::new(value)
    }
}

impl TryFrom<String> for Symbol {
    type Error = Error;
    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for Symbol {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(de::Error::custom)
    }
}

/// The side of an order or trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// Buy (acquire the base asset).
    Buy,
    /// Sell (dispose of the base asset).
    Sell,
}

impl Side {
    /// The on-the-wire token for this side (`buy` or `sell`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }
}

impl FromStr for Side {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "buy" => Ok(Self::Buy),
            "sell" => Ok(Self::Sell),
            other => Err(Error::invalid_request(format!(
                "invalid side '{other}' (expected 'buy' or 'sell')"
            ))),
        }
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad` (not `write_str`) so table widths like `{:<5}` align buy/sell rows.
        f.pad(self.as_str())
    }
}

/// An order identifier assigned by the exchange (the `venue_order_id`).
///
/// The API documents these as UUIDs, but [`OrderId`] is a forgiving string
/// wrapper so the SDK keeps working if the server ever returns another format.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OrderId(String);

impl OrderId {
    /// Wraps a raw order identifier.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad` (not `write_str`) so column widths apply when ids are tabulated.
        f.pad(&self.0)
    }
}

impl FromStr for OrderId {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(Self(s.to_owned()))
    }
}

impl From<&str> for OrderId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for OrderId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// A client-generated order identifier used for idempotency.
///
/// The exchange requires this to be a UUID, so [`ClientOrderId`] wraps
/// [`uuid::Uuid`] and serializes as its hyphenated string form. A fresh random
/// (v4) id can be created with [`ClientOrderId::random`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientOrderId(Uuid);

impl ClientOrderId {
    /// Generates a new random (v4) client order id.
    pub fn random() -> Self {
        Self(Uuid::new_v4())
    }

    /// Wraps an existing UUID.
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the underlying UUID.
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for ClientOrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad` (not `write!`) so column widths apply when ids are tabulated.
        f.pad(&self.0.to_string())
    }
}

impl From<Uuid> for ClientOrderId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl Default for ClientOrderId {
    /// Produces a fresh random (v4) client order id, suitable as a unique
    /// idempotency key.
    fn default() -> Self {
        Self::random()
    }
}

impl FromStr for ClientOrderId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Uuid::parse_str(s)
            .map(Self)
            .map_err(|e| Error::invalid_request(format!("invalid client order id: {e}")))
    }
}

impl Serialize for ClientOrderId {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ClientOrderId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(de::Error::custom)
    }
}

/// A strictly positive limit price, used when constructing orders.
///
/// Serializes as a decimal string to match the exchange wire format. Response
/// models expose raw [`Decimal`] values instead, since those are observations
/// from the exchange rather than client-authored inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Price(Decimal);

/// A strictly positive order size or amount, used when constructing orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Quantity(Decimal);

macro_rules! positive_decimal {
    ($name:ident, $label:literal) => {
        impl $name {
            #[doc = concat!("Creates a ", $label, ", rejecting zero or negative values.")]
            pub fn new(value: impl Into<Decimal>) -> Result<Self> {
                let value = value.into();
                if value <= Decimal::ZERO {
                    return Err(Error::invalid_request(concat!($label, " must be positive")));
                }
                Ok(Self(value))
            }

            #[doc = "Returns the underlying decimal value."]
            pub const fn value(&self) -> Decimal {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl TryFrom<Decimal> for $name {
            type Error = Error;
            fn try_from(value: Decimal) -> Result<Self> {
                Self::new(value)
            }
        }

        impl FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self> {
                let value = Decimal::from_str(s).map_err(|e| {
                    Error::invalid_request(format!(concat!("invalid ", $label, ": {}"), e))
                })?;
                Self::new(value)
            }
        }

        impl From<$name> for Decimal {
            fn from(value: $name) -> Decimal {
                value.0
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.collect_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                let raw = String::deserialize(deserializer)?;
                raw.parse().map_err(de::Error::custom)
            }
        }
    };
}

positive_decimal!(Price, "price");
positive_decimal!(Quantity, "quantity");

/// A point in time reported by the exchange.
///
/// The Revolut X API returns timestamps in two shapes: Unix epoch milliseconds
/// (integers) on authenticated endpoints, and ISO-8601 / RFC 3339 strings on
/// the public endpoints. [`Timestamp`] deserializes from either form into a
/// single [`time::OffsetDateTime`], so callers get one consistent instant type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(OffsetDateTime);

impl Timestamp {
    /// Builds a timestamp from Unix epoch milliseconds.
    pub fn from_unix_millis(millis: i64) -> Self {
        let nanos = i128::from(millis) * 1_000_000;
        Self(OffsetDateTime::from_unix_timestamp_nanos(nanos).unwrap_or(OffsetDateTime::UNIX_EPOCH))
    }

    /// Returns the instant as Unix epoch milliseconds.
    // Epoch milliseconds for any representable `OffsetDateTime` (max year 9999)
    // fit in `i64`; the i128 -> i64 narrowing cannot lose information here, and a
    // `const fn` cannot use `i64::try_from`.
    #[allow(clippy::cast_possible_truncation)]
    pub const fn unix_millis(&self) -> i64 {
        (self.0.unix_timestamp_nanos() / 1_000_000) as i64
    }

    /// Returns the underlying [`time::OffsetDateTime`] (always in UTC).
    pub const fn datetime(&self) -> OffsetDateTime {
        self.0
    }
}

impl From<OffsetDateTime> for Timestamp {
    fn from(value: OffsetDateTime) -> Self {
        Self(value)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad` (not `write_str`) so column widths apply when timestamps are tabulated.
        match self.0.format(&Rfc3339) {
            Ok(s) => f.pad(&s),
            Err(_) => write!(f, "{}", self.unix_millis()),
        }
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let formatted = self.0.format(&Rfc3339).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&formatted)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct TimestampVisitor;

        impl Visitor<'_> for TimestampVisitor {
            type Value = Timestamp;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an epoch-millisecond integer or an RFC 3339 timestamp string")
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Timestamp, E> {
                Ok(Timestamp::from_unix_millis(v))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Timestamp, E> {
                Ok(Timestamp::from_unix_millis(
                    i64::try_from(v).unwrap_or(i64::MAX),
                ))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Timestamp, E> {
                OffsetDateTime::parse(v, &Rfc3339)
                    .map(Timestamp)
                    .map_err(|e| de::Error::custom(format!("invalid RFC 3339 timestamp: {e}")))
            }
        }

        deserializer.deserialize_any(TimestampVisitor)
    }
}

/// Serde adapter for integer values the API encodes as JSON strings (e.g. the
/// order-book `no` count). Serializes back to a string for round-tripping.
pub(crate) mod string_u64 {
    use serde::{Deserialize, Deserializer, Serializer};

    // The `&u64` signature is mandated by serde's `serialize_with` contract.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.trim().parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn symbol_rejects_empty() {
        assert!(Symbol::new("").is_err());
        assert!(Symbol::new("   ").is_err());
        assert_eq!(Symbol::new("BTC-USD").unwrap().as_str(), "BTC-USD");
    }

    #[test]
    fn symbol_normalizes_between_dash_and_slash_forms() {
        assert_eq!(Symbol::from_pair("BTC", "USD").as_str(), "BTC-USD");

        let dashed = Symbol::new("BTC-USD").unwrap();
        assert_eq!(dashed.dashed(), "BTC-USD"); // already dashed
        assert_eq!(dashed.slashed(), "BTC/USD"); // converted

        let slashed = Symbol::new("BTC/USD").unwrap();
        assert_eq!(slashed.dashed(), "BTC-USD"); // converted
        assert_eq!(slashed.slashed(), "BTC/USD"); // already slashed

        // Display honors padding/alignment (used by the CLI tables).
        assert_eq!(format!("{dashed:<8}|"), "BTC-USD |");
    }

    #[test]
    fn price_and_quantity_reject_non_positive() {
        assert!(Price::from_str("0").is_err());
        assert!(Price::from_str("-1").is_err());
        assert!(Quantity::from_str("0").is_err());
        assert_eq!(Price::from_str("50000.50").unwrap().to_string(), "50000.50");
    }

    #[test]
    fn decimal_values_round_trip_without_float() {
        // Trailing zeros and scale are preserved exactly (no float conversion).
        let q = Quantity::from_str("1000.00000000").unwrap();
        assert_eq!(q.to_string(), "1000.00000000");
        let json = serde_json::to_string(&q).unwrap();
        assert_eq!(json, "\"1000.00000000\"");
    }

    #[test]
    fn client_order_id_round_trips_as_string() {
        let id: ClientOrderId = "3fa85f64-5717-4562-b3fc-2c963f66afa6".parse().unwrap();
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            "\"3fa85f64-5717-4562-b3fc-2c963f66afa6\""
        );
        assert!("not-a-uuid".parse::<ClientOrderId>().is_err());
    }

    #[test]
    fn timestamp_accepts_millis_and_rfc3339() {
        let from_millis: Timestamp = serde_json::from_str("3318215482991").unwrap();
        assert_eq!(from_millis.unix_millis(), 3_318_215_482_991);

        let from_iso: Timestamp = serde_json::from_str("\"2025-08-08T21:40:35.133962Z\"").unwrap();
        assert_eq!(from_iso.datetime().year(), 2025);

        // Round-trip an epoch-millis value back out as RFC 3339.
        assert!(
            serde_json::to_string(&from_millis)
                .unwrap()
                .contains("2075")
        );
    }

    #[test]
    fn string_u64_parses_quoted_integer() {
        #[derive(serde::Deserialize)]
        struct Wrap {
            #[serde(with = "string_u64")]
            n: u64,
        }
        let w: Wrap = serde_json::from_str(r#"{"n":"3"}"#).unwrap();
        assert_eq!(w.n, 3);
    }
}
