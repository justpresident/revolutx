//! Command-line argument definitions.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// `revolutx` — a command-line interface for the Revolut X crypto exchange.
#[derive(Parser)]
#[command(name = "revolutx", version, about)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalOpts,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args)]
pub struct GlobalOpts {
    /// Print raw JSON instead of human-readable output.
    #[arg(long, global = true)]
    pub json: bool,
    /// Target environment.
    #[arg(long, global = true, value_enum, default_value_t = EnvArg::Production)]
    pub env: EnvArg,
    /// Path to the encrypted vault (default: `~/.revolutx/vault`).
    #[arg(long, global = true)]
    pub vault: Option<PathBuf>,
    /// Use plaintext credentials from the REVOLUTX_* environment variables
    /// instead of the encrypted vault (insecure; for dev/CI).
    #[arg(long, global = true)]
    pub insecure_env: bool,
    /// Disable the anti-debugger / ptrace hardening (for CI or legitimately
    /// traced hosts).
    #[arg(long, global = true)]
    pub insecure_allow_debugging: bool,
}

#[derive(Copy, Clone, ValueEnum)]
pub enum EnvArg {
    Production,
    Dev,
}

/// The `--access` capability tiers, lowest to highest. Each tier includes every
/// operation the ones before it allow.
#[derive(Copy, Clone, ValueEnum)]
pub enum AccessArg {
    /// Public market data and exchange reference data only: tickers, order books,
    /// candles, public trades, currencies, and pairs. No personal account data and
    /// no trading.
    Market,
    /// Market data plus read-only account data: balances, your own orders and
    /// trades, and order fills. Cannot place, replace, or cancel orders.
    View,
    /// Everything in `view` plus placing, replacing, and cancelling orders (REAL
    /// TRADING; still requires per-command --yes/confirmation).
    Trading,
}

