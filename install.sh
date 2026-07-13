#!/bin/sh
#
# Build and install wl-showkeys.
#
# The binary is built as the current user (so cargo/rustup are on PATH), then
# installed setuid root: it needs root to read input events and drops those
# privileges after startup. Only the install steps use sudo.

set -eu

PREFIX="${PREFIX:-/usr/local}"
BINDIR="${BINDIR:-$PREFIX/bin}"
BIN="wl-showkeys"
TARGET="target/release/$BIN"

cd "$(dirname "$0")"

echo "Building $BIN (release)..."
cargo build --release

echo "Installing to $BINDIR/$BIN (setuid root, needs sudo)..."
sudo install -Dm755 -o root -g root "$TARGET" "$BINDIR/$BIN"
sudo chmod u+s "$BINDIR/$BIN"

echo "Installed $BIN to $BINDIR/$BIN"
