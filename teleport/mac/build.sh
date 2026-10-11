#!/bin/bash
# Baut erst die Rust-Bibliothek (../transport), dann das Swift-Paket. Ohne Warnungen.
set -euo pipefail
cd "$(dirname "$0")"
CONFIG="${1:-release}"

if ! command -v cargo >/dev/null 2>&1; then
    # rustup per Homebrew ist keg-only
    for p in /opt/homebrew/opt/rustup/bin "$HOME/.cargo/bin"; do
        [ -x "$p/cargo" ] && PATH="$p:$PATH" && break
    done
fi
command -v cargo >/dev/null || { echo "cargo nicht gefunden (rustup installieren)" >&2; exit 1; }

(cd ../transport && cargo build --release)
swift build -c "$CONFIG" -Xswiftc -warnings-as-errors
echo "fertig: $(swift build -c "$CONFIG" --show-bin-path)/BeamerTeleport"
