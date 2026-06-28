# Crate release process

This workspace publishes **three crates to crates.io**, each **versioned and
tagged independently**:

| Crate          | Manifest         | Tag scheme            | Notes |
| -------------- | ---------------- | --------------------- | ----- |
| `revolutx`     | `Cargo.toml`     | `vX.Y.Z`              | the library; the dependency the binaries build on |
| `revolutx-cli` | `cli/Cargo.toml` | `revolutx-cli-vX.Y.Z` | binary (`cargo install` builds it) |
| `revolutx-mcp` | `mcp/Cargo.toml` | `revolutx-mcp-vX.Y.Z` | binary (`cargo install` builds it) |

Each crate's version lives in **its own `Cargo.toml`** and moves on its own
cadence — bumping the library does not force a binary bump, and vice versa. The
library keeps the historical bare `vX.Y.Z` tag (`v0.1.0`, `v0.2.0`, …); the
binaries are namespaced so their tags never collide with it.

crates.io versions are **immutable**: once `cargo publish` succeeds you cannot
overwrite or re-upload that version (you can only *yank* it). Everything before
the publish step exists to make the artifact correct *before* it goes out. None
of these crates ship prebuilt binary assets — the GitHub release is just notes.

## Automated path: `scripts/release.sh`

The mechanical half — sync, dry-run, tag, push, publish, GitHub release — is
automated:

```bash
scripts/release.sh <crate> [--yes]      # revolutx | revolutx-cli | revolutx-mcp
```

It reads the version from that crate's `Cargo.toml`, derives the tag, and runs
the steps below in order. It **assumes the human steps are already done and
committed** (review, validation gate, changelog, version bump) — it does not make
those judgments for you. Before the irreversible push/publish it prints a summary
and asks for confirmation (`--yes` skips the prompt).

**Release order matters.** Publish `revolutx` **first**; the binaries depend on it
by version, so a binary's verify build resolves `revolutx = "X.Y"` from crates.io
and fails fast if the library isn't live yet. So:

```bash
scripts/release.sh revolutx          # 1. the library
scripts/release.sh revolutx-cli      # 2. then the binaries (any order)
scripts/release.sh revolutx-mcp
```

What the script does:

1. **Pre-flight** — on `master`, clean working tree, `gh` authenticated, a
   crates.io token present (`cargo login` or `CARGO_REGISTRY_TOKEN`), and the tag
   not already taken.
2. **`git pull --rebase origin master`** — *before* tagging. CI's coverage job
   pushes a `[skip ci]` badge commit after every master push, so the release
   commit must be rebased onto the current remote tip first; tagging only after
   the rebase means the tag lands on the final commit and never has to be deleted
   and re-created.
3. **`cargo publish --dry-run -p <crate>`** — builds the package in isolation (the
   OpenAPI submodule is excluded), catching a broken artifact before anything is
   pushed.
4. **tag + push** — annotated tag on the rebased release commit, then push the
   branch and the tag.
5. **`cargo publish -p <crate>`** — the irreversible upload.
6. **`gh release create`** — notes from this version's `CHANGELOG.md` section for
   the library; a short pointer for the binaries (they share the root changelog).

The sections below are the human judgment the script relies on; do them, commit,
then run the script.

## Pre-flight

0. Be on `master`, up to date, with a clean working tree and CI green on the last
   commit. Authenticate to crates.io (`cargo login`, or `CARGO_REGISTRY_TOKEN`
   set) and GitHub (`gh auth login`) — the script checks both, but fix them now.
   The OpenAPI submodule must be initialized (`git submodule update --init`) so
   the spec-backed tests can run.

## Review what's shipping

1. Check what changed since the crate's last release: `git log <last-tag>..HEAD`
   (`git describe --tags --abbrev=0 --match 'v*'` for the library; for a binary,
   `git describe --tags --abbrev=0 --match 'revolutx-cli-v*'`). For a crate's
   first publish, review its whole tree.
2. Check the files that changed: `git diff <last-tag>..HEAD --name-status`.
3. Read the changed Rust files in full and make sure:
   - a) all doc comments and code comments are correct and precise;
   - b) every public item is documented and the docs match the real API;
   - c) `docs/openapi-inventory.md` and `tests/spec_coverage.rs` still match the
     spec (the coverage test fails if they drift).
4. **Groom the user-facing docs and examples** so they reflect this release:
   - a) the root `README.md` (and `cli/README.md`, `mcp/README.md`) cover **all**
     current functionality — every Cargo feature, every endpoint group, every CLI
     command / MCP tool, and any capability shipped since the last release — with
     nothing stale;
   - b) bump the version references to the version you're releasing: the
     `revolutx = "X.Y"` install snippets and the MSRV note in `README.md`. The
     `cargo build` gate in [Validate](#validate) catches code that stops
     compiling, but **not** a doc that still advertises an old version;
   - c) the runnable examples in `examples/` exercise the current API, and a new
     public capability worth showing has an example (they must also compile —
     `cargo build --examples`, run below).
5. **Look for opportunities to improve the code** — abstractions that can be
   simplified, code that can be made more readable, duplication that can be
   removed. THIS IS REALLY IMPORTANT. Ask a human if you have ideas you are not
   certain about. Commit and re-test any changes you make here before continuing.

## Validate

