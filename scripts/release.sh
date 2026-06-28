#!/usr/bin/env bash
#
# release.sh — publish one workspace crate to crates.io + GitHub.
#
# Usage:
#   scripts/release.sh <crate> [--yes]
#
#   <crate>   one of: revolutx | revolutx-cli | revolutx-mcp
#   --yes     skip the confirmation prompt before the irreversible steps
#
# Each crate is versioned and tagged independently. The version is read from that
# crate's own Cargo.toml — bump it (and the changelog) and commit BEFORE running
# this. Tag scheme: the library uses the historical bare `vX.Y.Z`; the binaries
# are namespaced `<crate>-vX.Y.Z`.
#
# Release ORDER matters: publish `revolutx` first (the binaries depend on it),
# then `revolutx-cli` / `revolutx-mcp`. A binary's verify build resolves
# `revolutx = "X.Y"` from crates.io, so it fails fast if the library isn't live.
#
# This automates the mechanical half of docs/crate_release_process.md (it assumes
# the review, validation gate, changelog, and version bump are already done and
# committed). In order it:
#   1. pre-flight: on master, clean tree, tools authenticated, tag not yet taken;
#   2. `git pull --rebase` — absorb the coverage-badge CI commit BEFORE tagging,
#      so the tag lands on the final commit and never has to be moved;
#   3. `cargo publish --dry-run` — verify the exact artifact builds in isolation;
#   4. tag the release commit, push the branch + tag;
#   5. `cargo publish` (IRREVERSIBLE — crates.io versions are immutable);
#   6. `gh release create` (notes from CHANGELOG.md for the library).
#
set -euo pipefail

die() { echo "release: $*" >&2; exit 1; }
step() { printf '\n=== %s ===\n' "$*"; }

# --- parse args ---------------------------------------------------------------
CRATE=""
ASSUME_YES=0
for arg in "$@"; do
    case "$arg" in
        --yes | -y) ASSUME_YES=1 ;;
        -*) die "unknown option: $arg" ;;
        *) [ -z "$CRATE" ] && CRATE="$arg" || die "unexpected argument: $arg" ;;
    esac
done
[ -n "$CRATE" ] || die "usage: release.sh <crate> [--yes]  (revolutx | revolutx-cli | revolutx-mcp)"

# --- run from the repo root ---------------------------------------------------
ROOT="$(git rev-parse --show-toplevel)" || die "not inside a git repository"
cd "$ROOT"

# --- crate -> manifest dir ----------------------------------------------------
case "$CRATE" in
    revolutx) DIR="." ;;
    revolutx-cli) DIR="cli" ;;
    revolutx-mcp) DIR="mcp" ;;
    *) die "unknown crate '$CRATE' (expected revolutx, revolutx-cli, or revolutx-mcp)" ;;
esac
MANIFEST="$DIR/Cargo.toml"
[ -f "$MANIFEST" ] || die "no manifest at $MANIFEST"

# Version: the first `version = "..."` inside the [package] table.
VERSION="$(sed -n '/^\[package\]/,/^\[/{s/^[[:space:]]*version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p;}' "$MANIFEST" | head -n1)"
[ -n "$VERSION" ] || die "could not read [package] version from $MANIFEST"

# Tag: the library keeps the historical bare `vX.Y.Z`; binaries are namespaced.
if [ "$CRATE" = "revolutx" ]; then TAG="v$VERSION"; else TAG="$CRATE-v$VERSION"; fi

echo "release: $CRATE $VERSION  ->  tag $TAG"

# --- pre-flight ---------------------------------------------------------------
step "pre-flight"
BRANCH="$(git rev-parse --abbrev-ref HEAD)"
[ "$BRANCH" = "master" ] || die "not on master (on '$BRANCH')"
git diff --quiet && git diff --cached --quiet \
    || die "working tree is dirty — commit the version bump + changelog first"
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
    die "tag $TAG already exists — the version is already released, or delete the stale local tag (git tag -d $TAG)"
fi
command -v gh >/dev/null 2>&1 || die "the 'gh' CLI is required"
gh auth status >/dev/null 2>&1 || die "gh is not authenticated — run 'gh auth login'"
[ -n "${CARGO_REGISTRY_TOKEN:-}" ] || ls ~/.cargo/credentials* >/dev/null 2>&1 \
    || die "no crates.io token — run 'cargo login' or set CARGO_REGISTRY_TOKEN"

# --- sync with the remote BEFORE tagging --------------------------------------
# The CI coverage job pushes a "[skip ci]" badge commit after every master push.
# Rebasing now means the release commit reaches its FINAL hash before we tag it,
# so the tag never has to be deleted and re-created (the bug this guards against).
step "git pull --rebase origin master"
git fetch origin master
git pull --rebase origin master

# --- verify the artifact (no upload) ------------------------------------------
step "cargo publish --dry-run -p $CRATE"
cargo publish -p "$CRATE" --dry-run

# --- confirm before anything irreversible -------------------------------------
echo
echo "About to PUSH and PUBLISH — this is IRREVERSIBLE (crates.io versions are immutable):"
echo "  crate:   $CRATE $VERSION"
echo "  tag:     $TAG -> $(git rev-parse --short HEAD)  ($(git log -1 --format=%s))"
echo "  remote:  $(git remote get-url origin)"
if [ "$ASSUME_YES" -ne 1 ]; then
    printf 'Continue? [y/N] '
    read -r reply </dev/tty
    case "$reply" in y | Y | yes | YES) ;; *) die "aborted by user" ;; esac
fi

# --- tag, push, publish -------------------------------------------------------
step "tag + push"
git tag -a "$TAG" -m "$CRATE $VERSION"
git push origin master
git push origin "$TAG"

step "cargo publish -p $CRATE"
cargo publish -p "$CRATE"

# --- GitHub release -----------------------------------------------------------
step "gh release create $TAG"
NOTES="$(mktemp)"
trap 'rm -f "$NOTES"' EXIT
if [ "$CRATE" = "revolutx" ]; then
    # The root CHANGELOG.md is the library's; pull this version's section as notes.
    awk -v v="$VERSION" '
        $0 ~ "^## \\[" v "\\]" { f = 1; next }
        /^## \[/ { f = 0 }
        f { print }
    ' CHANGELOG.md > "$NOTES"
fi
if [ -s "$NOTES" ]; then
    gh release create "$TAG" --title "$TAG" --notes-file "$NOTES"
else
    gh release create "$TAG" --title "$TAG" \
        --notes "\`$CRATE\` $VERSION. See CHANGELOG.md and <https://crates.io/crates/$CRATE/$VERSION>."
fi

REPO_URL="$(git remote get-url origin | sed 's#git@github.com:#https://github.com/#; s#\.git$##')"
echo
echo "release: done — $CRATE $VERSION"
echo "  crates.io: https://crates.io/crates/$CRATE/$VERSION"
echo "  github:    $REPO_URL/releases/tag/$TAG"
