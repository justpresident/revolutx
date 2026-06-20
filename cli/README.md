# revolutx-cli

Command-line interface for the [Revolut X](https://exchange.revolut.com/) crypto
exchange, built on the [`revolutx`](https://crates.io/crates/revolutx) SDK. The
installed binary is named `revolutx`.

```sh
cargo install revolutx-cli
```

## Credentials

By default, authenticated commands read credentials from an **encrypted vault**
(rcypher: Argon2id + AES-256-CBC + HMAC), unlocked with a master password. Create
one from an Ed25519 key (see the SDK README for `openssl genpkey`):

```sh
revolutx vault init --key-file private.pem      # prompts for API key + master password
```

Then run commands — you'll be prompted for the master password when needed:

```sh
revolutx balances
revolutx market public-order-book BTC-USD       # public, no credentials
revolutx market order-book BTC-USD --limit 10
revolutx orders active
revolutx orders limit buy BTC-USD 0.001 50000 --post-only --yes   # REAL TRADING
revolutx --json market tickers                  # machine-readable output
```

For dev/CI you can bypass the vault with `--insecure-env` (plaintext credentials
from `REVOLUTX_API_KEY` / `REVOLUTX_PRIVATE_KEY_PEM` / `REVOLUTX_PRIVATE_KEY_PATH`).

## Signing agent (headless / no-TTY clients)

A headless client (such as the MCP server) has no terminal to prompt for the
master password. Run a **signing agent**: it unlocks the vault once, then signs
and performs every request on behalf of clients that connect to its unix socket.

```sh
revolutx agent start                       # prompts once, then serves
revolutx agent ping                        # check it's alive
revolutx agent start --idle-timeout 900    # auto-lock after 15 min idle
```

It is a **full proxy**: clients send a request description and receive only the
response bytes — neither the private key nor the API key ever leaves the agent.
The socket is created `0600` in `$XDG_RUNTIME_DIR` (itself user-private); there
is no network transport. While serving, a watchdog thread keeps checking for an
attached debugger and enforces the idle auto-lock.

## Safety

- Order placement / cancellation is **real trading** and requires `--yes`.
- Vault-unlocking commands harden the process (no core dumps; anti-debugger)
  before doing anything; use `--insecure-allow-debugging` on legitimately-traced
  hosts (CI, profilers).

## License

Licensed under the [Apache License 2.0](../LICENSE).
