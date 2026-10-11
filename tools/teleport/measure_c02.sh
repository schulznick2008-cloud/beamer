#!/bin/bash
# Messung fuer Paket C02 (Mac-Sender): Capture bis dekodiert, CPU des Helfers, Bitrate.
# Voraussetzung: ./build.sh in teleport/mac, Bildschirmaufnahme-Freigabe fuer das Terminal.
#   tools/teleport/measure_c02.sh [hevc|h264] [Sekunden]
set -euo pipefail
cd "$(dirname "$0")/../.."
CODEC="${1:-hevc}"; SECS="${2:-30}"; PORT=47010
BIN=teleport/mac/.build/release/BeamerTeleport
[ -x "$BIN" ] || { echo "erst teleport/mac/build.sh ausfuehren" >&2; exit 1; }
swiftc -O tools/teleport/anim_window.swift -o /tmp/teleport_anim
/tmp/teleport_anim >/dev/null 2>&1 &
ANIM=$!
trap 'kill $ANIM 2>/dev/null || true' EXIT
sleep 2
"$BIN" --selftest-recv $PORT --seconds $((SECS + 6)) > /tmp/c02_recv.out 2>&1 &
sleep 1
tools/teleport/send_demo.py --helper "$BIN" --app teleport_anim --peer 127.0.0.1:$PORT \
    --codec "$CODEC" --seconds "$SECS" | tee /tmp/c02_send.out | grep -E "send\.|^stats" | tail -n +1
wait %2 2>/dev/null || sleep 7
echo "--- Empfaenger (Capture bis dekodiert, gleiche Host-Uhr) ---"
tail -3 /tmp/c02_recv.out
echo "--- Sender-CPU (Mittel ab Sekunde 3, Prozent eines Kerns) ---"
grep '^stats' /tmp/c02_send.out | tail -n +3 | sed -E 's/.*cpu=([0-9.]+)%.*/\1/' | awk '{s+=$1;n++} END {printf "%.1f %% (n=%d)\n", s/n, n}'
