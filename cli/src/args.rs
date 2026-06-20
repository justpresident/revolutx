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
    /// Path to the encrypted vault (default: $`XDG_CONFIG_HOME/revolutx/vault`).
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
    /// Run or query the signing agent (a daemon that unlocks the vault once and
    /// signs/sends on behalf of headless clients).
    Agent {
        #[command(subcommand)]
        command: AgentCmd,
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
            // `agent start` unlocks the vault (needs hardening); `agent ping` is
            // a thin client that only talks to an existing daemon.
            Self::Agent { command } => matches!(command, AgentCmd::Start { .. }),
            _ => true,
        }
    }
}

#[derive(Subcommand)]
pub enum AgentCmd {
    /// Unlock the vault and serve signing requests over a unix socket until
    /// stopped (Ctrl-C). Runs in the foreground.
    Start {
        /// Unix socket path (default: $`XDG_RUNTIME_DIR/revolutx-agent.sock`).
        #[arg(long)]
        socket: Option<PathBuf>,
        /// Auto-lock (exit) after this many seconds with no requests. 0 disables
        /// the idle timeout.
        #[arg(long, default_value_t = 0)]
        idle_timeout: u64,
    },
    /// Check that an agent is responding on the socket.
    Ping {
        /// Unix socket path (default: $`XDG_RUNTIME_DIR/revolutx-agent.sock`).
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum VaultCmd {
    /// Create a new encrypted vault from an API key and an Ed25519 PEM key.
    Init {
        /// Path to an existing Ed25519 private key PEM (e.g. from
        /// `openssl genpkey -algorithm ed25519 -out private.pem`).
        #[arg(long)]
        key_file: PathBuf,
        /// API key (prompted, hidden, if omitted).
        #[arg(long)]
        api_key: Option<String>,
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
        /// Start time, Unix epoch milliseconds.
        #[arg(long)]
        since: Option<i64>,
        /// End time, Unix epoch milliseconds.
        #[arg(long)]
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
        #[arg(long)]
        limit: Option<u32>,
    },
    /// List historical orders.
    Historical {
        #[arg(long)]
        symbol: Vec<String>,
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
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Your own (private) trades for a symbol.
    Mine {
        symbol: String,
        #[arg(long)]
        limit: Option<u32>,
    },
}
