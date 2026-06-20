# Crate release process

Publishing a new `revolutx` release to crates.io. Examples below use `v0.1.0`
as the new release tag — substitute the real versions. Get the previous tag with
`git describe --tags --abbrev=0` (there is none for the first release).

crates.io versions are **immutable**: once `cargo publish` succeeds you cannot
overwrite or re-upload that version (you can only *yank* it). Everything below
the publish step exists to make sure the artifact is correct *before* it goes
out.

`revolutx` is a **library crate**, so a release ships exactly one thing: the
crate on crates.io (`cargo publish`, manual). The GitHub release is just notes
(the changelog section) — there are no binaries to build or attach.

This repo is a **workspace** with dependent crates (`revolutx-mcp`, and any
future `revolutx-*`) that depend on `revolutx`. Releasing `revolutx` has a
knock-on effect on their version requirements — see
[Workspace dependencies](#workspace-dependencies) before you publish.

## Pre-flight

0. Be on `master`, up to date, with a clean working tree and CI green on the last
   commit. Make sure you're authenticated to crates.io (`cargo login`, or
   `CARGO_REGISTRY_TOKEN` set) — otherwise the final publish fails after all the
   work. The OpenAPI submodule must be initialized (`git submodule update --init`)
   so the spec-backed tests can run.

## Review what's shipping

1. Check what has changed since the last release: `git log v0.1.0..HEAD`
   (for the first release, review the whole history: `git log`).
2. Check the files that changed: `git diff v0.1.0..HEAD --name-status`.
3. Read the changed Rust files in full and make sure:
   - a) all doc comments and code comments are correct and precise;
   - b) every public item is documented and the docs match the real API;
   - c) `docs/openapi-inventory.md` and `tests/spec_coverage.rs` still match the
     spec (the coverage test fails if they drift).
4. **Look for opportunities to improve the code** — abstractions that can be
   simplified, code that can be made more readable, duplication that can be
   removed. THIS IS REALLY IMPORTANT. Ask a human if you have ideas you are not
   certain about. Commit and re-test any changes you make here before continuing.

## Validate

5. Make the full gate pass cleanly (and commit any fixes):
   ```bash
   cargo fmt --all --check
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test --all-features
   RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
   cargo build --examples            # the public examples must compile
   ```
   The default test suite is offline and deterministic. Do **not** run the
   ignored live smoke tests as part of a release (they need real credentials and
   hit the network); they are not a release gate.

## Update the changelog

6. Update `CHANGELOG.md` at the repo root ([Keep a Changelog](https://keepachangelog.com/)
   format). From the `git log` review above, turn the `[Unreleased]` section into
   a dated version section, grouping notable changes under
   `Added` / `Changed` / `Fixed` / `Removed`. Keep it human-readable — summarize
   what users care about, not raw commit subjects. This section is the single
   source of truth for the GitHub release notes in the last step.
   ```markdown
   ## [0.1.0] - 2026-06-19
   ### Added
   - Initial public SDK: client, Ed25519 signing, all endpoint groups, …
   ```
   (`CHANGELOG.md` ships in the published crate — `exclude` only drops the
   submodule, dev-only files, and the two submodule-dependent test files.)

## Bump and verify the artifact

7. Decide the new version from the review above (pre-1.0 semver: breaking changes
   bump the **minor**, features/fixes bump the **patch**; the first release is
   `0.1.0`). Set `version` in `Cargo.toml`, then run `cargo build` so `Cargo.lock`
   picks up the new `revolutx` version. Then check the dependent crates — see
   [Workspace dependencies](#workspace-dependencies) below.
8. Commit the bump and changelog first (so the tree is clean), then verify the
   package without `--allow-dirty` (which would validate a tarball containing
   uncommitted changes you'll never tag):
   ```bash
   cargo package --list          # eyeball the included files
   cargo publish --dry-run       # builds the package in isolation; submodule is excluded
   ```
   `cargo package --list` must show only intended files — no `revolut-openapi/`,
   no `.taska/`, no `.github/`. The verify build compiles the lib only and does
   not need the submodule.

## Workspace dependencies

The crates in this workspace depend on each other by **both** `path` and
`version` — e.g. `revolutx-mcp` has:

```toml
revolutx = { path = "..", version = "0.1" }
```

The `path` is only used for local/workspace builds; the **`version` requirement
is what gets enforced once the crate is published** (cargo strips the `path` on
publish). So every time you release `revolutx`, you must check each crate that
depends on it and update it deliberately — these requirements do **not** update
themselves:

1. Decide whether the dependent relies on anything introduced in *this* release.
2. If it does, bump that crate's `revolutx` `version` requirement to the version
   you're releasing, and publish the dependent **after** `revolutx` is live
   (dependency before dependents). A requirement that's too loose (e.g. `"0.1"`)
   would let the *published* dependent resolve against an older `revolutx` it
   cannot actually compile against — a broken `cargo install`.
3. If it doesn't, leave the requirement, but confirm it still resolves.

The next `revolutx` version is **not predetermined** — it might be a patch
(`0.1.1`) or a minor (`0.2.0`) depending on what's shipping — so bump the
dependents to whatever you actually choose here, never an assumed number.

> **Current example:** `revolutx-mcp` uses the `Serialize` impls on the response
> wrapper types (`OrderBook`, `Tickers`, `LastTrades`, `Page<T>`) that landed
> *after* `revolutx` 0.1.0. It therefore cannot be published until the next
> `revolutx` release, and its requirement must be bumped from `"0.1"` to that
> version (whatever it turns out to be) at that time.

## Commit, tag, push

9. Commit the bump and changelog as the release commit:
   ```bash
   git add Cargo.toml Cargo.lock CHANGELOG.md
   git commit -F- <<'MSG'
   Release v0.1.0
   MSG
   ```
10. Tag **that** commit so the tag and the published version agree:
    ```bash
    git tag v0.1.0
    ```
11. Push the commit and the tag:
    ```bash
    git push origin master
    git push origin v0.1.0
    ```

## Publish

12. Publish from the clean, tagged tree:
    ```bash
    cargo publish
    ```
    Then confirm it on <https://crates.io/crates/revolutx> and that docs build at
    <https://docs.rs/revolutx>.

## Create the GitHub release

13. Create a GitHub release for the tag, using the new `CHANGELOG.md` section as
    the notes. Write that section to a **temporary** scratch file at
    `docs/release-notes-v0.1.0.md` — `docs/` is fine for scratch, but **never
    commit it, and delete it as soon as the release exists**. Preferred
    (automated, via the `gh` CLI):
    ```bash
    gh release create v0.1.0 --title "v0.1.0" --notes-file docs/release-notes-v0.1.0.md
    rm docs/release-notes-v0.1.0.md   # done with it — remove, don't commit
    ```
    If you'd rather have GitHub draft the notes from merged commits/PRs instead,
    use `--generate-notes` (no scratch file needed). If `gh` is unavailable or
    unauthenticated, create it manually: GitHub → **Releases** → **Draft a new
    release** → choose the existing `v0.1.0` tag → paste the changelog section →
    **Publish release**, then delete the scratch file.

    There are no binary assets to attach: `revolutx` is a library, and the crate
    itself lives on crates.io.
