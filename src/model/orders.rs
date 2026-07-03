//! Order models: enums, the `Order` record, request bodies, and acknowledgements.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::common::{ClientOrderId, OrderId, Price, Quantity, Side, Symbol, Timestamp};

/// The type of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderType {
    /// Executed immediately at the current market price.
    Market,
    /// Executed only at or better than a specified price.
    Limit,
    /// Submitted once a trigger price is reached.
    Conditional,
    /// A take-profit / stop-loss configuration for a position.
    ///
    /// Read-only today: the exchange's own UI creates these, but `POST /orders`
    /// rejects every `tpsl` placement shape unparsed (probed in production
    /// 2026-07-02; see `docs/openapi-inventory.md` and `examples/tpsl_probe.rs`).
    Tpsl,
    /// A type this SDK version does not recognize (forward compatibility — the
    /// exchange may add order types without an API-version bump).
    #[serde(other)]
    Unknown,
}

impl OrderType {
    /// The on-the-wire token for this order type.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::Limit => "limit",
            Self::Conditional => "conditional",
            Self::Tpsl => "tpsl",
            Self::Unknown => "unknown",
        }
    }
}

/// The lifecycle status of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    /// Accepted by the matching engine but not yet working.
    PendingNew,
    /// A working order.
    New,
    /// Partially filled.
    PartiallyFilled,
    /// Fully filled.
    Filled,
    /// Cancelled.
    Cancelled,
    /// Rejected.
    Rejected,
    /// Replaced by another order.
    Replaced,
    /// A status this SDK version does not recognize (forward compatibility — the
    /// exchange may add lifecycle states without an API-version bump). Lets a
    /// page of orders deserialize instead of failing wholesale on one new value.
    #[serde(other)]
    Unknown,
}

impl OrderStatus {
    /// The on-the-wire token for this status.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PendingNew => "pending_new",
            Self::New => "new",
            Self::PartiallyFilled => "partially_filled",
            Self::Filled => "filled",
            Self::Cancelled => "cancelled",
            Self::Rejected => "rejected",
            Self::Replaced => "replaced",
            Self::Unknown => "unknown",
        }
    }
}

/// How long an order remains active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeInForce {
    /// Good till cancelled.
    Gtc,
    /// Immediate or cancel.
    Ioc,
    /// Fill or kill.
    Fok,
    /// A value this SDK version does not recognize (forward compatibility).
    #[serde(other)]
    Unknown,
}

impl TimeInForce {
    /// The on-the-wire token for this time in force.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gtc => "gtc",
            Self::Ioc => "ioc",
            Self::Fok => "fok",
            Self::Unknown => "unknown",
        }
    }
}

/// An execution instruction attached to an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionInstruction {
    /// Permit taker execution.
    AllowTaker,
    /// Only rest on the book (never take liquidity).
    PostOnly,
    /// A value this SDK version does not recognize (forward compatibility). Only
    /// produced when reading a response; never send it.
    #[serde(other)]
    Unknown,
}

/// The order type produced when a trigger fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerType {
    /// Trigger produces a market order.
    Market,
    /// Trigger produces a limit order.
    Limit,
    /// A value this SDK version does not recognize (forward compatibility) — so
    /// one exotic trigger does not fail deserialization of a whole orders page.
    #[serde(other)]
    Unknown,
}

impl TriggerType {
    /// The on-the-wire token for this trigger type.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::Limit => "limit",
            Self::Unknown => "unknown",
        }
    }
}

/// The price-comparison operator that activates a trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerDirection {
    /// Activate when the market price is greater than or equal to the trigger.
    Ge,
    /// Activate when the market price is less than or equal to the trigger.
    Le,
    /// A value this SDK version does not recognize (forward compatibility) — so
    /// one exotic trigger does not fail deserialization of a whole orders page.
    #[serde(other)]
    Unknown,
}

impl TriggerDirection {
    /// The on-the-wire token for this trigger direction.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ge => "ge",
            Self::Le => "le",
            Self::Unknown => "unknown",
        }
    }
}

/// Trigger conditions for conditional and take-profit / stop-loss orders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderTrigger {
    /// The price level that activates the order.
    #[serde(with = "rust_decimal::serde::str")]
    pub trigger_price: Decimal,
    /// The order type produced when the trigger fires.
    #[serde(rename = "type")]
    pub trigger_type: TriggerType,
    /// The comparison operator used to evaluate the trigger.
    pub trigger_direction: TriggerDirection,
    /// The limit price for the triggered order (limit triggers only).
    #[serde(
        with = "rust_decimal::serde::str_option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub limit_price: Option<Decimal>,
    /// Time in force for the triggered order.
    pub time_in_force: TimeInForce,
    /// Execution instructions for the triggered order.
    #[serde(default)]
    pub execution_instructions: Vec<ExecutionInstruction>,
}