6. Make the full gate pass cleanly (and commit any fixes). These mirror CI; the
   `--workspace` flags matter because the binary crates live here too and must
   build/lint/test as well:
   ```bash
   cargo fmt --all --check
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test --workspace --all-features
   RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
   cargo build --examples            # the public examples must compile
   ```
   Also keep the optional-feature tree clean (mirrors CI's feature matrix — this
   is what catches a feature accidentally pulling a default-on dependency, or
   non-default code that fails to compile in isolation):
   ```bash
   cargo clippy -p revolutx --no-default-features -- -D warnings               # models only
   cargo clippy -p revolutx --no-default-features --features fix -- -D warnings
   cargo clippy -p revolutx --no-default-features --features agent --all-targets -- -D warnings
   cargo clippy -p revolutx --no-default-features --features commands --all-targets -- -D warnings
   cargo clippy -p revolutx --all-features -- -D warnings
   ```
   The default test suite is offline and deterministic. Do **not** run the
   ignored live smoke tests as part of a release (they need real credentials and
   hit the network); they are not a release gate.

## Update the changelog

7. Update `CHANGELOG.md` at the repo root ([Keep a Changelog](https://keepachangelog.com/)
   format). It is the **library's** changelog and the single source of truth for
   the library's GitHub release notes; it also narrates the workspace's changes,
   so a binary release that ships alongside is described here too. From the `git
   log` review above, turn the `[Unreleased]` section into a dated version
   section, grouping notable changes under `Added` / `Changed` / `Fixed` /
   `Removed`. Summarize what users care about, not raw commit subjects.
   ```markdown
   ## [0.3.0] - 2026-07-01
   ### Changed
   - Credential vault moved to rcypher 0.3's `SecretStore` format …
   ```
   (`CHANGELOG.md` ships in the published crate — `exclude` only drops the
   submodule, dev-only files, `scripts/`, and the two submodule-dependent tests.)

## Bump the versions

8. Decide each crate's new version from the review above (pre-1.0 semver: breaking
   changes bump the **minor**, features/fixes bump the **patch**). Set `version`
   in the relevant `Cargo.toml`(s) — only the crates that actually changed — then
   run `cargo build` so `Cargo.lock` picks the new versions up. If you bumped
   `revolutx`, also update its dependents — see
   [Workspace dependencies](#workspace-dependencies).
9. Commit the bumps and changelog as the release commit(s), so the tree is clean
   before the script tags it:
   ```bash
   git add -A && git commit -m "Release v0.3.0"
   ```
   The script then verifies the package without `--allow-dirty` (which would
   validate a tarball containing uncommitted changes you'll never tag). You can
   eyeball the contents first:
   ```bash
   cargo package --list -p <crate>   # no revolut-openapi/, .taska/, .github/, scripts/
   ```

## Workspace dependencies

The crates in this workspace depend on `revolutx` by **both** `path` and
`version` — e.g. `revolutx-mcp` has:

```toml
revolutx = { path = "..", version = "0.3", features = ["rest", "agent", "commands"] }
```

The `path` is only used for local/workspace builds; the **`version` requirement is
what gets enforced once the crate is published** (cargo strips the `path` on
publish). So every time you release `revolutx`, check each dependent and update it
deliberately — these requirements do **not** update themselves:

1. Decide whether the dependent relies on anything introduced in *this* `revolutx`
   release.
2. If it does, bump that crate's `revolutx` `version` requirement to the version
   you're releasing, and publish the dependent **after** `revolutx` is live
   (dependency before dependents). A requirement that's too loose (e.g. `"0.2"`
   when the dependent now needs `0.3` APIs) would let the *published* dependent
   resolve against an older `revolutx` it cannot compile against — a broken
   `cargo install`.
3. If it doesn't, leave the requirement, but confirm it still resolves.

## Run the release script

10. With everything above committed, publish each crate — `revolutx` first:
    ```bash
    scripts/release.sh revolutx
    scripts/release.sh revolutx-cli
    scripts/release.sh revolutx-mcp
    ```
    Then confirm the library on <https://crates.io/crates/revolutx>, that docs
    build at <https://docs.rs/revolutx>, and the binaries at
    <https://crates.io/crates/revolutx-cli> /
    <https://crates.io/crates/revolutx-mcp>.

## Manual fallback

If the script can't be used, do its steps by hand for one crate (`revolutx`
shown; for a binary, `cargo publish -p revolutx-cli` and tag
`revolutx-cli-vX.Y.Z`). Note the **rebase before the tag**:

```bash
git fetch origin master
git pull --rebase origin master           # absorb the CI badge commit FIRST
cargo publish --dry-run                    # verify the artifact
git tag -a v0.3.0 -m "revolutx 0.3.0"      # tag the rebased commit
git push origin master
git push origin v0.3.0
cargo publish                              # irreversible
# GitHub release notes = this version's CHANGELOG.md section:
awk '/^## \[0.3.0\]/{f=1;next} /^## \[/{f=0} f' CHANGELOG.md > /tmp/notes-v0.3.0.md
gh release create v0.3.0 --title "v0.3.0" --notes-file /tmp/notes-v0.3.0.md
rm /tmp/notes-v0.3.0.md
```

There are no binary assets to attach: every crate lives on crates.io as source.
