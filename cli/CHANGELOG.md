# Changelog — revolutx-cli

All notable changes to the `revolutx-cli` binary (the `revolutx` command) are
documented here, following [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The `revolutx`
library it builds on has its own changelog at [`../CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

## [0.3.1] - 2026-07-01

### Fixed

- `agent start` (via revolutx 0.5.1) refuses an oversized forwarded response gracefully
  instead of desynchronizing the client's connection, and handles larger (up to 8 MiB)
  market-data responses.

## [0.3.0] - 2026-07-01

`agent start` becomes a persistent, multi-client agent with an interactive operator
console.

### Added

- Operator console on the agent's stdin: `list` (each connection's id, uid/gid/pid,
  method, state, label), `grant <id> [market|view|trading]`, `deny <id>`, `help`,
  `quit`.
- Manual authorization: a connection with no token becomes *pending* and is granted
  or denied from the console at a tier the operator picks (up to the `--access`
  ceiling). Any number of clients can be authorized at once.

### Changed

- `--auth-token` is now optional — a second, headless auth method alongside manual
  approval, rather than required.
- `--access` is the grant *ceiling*; `--idle-timeout` now auto-locks after no
  *authorized* client has been connected for the timeout.
- The socket is world-connectable and cross-UID clients are allowed and evaluated by
  the operator (their uid/gid/pid are shown) — the same-UID requirement is dropped.

## [0.2.1] - 2026-06-30

Modernizes `vault init` on rcypher 0.4's reusable new-store flow (optional FIDO2
enrolment at creation), stores the public key, and shows a progress spinner during
the slow key-derivation steps.

### Added

- `vault init` now also stores the Ed25519 public key in the vault (record
  `public_key_pem`), so it's always on hand for reference. For `--key-file`
  imports it is derived from the supplied private key.
- A progress spinner on stderr during the slow Argon2 steps of vault unlock and
  creation, so the CLI no longer appears to hang while a key is derived. It is
  injected into rcypher's `UnlockProgress` hook and shown only on an interactive
  terminal (suppressed for piped output and CI).

### Changed

- `vault init` now drives rcypher 0.4's standard new-store flow
  (`cli::prompt_until_initialized`) instead of a hand-rolled sequence. The
  unrecoverable-password warning, double confirmation, and zxcvbn strength gate
  are unchanged. When built with the `fido2` feature it now also offers to enrol
  a security key at creation and, if you accept, lets you choose the unlock
  policy (any one factor, or all) — previously a key could only be added
  afterward with the `rcypher` CLI.

## [0.2.0] - 2026-06-28

First release on the `revolutx` 0.3 SDK: adds an interactive shell and a
capability-tier access model, and moves vault unlock to rcypher's multi-factor
store.

### Added

- **`revolutx cli` interactive shell**: unlocks the vault once, then a REPL that
  runs the same commands as the one-shot CLI, with history, line editing, and
  Tab-completion of commands and trading symbols. Real-trading commands prompt for
  confirmation instead of requiring `--yes`; `market watch` streams until you press
  Enter.
- **`--access market | view | trading` capability tiers.** `revolutx agent start
  --access` sets the tier the agent serves and enforces (default `market`, least
  privilege); `revolutx cli --access` gates the shell locally (default `view`) so an
  agent policy can be rehearsed. One-shot commands are run by the credential owner
  and are not gated.
- Parity flags so the commands match the SDK's full surface: `--side`/`--cursor` on
  `orders active`, date ranges + `--cursor` on `orders historical` and `trades`,
  and `--client-order-id` on `orders limit`/`market`. Date/time flags accept human
  forms (`2024-01-31`, `"2024-01-31 14:30"`, `7d`, an RFC 3339 timestamp) as well as
  epoch milliseconds. Order tables gained a SYMBOL column.

### Changed

- Default-on `fido2` feature (`--no-default-features` for hosts without
  `libudev`/`hidapi`); vault unlock now drives rcypher 0.3's interactive
  multi-factor loop (password and/or FIDO2 security key) against the standard
  `SecretStore` vault. **No migration** from the old vault format — re-run
  `revolutx vault init`.
- `revolutx agent start` requires `--auth-token`: it prints a one-time token the
  connecting client must present before the agent serves any request, so another
  same-UID process cannot use the signing oracle.

### Removed

- `revolutx agent start --enable-trading`, replaced by `--access trading` (the tier
  ladder also gates account reads behind `--access view`, which the boolean did
  not).

## [0.1.0] - 2026-06-20

Initial release: the `revolutx` command-line interface — vault management
(`vault init`), account balances, exchange configuration, market data, orders, and
trade history — with process hardening (no core dumps; anti-debugger) for the
credential-handling commands.
