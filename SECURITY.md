# Security Policy

## Reporting a Vulnerability

If you discover a security issue, please report it **privately** — do not open a
public issue or disclose it before it has been addressed.

Contact: justpresident át gmail ɗοt com

Please include a description, reproduction steps if possible, and affected
versions. You can expect an acknowledgment within a reasonable time.

## Scope

`revolutx` is an exchange API client. Its security-sensitive surface is request
authentication (Ed25519 signing) and — with the `keystore` feature — protecting
credentials at rest. The keystore is built on [`rcypher`](https://crates.io/crates/rcypher)
and adopts its threat model.

**In scope:**
- Correct, automatic request signing (the signed message must match the bytes
  sent on the wire).
- Protecting the API key and private key **at rest** (the `keystore` feature:
  Argon2id + AES-256-CBC + HMAC, encrypt-then-MAC).
- Best-effort in-memory hygiene: secrets are zeroized after use (`zeroize`), the
  keystore decrypts per request and wipes the plaintext immediately, and the
  default signer's key is `ZeroizeOnDrop`.

**Out of scope** (matching rcypher):
- A compromised operating system.
- Malware, keyloggers, or clipboard managers on the host.
- Privileged (root) attackers.
- Side-channel attacks.
- Availability / denial-of-service.

## Runtime hardening (binaries)

Process hardening is the responsibility of the **binary** that unlocks a vault,
not of this library — a library must never `fork()` as a side effect. The
`keystore` feature re-exports rcypher's helpers
(`enable_ptrace_protection`, `disable_core_dumps`, `is_debugger_attached`); an
unlocking process should call them at the very top of `main`, **before** starting
any threads or async runtime (`enable_ptrace_protection` forks on Linux). The
vault cipher also refuses to operate while a debugger is attached unless
`KeystoreOptions::trace_detection` is disabled (for legitimately-traced hosts).

These are defense-in-depth measures; they raise the bar against casual memory
inspection but cannot stop an attacker with OS-level privileges.

## Honest limits

- The API key must be placed in an HTTP header on every request, so `reqwest`
  holds a copy in its own (non-zeroizing) buffers for the duration of the
  request — beyond this crate's control.
- Memory is not locked against swapping (no `mlock`).
- No formal security audit has been performed. No guarantees are made beyond
  what is documented here.
