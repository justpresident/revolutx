# Changelog — revolutx-cli

All notable changes to the `revolutx-cli` binary (the `revolutx` command) are
documented here, following [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The `revolutx`
library it builds on has its own changelog at [`../CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

### Added

- The agent writes a secrets-free event log to the file named by
  `REVOLUTX_AGENT_LOG` (opt-in; unset means no log). It records startup,
  `listening` (socket + access ceiling), the shutdown reason, the security
  watchdog's hard-exit reasons (clock anomaly / debugger), and panics — each a
  timestamped `0600` line — so an abnormal exit is diagnosable from a file even
  when the console is wrecked or stderr is lost. A panic hook restores the
  terminal and records the panic before chaining to the default handler. Only
  lifecycle metadata is logged; the password, keys, and one-time token never are.

### Fixed

- The agent no longer dies silently — killed by its own ptrace-protection
  watchdog — when the terminal delivers an everyday signal. Under
  `PTRACE_TRACEME`, *any* delivered signal stops the traced child first, and
  rcypher 0.4's watchdog parent treats every such stop as an attack and
  SIGKILLs it — so a terminal **window resize** (SIGWINCH) or Ctrl-Z (SIGTSTP)
  killed a hardened command outright: no message, no cleanup, and the
  operator console's raw-mode terminal left broken. The benign
  terminal-generated signals (SIGWINCH, SIGTSTP, SIGTTIN, SIGTTOU) are now
  blocked for every hardened command before the protection fork, and SIGINT is
  blocked for the agent like it already was for the shell (Ctrl-C still works —
  rustyline reads it as a keystroke and shuts down gracefully). SIGTERM/SIGHUP
  remain lethal under the watchdog, but **rcypher 0.4.1** (now adopted) makes
  that kill observable: the watchdog parent restores the terminal and exits
  `128 + signal` instead of wedging the console and exiting a bare `1`, so a
  `docker stop` (SIGTERM → 143) or terminal close (SIGHUP → 129) is legible.
  Full graceful handling (re-injecting signals with `PTRACE_CONT` instead of
  killing) is still future work in rcypher.
- Every agent exit path now restores the terminal state captured at startup —
  the idle auto-lock, a server error, Ctrl-C, panics, and the watchdog's hard
  exits previously ended the process while the operator console still held the
  terminal in raw mode, leaving the shell unusable and printing the farewell
  (e.g. "idle timeout — locking and exiting") garbled into it, which read as a
  crash with no logs.

## [0.3.1] - 2026-07-03

### Changed

- The date-range flags are now `--since` / `--until` on `orders historical`,
  `trades all`, and `trades mine`, matching `market candles`. The old
  `--start-date` / `--end-date` spellings remain accepted as hidden aliases.

### Fixed

- `orders historical --since <when>` and `trades mine <symbol> --since <when>`
  now return the whole range through `--until` (default: now). The exchange
  answers at most 30 days per query and defaults a missing end date to start +
  7 days, so these commands previously showed only the first week after the
  start date — and with no `--since` at all they cover only the last 7 days
  (now documented in `--help`, along with the same server rules on
  `trades all`). Ranges beyond 30 days are fetched in windows automatically,
  with bounded retries on rate limits; `--limit` caps the total, newest first;
  `--cursor` remains verbatim manual pagination.
- Date flags now accept a bare year (`2026`) and a year-month (`2026-05`),
  reading them as calendar dates. A bare `2026` was previously parsed as a raw
  *epoch* — an instant in early 1970 — which turned a historical-orders range
  into a half-century walk that tripped the exchange's rate limit. Epoch
  values large enough to be real timestamps still parse as epochs.
- An empty orders listing prints `(no orders)` instead of a bare header row
  that read like a rendering bug.
- Symbols are accepted in either form (`BTC-USD` or `BTC/USD`) in all
  commands: the two `trades` commands previously rejected the slash form
  (via the library fix; the other commands already normalized it).

### Added

- `orders get` now shows trigger details (`condition:`, `t/profit:`, `s/loss:`)
  for conditional and TP/SL orders, which were previously rendered without
  their triggers. (TP/SL orders remain read-only through the public API — see
  the library's `examples/tpsl_probe.rs` for the production probe that
  established this.)

## [0.3.0] - 2026-07-02

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
- The `agent` operator console gains a `> ` prompt, session (in-memory, never
  persisted) history, and completion (commands, then live connection ids for
  `grant`/`deny`, then tiers for `grant <id>`). A piped/redirected stdin falls back to
  plain line reading. `list` also shows each peer's process **NAME** (from
  `/proc/<pid>/comm`). Connection labels are sanitized before printing (control
  characters shown as `·`), so a crafted label cannot inject terminal escape sequences.
- REPL completion no longer offers the shell-unavailable `vault`/`agent`/`cli`
  commands; the `market watch` poll loop is shared with the one-shot path. REPL history
  remains in-memory for the session only (not persisted to disk).

### Fixed

- `agent start` refuses an oversized forwarded response gracefully instead of
  desynchronizing the client's connection, and handles larger (up to 8 MiB) market-data
  responses.
- `balances` shows a **STAKED** column, so AVAILABLE + RESERVED + STAKED reconciles to
  TOTAL; staked funds were previously invisible outside `--json`.
- `orders limit`/`market` decimal errors name the offending field (`size` vs `price`).
- The REPL accepts `--json` after the command (`balances --json`), not only before it.
- Bare epoch integers in date flags are auto-detected as seconds or milliseconds by
  magnitude (a pasted seconds value is no longer read as 1970), and RFC 3339 sub-second
  precision is preserved.
- `market watch` stops on **Enter** — the reliable signal under the SIGINT hardening the
  command runs with (the previous Ctrl-C handler was unreachable).
- With `--insecure-env`, `REVOLUTX_ENVIRONMENT` now takes effect when `--env` is not
  passed (the flag default previously always overrode it).

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
