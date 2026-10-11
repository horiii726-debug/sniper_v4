#!/usr/bin/env bash
# Re-decode V6 windows from data/rawkeep (hardlinked raw segments) with the current decoder, then rebuild the 1 h
# snapshots. Usage: scripts/v6_redecode.sh NAME... (window names, e.g. s05d_11h). Raw links are removed afterwards.
set -u
cd "$(dirname "$0")/.."
BIN=./target/release/sniper
for n in "$@"; do
  d=data/v5_$n
  k=data/rawkeep/v5_$n
  [ -f $d/decision_window.json ] && [ -d $k ] || { echo "$n: missing window or raw"; continue; }
  for f in $k/raw-*.zst; do [ -e $d/$(basename $f) ] || ln $f $d/$(basename $f); done
  rm -f $d/pump_events_v4.bin $d/pump_events_v3.bin $d/creations.bin.zst $d/snapshots_v5.done
  $BIN export-events --rec $d --out $d/trades.csv > logs/redecode_$n.out 2>&1 || { echo "$n: decode FAILED"; continue; }
  $BIN --label-horizon-s 3600 snapshots --rec $d >> logs/redecode_$n.out 2>&1 || { echo "$n: snapshots FAILED"; continue; }
  touch $d/snapshots_v5.done
  rm -f $d/raw-*.zst
  echo "$n: re-decoded $(date -u +%FT%TZ)"
done
