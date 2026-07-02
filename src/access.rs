//! Access tiers — the capability ladder shared by the signing agent's
//! authoritative gate and the high-level command layer.
//!
//! A session (a running agent, or a direct CLI invocation) is configured with one
//! [`AccessLevel`]. Every operation requires a minimum level, and the session may
//! run it only when its level is at least that minimum. From least to most
//! privileged: [`Market`](AccessLevel::Market) (public market data only),
//! [`View`](AccessLevel::View) (adds read-only account data), and
//! [`Trading`](AccessLevel::Trading) (adds order placement and cancellation). The
//! tiers are cumulative — each grants everything the lower ones do.

/// The capability tier a session is authorized for. Ordered
/// `Market < View < Trading`, so comparisons (`>=`) express "at least this tier".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
#[cfg_attr(feature = "agent", derive(bincode::Encode, bincode::Decode))]
pub enum AccessLevel {
    /// Public market data and exchange reference data only: tickers, order books,
    /// candles, public trades, currencies, and pairs. No personal account data and
    /// no trading. The default — the least privilege.
    #[default]
    Market,
    /// Everything in [`Market`](Self::Market), plus read-only personal account
    /// data: balances, your own orders and trades, and order fills. Cannot place,
    /// replace, or cancel orders.
    View,
    /// Everything in [`View`](Self::View), plus placing, replacing, and cancelling
    /// orders (REAL TRADING).
    Trading,
}

impl AccessLevel {
    /// The flag value naming this tier (`market`, `view`, or `trading`), as used in
    /// `--access` and in messages.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::View => "view",
            Self::Trading => "trading",
        }
    }

    /// Whether a session at this level may run an operation that requires
    /// `required` — true when this level is at least `required`.
    #[must_use]
    pub fn permits(self, required: Self) -> bool {
        self >= required
    }
}

impl std::str::FromStr for AccessLevel {
    type Err = crate::error::Error;
    /// Parses a tier name (`market`, `view`, `trading`), case-insensitively.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "market" => Ok(Self::Market),
            "view" => Ok(Self::View),
            "trading" => Ok(Self::Trading),
            other => Err(crate::error::Error::invalid_request(format!(
                "invalid access tier '{other}' (expected one of: {ACCESS_LADDER})"
            ))),
        }
    }
}

/// The capability ladder, lowest to highest — for help text and messages.
pub const ACCESS_LADDER: &str = "market < view < trading";

/// The minimum [`AccessLevel`] an HTTP request requires, classified by method and
/// API path (e.g. `/balances`, `/orders/active`).
///
/// This is the signing agent's authoritative gate, so it is an **allowlist** that
/// fails closed: any non-`GET` request is an order mutation
/// ([`Trading`](AccessLevel::Trading)); a `GET` is [`Market`](AccessLevel::Market)
/// only when its path is a known public market-data / reference endpoint, and is
/// otherwise treated as account data ([`View`](AccessLevel::View)). That way a
/// future account endpoint nobody classified is never exposed at the market tier —
/// at worst a public one is over-restricted until it is added here.
#[must_use]
pub fn required_access_for(method: &str, path: &str) -> AccessLevel {
    if !method.eq_ignore_ascii_case("GET") {
        return AccessLevel::Trading;
    }
    let p = path.trim_start_matches('/');
    let is_public_market = p == "tickers"
        || p.starts_with("order-book/")
        || p.starts_with("public/")
        || p.starts_with("candles/")
        || p.starts_with("trades/all/")
        || p.starts_with("configuration/");
    if is_public_market {
        AccessLevel::Market
    } else {
        AccessLevel::View
    }
}

/// A uniform refusal for an operation blocked by the access gate.
///
/// It states the tier the operation needs, the tier in effect, and the `--access`
/// option that would grant it. Used by both the agent (the authoritative gate) and
/// the CLI (which gates locally so an agent policy can be rehearsed).
#[must_use]
pub fn access_denied(required: AccessLevel, current: AccessLevel) -> String {
    format!(
        "access denied: this operation needs `--access {req}`, but the session is running with \
         `--access {cur}`. (Re)start with `--access {req}` (or higher; the tiers are {ACCESS_LADDER}) \
         to allow it.",
        req = required.as_str(),
        cur = current.as_str(),
    )
}

#[cfg(test)]
mod tests {
    use super::{AccessLevel, access_denied, required_access_for};

    #[test]
    fn tiers_are_ordered_and_cumulative() {
        assert!(AccessLevel::Market < AccessLevel::View);
        assert!(AccessLevel::View < AccessLevel::Trading);
        // A higher tier permits everything a lower one requires.
        assert!(AccessLevel::Trading.permits(AccessLevel::Market));
        assert!(AccessLevel::View.permits(AccessLevel::Market));
        assert!(AccessLevel::View.permits(AccessLevel::View));
        // A lower tier does not reach a higher requirement.
        assert!(!AccessLevel::Market.permits(AccessLevel::View));
        assert!(!AccessLevel::View.permits(AccessLevel::Trading));
        // The default is the least privilege.
        assert_eq!(AccessLevel::default(), AccessLevel::Market);
    }

    #[test]
    fn classifies_known_endpoints() {
        let m = |p: &str| required_access_for("GET", p);
        // Public market data + reference → market.
        assert_eq!(m("/tickers"), AccessLevel::Market);
        assert_eq!(m("/order-book/BTC-USD"), AccessLevel::Market);
        assert_eq!(m("/public/last-trades"), AccessLevel::Market);
        assert_eq!(m("/candles/BTC-USD"), AccessLevel::Market);
        assert_eq!(m("/trades/all/BTC-USD"), AccessLevel::Market);
        assert_eq!(m("/configuration/pairs"), AccessLevel::Market);
        // Personal account reads → view.
        assert_eq!(m("/balances"), AccessLevel::View);
        assert_eq!(m("/orders/active"), AccessLevel::View);
        assert_eq!(m("/orders/fills/abc"), AccessLevel::View);
        assert_eq!(m("/trades/private/BTC-USD"), AccessLevel::View);
        // Any mutation → trading.
        assert_eq!(required_access_for("POST", "/orders"), AccessLevel::Trading);
        assert_eq!(
            required_access_for("DELETE", "/orders/abc"),
            AccessLevel::Trading
        );
    }

    #[test]
    fn unknown_get_endpoint_fails_closed_to_view() {
        // An endpoint not on the public allowlist is treated as account data, so a
        // future unclassified read is never leaked at the market tier.
        assert_eq!(
            required_access_for("GET", "/some/new/thing"),
            AccessLevel::View
        );
    }

    #[test]
    fn denial_names_the_required_option() {
        let msg = access_denied(AccessLevel::View, AccessLevel::Market);
        assert!(msg.contains("--access view"));
        assert!(msg.contains("--access market"));
    }
}