/// A full order record as returned by the order endpoints.
///
/// Monetary and quantity fields use [`Decimal`] (decimal strings on the wire).
/// Optional fields are absent for order types or states that do not use them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    /// Exchange-assigned order id.
    pub id: OrderId,
    /// Id of the order this one replaced, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_order_id: Option<OrderId>,
    /// Client-assigned order id.
    pub client_order_id: ClientOrderId,
    /// The trading pair.
    pub symbol: Symbol,
    /// Buy or sell.
    pub side: Side,
    /// The order type.
    #[serde(rename = "type")]
    pub order_type: OrderType,
    /// Order quantity in the base currency.
    #[serde(with = "rust_decimal::serde::str")]
    pub quantity: Decimal,
    /// Quantity executed so far, in the base currency.
    #[serde(with = "rust_decimal::serde::str")]
    pub filled_quantity: Decimal,
    /// Quantity not yet executed, in the base currency.
    #[serde(with = "rust_decimal::serde::str")]
    pub leaves_quantity: Decimal,
    /// Order amount in the quote currency, when applicable.
    #[serde(
        with = "rust_decimal::serde::str_option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub amount: Option<Decimal>,
    /// Amount executed so far, in the quote currency, when applicable.
    #[serde(
        with = "rust_decimal::serde::str_option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub filled_amount: Option<Decimal>,
    /// The worst price at which the order will fill (limit price), when set.
    #[serde(
        with = "rust_decimal::serde::str_option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub price: Option<Decimal>,
    /// Quantity-weighted average fill price, when any fills have occurred.
    #[serde(
        with = "rust_decimal::serde::str_option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub average_fill_price: Option<Decimal>,
    /// The order status.
    pub status: OrderStatus,
    /// Reason for rejection, present only when `status` is `rejected`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject_reason: Option<String>,
    /// Time in force.
    pub time_in_force: TimeInForce,
    /// Execution instructions.
    #[serde(default)]
    pub execution_instructions: Vec<ExecutionInstruction>,
    /// Trigger details, present only for `conditional` orders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditional: Option<OrderTrigger>,
    /// Take-profit trigger, present for `tpsl` orders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_profit: Option<OrderTrigger>,
    /// Stop-loss trigger, present for `tpsl` orders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_loss: Option<OrderTrigger>,
    /// Total fee charged for the order (present on single-order lookups).
    #[serde(
        with = "rust_decimal::serde::str_option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub total_fee: Option<Decimal>,
    /// Currency the fee is charged in (present on single-order lookups).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_currency: Option<String>,
    /// Creation time.
    pub created_date: Timestamp,
    /// Last update time.
    pub updated_date: Timestamp,
}

/// Alias for [`Order`]; `GET /orders/{id}` returns an order with the optional
/// [`Order::total_fee`] and [`Order::fee_currency`] fields populated.
pub type OrderDetails = Order;

/// Acknowledgement returned when placing or replacing an order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderAck {
    /// Exchange-assigned order id.
    pub venue_order_id: OrderId,
    /// The client order id supplied with the request.
    pub client_order_id: ClientOrderId,
    /// Order state immediately after submission.
    pub state: OrderStatus,
}

/// Limit-order configuration. Exactly one of `base_size` / `quote_size` is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitOrderConfiguration {
    /// Amount of base currency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_size: Option<Quantity>,
    /// Amount of quote currency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_size: Option<Quantity>,
    /// The limit price.
    pub price: Price,
    /// Execution instructions for the order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_instructions: Option<Vec<ExecutionInstruction>>,
}

/// Market-order configuration. Exactly one of `base_size` / `quote_size` is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketOrderConfiguration {
    /// Amount of base currency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_size: Option<Quantity>,
    /// Amount of quote currency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_size: Option<Quantity>,
}

/// Order configuration. Exactly one of `limit` / `market` is set.
///
/// There is deliberately no `tpsl` configuration: `POST /orders` rejects every
/// take-profit / stop-loss placement shape unparsed (probed in production
/// 2026-07-02; `examples/tpsl_probe.rs` reproduces the sweep via
/// [`place_raw`](crate::api::orders::OrdersApi::place_raw)).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderConfiguration {
    /// Limit-order configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<LimitOrderConfiguration>,
    /// Market-order configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market: Option<MarketOrderConfiguration>,
}

/// Request body for `POST /orders`.
///
/// Prefer the builders on [`crate::api::orders::OrdersApi`] (for example
/// `client.orders().limit_buy(..)`) over constructing this directly; they
/// guarantee a valid configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderPlacementRequest {
    /// Client-generated id for idempotency.
    pub client_order_id: ClientOrderId,
    /// The trading pair.
    pub symbol: Symbol,
    /// Buy or sell.
    pub side: Side,
    /// The order configuration.
    pub order_configuration: OrderConfiguration,
}

/// Request body for `PUT /orders/{id}` (replace order).
///
/// Only the provided fields are changed; omitted fields are inherited from the
/// original order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OrderReplacementRequest {
    /// Client-generated id for idempotency of the replacement.
    pub client_order_id: ClientOrderId,
    /// New base-currency size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_size: Option<Quantity>,
    /// New quote-currency size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_size: Option<Quantity>,
    /// New limit price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<Price>,
    /// Replacement execution instructions (pass an empty vec to clear them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_instructions: Option<Vec<ExecutionInstruction>>,
}
