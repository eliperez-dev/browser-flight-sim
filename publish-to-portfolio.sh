#!/usr/bin/env bash
# Builds the client in release mode with Trunk and copies the output into
# the portfolio site's static/flight-sim, replacing whatever was there
# (including stale hash-named .js/.wasm from previous builds — Trunk names
# output by content hash, so old files never get overwritten in place and
# just pile up otherwise).
#
# This does not touch git or deploy anything: you still review and push
# Portfolio-Website yourself. Two separate repos, two separate deploy
# pipelines (DigitalOcean App Platform for server/, Netlify for the site) —
# keeping the publish step manual avoids wiring cross-repo CI for something
# that happens a handful of times a year.
set -euo pipefail

CLIENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/client" && pwd)"
PORTFOLIO_DIR="${PORTFOLIO_DIR:-$HOME/Portfolio-Website}"
DEST="$PORTFOLIO_DIR/static/flight-sim"

if [ ! -d "$PORTFOLIO_DIR" ]; then
    echo "error: portfolio repo not found at $PORTFOLIO_DIR (override with PORTFOLIO_DIR=...)" >&2
    exit 1
fi

# Trunk can skip regenerating index.html if its cargo build considers
# everything already up to date (e.g. after a trunk.toml-only change with no
# Rust source changes) — force a clean slate so config changes always apply.
echo "==> Cleaning previous dist/"
rm -rf "$CLIENT_DIR/dist"

# --public-url is passed explicitly rather than relying on trunk.toml's
# public_url key: on this Trunk version (0.21.14) that key was silently
# ignored (dist/index.html kept emitting absolute "/browser-flight-sim-*"
# asset paths, which 404 once served under /flight-sim/ instead of the
# domain root) while the equivalent CLI flag worked correctly.
echo "==> Building release wasm client with Trunk"
(cd "$CLIENT_DIR" && trunk build --release --public-url /flight-sim/)

BUILT="$CLIENT_DIR/dist"
if [ ! -f "$BUILT/index.html" ]; then
    echo "error: expected Trunk output at $BUILT/index.html, not found" >&2
    exit 1
fi

echo "==> Replacing $DEST with fresh build"
rm -rf "$DEST"
mkdir -p "$DEST"
cp -r "$BUILT"/. "$DEST"/

echo "==> Done. Built files:"
ls -la "$DEST"

echo
echo "Next: cd $PORTFOLIO_DIR && git status, review, commit, and push to deploy via Netlify."
