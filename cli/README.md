# revolutx-cli

Command-line interface for the [Revolut X](https://exchange.revolut.com/) crypto
exchange, built on the [`revolutx`](https://crates.io/crates/revolutx) SDK. The
installed binary is named `revolutx`.

```sh
cargo install revolutx-cli
```

## Setup

Authenticated commands read credentials from an **encrypted vault** (rcypher:
Argon2id + AES-256-CBC + HMAC) at `~/.revolutx/vault`, unlocked with a master
password. Initialize it once:

```sh
revolutx vault init
```

This:

1. prompts for a master password,
2. generates an Ed25519 key pair (the private key is stored **only** in the
   vault — it never touches the disk unencrypted),
3. prints the public key with instructions to create your API key at
   <https://exchange.revolut.com>, and
4. stores the API key you paste back in.

Already have a key pair? `revolutx vault init --key-file private.pem` imports it
instead of generating one.

Then run commands — you'll be prompted only for the master password to unlock the
vault:

```sh
revolutx balances
revolutx market public-order-book BTC-USD       # public, no credentials
revolutx market order-book BTC-USD --limit 10
revolutx orders active
revolutx orders limit buy BTC-USD 0.001 50000 --post-only --yes   # REAL TRADING
revolutx orders replace <ID> --price 49000 --yes                  # atomic amend (size and/or price)
revolutx --json market tickers                  # machine-readable output
```

For dev/CI you can bypass the vault with `--insecure-env` (plaintext credentials
from `REVOLUTX_API_KEY` / `REVOLUTX_PRIVATE_KEY_PEM` / `REVOLUTX_PRIVATE_KEY_PATH`).

## Signing agent (headless / no-TTY clients)

A headless client (such as the MCP server) has no terminal to prompt for the
master password. Run a **signing agent**: it unlocks the vault once, then signs
and performs every request on behalf of the client connected to its unix socket.

```sh
revolutx agent start                       # prompts once, then serves (read-only)
revolutx agent start --enable-trading      # also permit order placement/cancellation (REAL TRADING)
revolutx agent start --idle-timeout 600    # exit if no client connects in 10 min
revolutx agent start --idle-timeout 0      # never auto-lock before connecting
```

It is a **full proxy**: the client sends a request description and receives only
the response bytes — neither the private key nor the API key ever leaves the
agent. Protections:

- **Single client.** The agent accepts exactly one connection and refuses the
  rest. When that client disconnects, the daemon exits and the vault is
  re-locked — reconnects are not allowed.
- **Trading off by default.** The agent refuses every order-mutating request
  unless started with `--enable-trading`. This is the authoritative gate — a
  connected client (e.g. the MCP) cannot turn trading on.
- **Pre-connection idle timeout** (default 5 minutes). If no client connects in
  time, the agent auto-locks and exits. Once a client *is* connected it is never
  timed out for being idle.
- **`0600` socket** in `$XDG_RUNTIME_DIR` (itself user-private); no network
  transport.
- A **watchdog thread** keeps checking for an attached debugger while serving.

## Safety

- Order placement / cancellation is **real trading** and requires `--yes`.
- Vault-unlocking commands harden the process (no core dumps; anti-debugger)
  before doing anything; use `--insecure-allow-debugging` on legitimately-traced
  hosts (CI, profilers).

## License

Licensed under the [Apache License 2.0](../LICENSE).
