#!/bin/bash
# M0-Test (Mac): AWDL (AirDrop/Handoff/Sidecar/Universal Control) vorübergehend abschalten.
# status | down | down --watch (hält es aus, bis Strg+C; dann wieder an) | up
# Wirkt nur bis zum Neustart. Während "down" funktionieren AirDrop, Handoff, Sidecar, Universal Control nicht.
set -u
case "${1:-status}" in
  status) ifconfig awdl0 | awk '/flags|status/{print}';;
  up)     sudo ifconfig awdl0 up && echo "AWDL an";;
  down)
    sudo ifconfig awdl0 down && echo "AWDL aus"
    if [ "${2:-}" = "--watch" ]; then
      trap 'sudo ifconfig awdl0 up; echo; echo "AWDL wieder an"; exit 0' INT TERM
      echo "Halte AWDL aus. Beenden mit Strg+C."
      while true; do ifconfig awdl0 | grep -q "<UP" && sudo ifconfig awdl0 down && echo "$(date +%T) AWDL war wieder an -> aus"; sleep 2; done
    fi;;
  *) echo "Aufruf: $0 status|down [--watch]|up"; exit 1;;
esac
