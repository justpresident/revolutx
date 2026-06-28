//! The structured result of a command.

use serde::Serialize;

use crate::OrderId;
use crate::Page;
use crate::model::balances::Balance;
use crate::model::configuration::{Currencies, CurrencyPairs};
use crate::model::market_data::{Candle, LastTrades, OrderBook, Tickers, Trade};
use crate::model::orders::{Order, OrderAck, OrderDetails};
use crate::model::trades::Fill;

/// The result of running a [`Command`](super::Command).
///
/// `#[serde(untagged)]` makes it serialize to **exactly** the inner value (no
/// variant wrapper), so a surface's JSON output is byte-identical to calling the
/// SDK endpoint directly; the Rust variant tag is what a human presenter matches
/// on. It is only ever serialized, never deserialized, so untagged is
/// unambiguous.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum CommandOutput {
    Balances(Vec<Balance>),
    Currencies(Currencies),
    Pairs(CurrencyPairs),
    Tickers(Tickers),
    // The single-record structs are boxed to keep the enum small (they dwarf the
    // ack variants); `Box<T>` serializes transparently, so JSON is unchanged.
    OrderBook(Box<OrderBook>),
    Candles(Vec<Candle>),
    LastTrades(LastTrades),
    AllTrades(Page<Trade>),
    PrivateTrades(Page<Fill>),
    Orders(Page<Order>),
    Order(Box<OrderDetails>),
    Fills(Vec<Fill>),
    Ack(Box<OrderAck>),
    Cancelled(CancelAck),
    AllCancelled(CancelAllAck),
}

/// Acknowledges a cancelled order. Serializes as `{"status":"cancelled","order_id":"…"}`.
#[derive(Debug, Serialize)]
pub struct CancelAck {
    pub status: &'static str,
    pub order_id: String,
}

impl CancelAck {
    pub(crate) fn new(id: &OrderId) -> Self {
        Self {
            status: "cancelled",
            order_id: id.as_str().to_owned(),
        }
    }
}

/// Acknowledges a cancel-all. Serializes as `{"status":"all_cancelled"}`.
#[derive(Debug, Serialize)]
pub struct CancelAllAck {
    pub status: &'static str,
}

impl CancelAllAck {
    pub(crate) const fn new() -> Self {
        Self {
            status: "all_cancelled",
        }
    }
}
