#!/bin/bash
# M0-Test (Mac): WLAN-Daten anzeigen und Ping-Statistik messen. Ändert nichts am System.
# Aufruf: ./mac_netz.sh <PC-IP> [Sekunden=60] [Fritzbox-IP=192.168.178.1]
set -u
PC="${1:?PC-IP angeben, z. B. ./mac_netz.sh 192.168.178.50}"
SECS="${2:-60}"; GW="${3:-192.168.178.1}"
echo "== WLAN (Mac) =="
system_profiler SPAirPortDataType 2>/dev/null | awk '/Current Network Information/{f=1} f&&/PHY Mode|Channel|Signal \/ Noise|Transmit Rate|MCS Index/{print "  "$0} /Other Local Wi-Fi Networks/{exit}'
echo "== AWDL =="; ifconfig awdl0 2>/dev/null | awk '/status|flags/{print "  "$0}'
stats() { # liest ms-Werte von stdin, gibt Perzentile aus
  sort -n | awk 'function q(p,  i){i=int(NR*p+0.999); if(i<1)i=1; if(i>NR)i=NR; return a[i]}
    {a[NR]=$1} END{if(NR==0){print "  keine Antworten"; exit}
    printf "  n=%d  min=%.1f  p50=%.1f  p95=%.1f  p99=%.1f  max=%.1f ms\n", NR, a[1], q(0.5), q(0.95), q(0.99), a[NR]}'
}
run() { # ziel name
  local n=$((SECS*5)) out
  out=$(ping -i 0.2 -c "$n" "$1" 2>&1) || true
  if echo "$out" | grep -q "only.*super-user\|Operation not permitted"; then out=$(ping -i 1 -c "$SECS" "$1" 2>&1) || true; fi
  echo "== Ping $2 ($1) =="
  echo "$out" | grep -Eo 'time=[0-9.]+' | cut -d= -f2 | stats
  echo "$out" | grep -E 'packet loss' | sed 's/^/  /'
}
run "$GW" "Fritzbox" &
run "$PC" "PC" &
wait
