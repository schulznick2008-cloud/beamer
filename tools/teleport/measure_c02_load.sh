#!/bin/bash
# Lasttest fuer Paket C02b: measure_c02.sh (Fenster-Capture mit Messung) plus zwei weitere Helfer, die gleichzeitig
# den ganzen Bildschirm streamen (send_demo.py --display 0). Wird 3 Mal wiederholt. Geprueft wird: kein Haenger,
# send.stop jeweils unter 2 s, EOF-Ende unter 2 s.
#   tools/teleport/measure_c02_load.sh [hevc|h264] [Sekunden] [Wiederholungen]
set -uo pipefail
cd "$(dirname "$0")/../.."
CODEC="${1:-hevc}"; SECS="${2:-30}"; RUNS="${3:-3}"
BIN=teleport/mac/.build/release/BeamerTeleport
[ -x "$BIN" ] || { echo "erst teleport/mac/build.sh ausfuehren" >&2; exit 1; }
echo "Energiesparmodus: $(pmset -g | awk '/lowpowermode/{print $2}')"
for run in $(seq 1 "$RUNS"); do
  echo "=== Lauf $run von $RUNS ==="
  pids=()
  for n in 1 2; do
    python3 -u tools/teleport/send_demo.py --helper "$BIN" --display 0 --codec "$CODEC" --seconds $((SECS + 12)) \
        > /tmp/c02_load_$n.out 2>&1 &
    pids+=($!)
  done
  sleep 8   # die Last-Helfer laufen schon, wenn die Messung startet (wie im Review)
  tools/teleport/measure_c02.sh "$CODEC" "$SECS" 2>&1 | grep -vE "^beamer-transport" | tail -6 | cut -c1-330
  # Auf die Last-Helfer warten, hoechstens 60 s; danach gilt ein Helfer als haengend.
  ( sleep 60; kill -9 "${pids[@]}" 2>/dev/null ) &
  guard=$!
  for n in 0 1; do
    if wait "${pids[$n]}"; then rc=0; else rc=$?; fi
    [ "$rc" = 137 ] && echo "Last-Helfer $((n+1)): HAENGT (nach 60 s beendet)"
    f=/tmp/c02_load_$((n+1)).out
    echo "Last-Helfer $((n+1)): $(grep -E '^send.start' $f | cut -c1-70) | $(grep -E '^send.stop' $f | cut -c1-80) | $(grep -E '^EOF' $f)"
  done
  kill "$guard" 2>/dev/null
  pkill -f "BeamerTeleport --ipc" 2>/dev/null
  sleep 2
done
echo LOAD_DONE