impl From<AccessArg> for revolutx::AccessLevel {
    fn from(arg: AccessArg) -> Self {
        match arg {
            AccessArg::Market => Self::Market,
            AccessArg::View => Self::View,
            AccessArg::Trading => Self::Trading,
        }
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// Manage the encrypted credential vault.
    Vault {
        #[command(subcommand)]
        command: VaultCmd,
    },
    /// Show account balances.
    Balances,
    /// Exchange configuration (currencies, pairs).
    Config {
        #[command(subcommand)]
        command: ConfigCmd,
    },
    /// Market data.
    Market {
        #[command(subcommand)]
        command: MarketCmd,
    },
    /// Orders.
    Orders {
        #[command(subcommand)]
        command: OrderCmd,
    },
    /// Trade history.
    Trades {
        #[command(subcommand)]
        command: TradeCmd,
    },
    /// Run the signing agent: a daemon that unlocks the vault once and
    /// signs/sends on behalf of a single headless client.
    Agent {
        #[command(subcommand)]
        command: AgentCmd,
    },
    /// Open the vault once and enter an interactive shell that runs the same
    /// commands (with history and command/symbol autocomplete).
    Cli {
        /// Capability tier the shell may use (cumulative; default: view).
        ///
        /// `market` allows only public market data and exchange reference data;
        /// `view` adds read-only account data (balances, your orders and trades,
        /// fills); `trading` adds placing, replacing, and cancelling orders. This
        /// gates the shell locally so you can rehearse the policy an agent would
        /// enforce; order placement still needs per-command confirmation.
        #[arg(long, value_enum, default_value_t = AccessArg::View)]
        access: AccessArg,
    },
}

impl Command {
    /// Whether the command needs credentials (and thus runtime hardening).
    pub const fn needs_secrets(&self) -> bool {
        match self {
            Self::Market { command } => !matches!(
                command,
                MarketCmd::PublicOrderBook { .. } | MarketCmd::LastTrades
            ),
            _ => true,
        }
    }
}

#[derive(Subcommand)]
pub enum AgentCmd {
    /// Unlock the vault and serve connections over a unix socket, authorizing each
    /// interactively from the console (or via a one-time token), until stopped
    /// (Ctrl-C or `quit`). Runs in the foreground with an operator console.
    Start {
        /// Unix socket path (default: $`XDG_RUNTIME_DIR/revolutx-agent.sock`).
        /// Put it in a directory other UIDs can reach (e.g. a shared project dir)
        /// to allow cross-UID clients; the socket itself is world-connectable and
        /// nothing is served without a token or an operator grant.
        #[arg(long)]
        socket: Option<PathBuf>,
        /// Also accept a one-time token as an auth method: generate one, print it,
        /// and let a headless client (e.g. the MCP) present it to authorize at the
        /// access ceiling. It is single-use. Without this flag, connections are
        /// authorized only interactively from the console.
        #[arg(long)]
        auth_token: bool,
        /// Auto-lock (exit) after this many seconds with no *authorized* client
        /// connected (a merely-pending connection does not count). 0 disables it.
        #[arg(long, default_value_t = 3600)]
        idle_timeout: u64,
        /// The capability ceiling: the highest tier grantable to any connection
        /// (and the tier a token grants). The agent enforces it per connection; a
        /// grant cannot exceed it (cumulative; default: market — least privilege).
        ///
        /// `market` permits only public market data and exchange reference data;
        /// `view` adds read-only account data (balances, orders, trades, fills);
        /// `trading` adds placing, replacing, and cancelling orders (REAL TRADING).
        #[arg(long, value_enum, default_value_t = AccessArg::Market)]
        access: AccessArg,
    },
}

#[derive(Subcommand)]
pub enum VaultCmd {
    /// Initialize the encrypted vault (one-time setup).
    ///
    /// Prompts for a master password (and, when built with the `fido2` feature,
    /// offers to enrol a security key), generates an Ed25519 key pair (the private
    /// key is stored only in the vault), prints the public key with instructions
    /// to create your Revolut X API key, then stores that API key in the vault.
    Init {
        /// Import an existing Ed25519 private key PEM instead of generating one
        /// (e.g. from `openssl genpkey -algorithm ed25519 -out private.pem`).
        #[arg(long)]
        key_file: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum ConfigCmd {
    /// Supported currencies.
    Currencies,
    /// Supported trading pairs.
    Pairs,
}

#[derive(Subcommand)]
pub enum MarketCmd {
    /// Order book snapshot (authenticated).
    OrderBook {
        symbol: String,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Order book snapshot from the public endpoint (no credentials).
    PublicOrderBook { symbol: String },
    /// Tickers for all pairs, or the given ones.
    Tickers { symbols: Vec<String> },
    /// Historical OHLCV candles.
    Candles {
        symbol: String,
        /// Interval in minutes (1,5,15,30,60,240,1440,2880,5760,10080,20160,40320).
        #[arg(long)]
        interval: Option<i64>,
        /// Start time: a date/time (`2024-01-31`), a relative (`7d`), or epoch ms.
        #[arg(long, value_parser = crate::datetime::parse_when)]
        since: Option<i64>,
        /// End time (same formats as `--since`).
        #[arg(long, value_parser = crate::datetime::parse_when)]
        until: Option<i64>,
    },
    /// Latest public trades (no credentials).
    LastTrades,
    /// Stream an order book by polling.
    Watch {
        symbol: String,
        /// Poll interval in seconds.
        #[arg(long, default_value_t = 2)]
        interval: u64,
    },
}

#[derive(Subcommand)]
pub enum OrderCmd {
    /// List active orders.
    Active {
        #[arg(long)]
        symbol: Vec<String>,
        /// Restrict to one side.
        #[arg(long, value_enum)]
        side: Option<SideArg>,
        /// Pagination cursor from a previous page.
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// List historical orders.
    Historical {
        #[arg(long)]
        symbol: Vec<String>,
        /// Start of the range: a date/time (`2024-01-31`, `"2024-01-31 14:30"`), a
        /// relative offset (`7d`, `24h`), an RFC3339 timestamp, or epoch ms.
        #[arg(long, value_parser = crate::datetime::parse_when)]
        start_date: Option<i64>,
        /// End of the range (same formats as `--start-date`; defaults to now).
        #[arg(long, value_parser = crate::datetime::parse_when)]
        end_date: Option<i64>,
        /// Pagination cursor from a previous page.
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Get an order by id.
    Get { id: String },
    /// Get an order's fills.
    Fills { id: String },
    /// Place a limit order. REAL TRADING — requires --yes.
    Limit {
        #[arg(value_enum)]
        side: SideArg,
        symbol: String,
        size: String,
        price: String,
        /// Interpret `size` as the quote currency amount.
        #[arg(long)]
        quote: bool,
        /// Reject if the order would take liquidity.
        #[arg(long)]
        post_only: bool,
        /// Optional UUID idempotency key (generated if omitted).
        #[arg(long)]
        client_order_id: Option<String>,
        /// Confirm the (real) trade.
        #[arg(long)]
        yes: bool,
    },
    /// Place a market order. REAL TRADING — requires --yes.
    Market {
        #[arg(value_enum)]
        side: SideArg,
        symbol: String,
        size: String,
        /// Interpret `size` as the quote currency amount.
        #[arg(long)]
        quote: bool,
        /// Optional UUID idempotency key (generated if omitted).
        #[arg(long)]
        client_order_id: Option<String>,
        /// Confirm the (real) trade.
        #[arg(long)]
        yes: bool,
    },
    /// Atomically replace a resting order (new size and/or price). REAL TRADING
    /// — requires --yes.
    Replace {
        id: String,
        /// New size (base currency, or quote with --quote).
        #[arg(long)]
        size: Option<String>,
        /// New limit price.
        #[arg(long)]
        price: Option<String>,
        /// Interpret `--size` as the quote currency amount.
        #[arg(long)]
        quote: bool,
        /// Reject if the replacement would take liquidity.
        #[arg(long)]
        post_only: bool,
        /// Confirm the (real) trade.
        #[arg(long)]
        yes: bool,
    },
    /// Cancel an order by id. REAL TRADING — requires --yes.
    Cancel {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Cancel all active orders. REAL TRADING — requires --yes.
    CancelAll {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Copy, Clone, ValueEnum)]
pub enum SideArg {
    Buy,
    Sell,
}

#[derive(Subcommand)]
pub enum TradeCmd {
    /// Public market trades for a symbol.
    All {
        symbol: String,
        /// Start of the range: a date/time (`2024-01-31`, `"2024-01-31 14:30"`), a
        /// relative offset (`7d`, `24h`), an RFC3339 timestamp, or epoch ms.
        #[arg(long, value_parser = crate::datetime::parse_when)]
        start_date: Option<i64>,
        /// End of the range (same formats as `--start-date`; defaults to now).
        #[arg(long, value_parser = crate::datetime::parse_when)]
        end_date: Option<i64>,
        /// Pagination cursor from a previous page.
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Your own (private) trades for a symbol.
    Mine {
        symbol: String,
        /// Start of the range: a date/time (`2024-01-31`, `"2024-01-31 14:30"`), a
        /// relative offset (`7d`, `24h`), an RFC3339 timestamp, or epoch ms.
        #[arg(long, value_parser = crate::datetime::parse_when)]
        start_date: Option<i64>,
        /// End of the range (same formats as `--start-date`; defaults to now).
        #[arg(long, value_parser = crate::datetime::parse_when)]
        end_date: Option<i64>,
        /// Pagination cursor from a previous page.
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
}
