#!/usr/bin/env bash
# V6 data pipeline per window of config/sample_windows.json (historical getBlock backfill only, no follow-up):
#   window backfill (SLOTS) -> extension backfill of the next EXT_SLOTS -> G0.H1 checks -> extend-window
#   (decisions in the window, labels over window + extension, every coin and venue) -> decode (all venues,
#   creations) -> snapshots with LABEL_H labels -> optional raw deletion.
# Resumable: finished steps are skipped. Usage: scripts/v6_fetch.sh [ROLE] [MAX_WINDOWS] [WINDOW_NAMES...]
# Env: SLOTS (16500 ~ 1 h), EXT_SLOTS (16500), CONC (10), LABEL_H (3600), KEEP_RAW (1).
set -u
cd "$(dirname "$0")/.."
ROLE=${1:-train}
MAX=${2:-1000}
shift 2 2>/dev/null || shift $#
SLOTS=${SLOTS:-16500}
EXT_SLOTS=${EXT_SLOTS:-16500}
CONC=${CONC:-10}
LABEL_H=${LABEL_H:-3600}
KEEP_RAW=${KEEP_RAW:-1}
BIN=./target/release/sniper
mkdir -p logs data
if [ $# -gt 0 ]; then
  list=$(python3 -c "
import json,sys
w={x['name']:x for x in json.load(open('config/sample_windows.json'))['windows']}
[print(n,w[n]['end_slot'],w[n]['role']) for n in sys.argv[1:]]" "$@")
else
  list=$(python3 -c "import json;[print(w['name'],w['end_slot'],w['role']) for w in json.load(open('config/sample_windows.json'))['windows']]")
fi
h1() { $BIN verify-backfill --rec "$1" > "$1/verify_backfill.json" 2>/dev/null; python3 -c "import json;print(json.load(open('$1/verify_backfill.json'))['G0_H1'])" 2>/dev/null || echo ERR; }
n=0
while read -r name end role; do
  [ -z "$name" ] && continue
  [ "$ROLE" != all ] && [ "$role" != "$ROLE" ] && continue
  [ $n -ge "$MAX" ] && break
  n=$((n + 1))
  d=data/v5_$name
  x=data/v5_${name}_ext
  if [ -f $d/snapshots_v5.done ]; then echo "$name already done"; continue; fi
  if [ ! -f $d/summary.json ]; then
    rm -rf $d
    $BIN backfill --out $d --end-slot "$end" --slots "$SLOTS" --concurrency "$CONC" > logs/v5_$name.backfill.out 2>&1 || { echo "$name backfill FAILED"; continue; }
  fi
  [ "$(h1 $d)" = PASS ] || { echo "$name G0.H1 failed"; continue; }
  if [ ! -f $d/decision_window.json ]; then
    if [ ! -f $x/summary.json ]; then
      rm -rf $x
      $BIN backfill --out $x --end-slot "$((end + EXT_SLOTS))" --slots "$EXT_SLOTS" --concurrency "$CONC" > logs/v5_${name}_ext.backfill.out 2>&1 || { echo "$name extension FAILED"; continue; }
    fi
    [ "$(h1 $x)" = PASS ] || { echo "$name extension G0.H1 failed"; continue; }
    $BIN extend-window --rec $d --ext $x || { echo "$name extend FAILED"; continue; }
    rm -rf $x
  fi
  if [ ! -f $d/pump_events_v3.bin ]; then
    $BIN export-events --rec $d --out $d/trades.csv > logs/v5_$name.decode.out 2>&1 || { echo "$name decode FAILED"; continue; }
  fi
  $BIN --label-horizon-s "$LABEL_H" snapshots --rec $d > logs/v5_$name.snapshots.out 2>&1 || { echo "$name snapshots FAILED"; continue; }
  touch $d/snapshots_v5.done
  [ "$KEEP_RAW" = 0 ] && rm -f $d/raw-*.zst
  echo "$name done: $(python3 -c "import json;print(json.load(open('$d/venue_decode_stats.json'))['events_by_venue'])")"
done <<< "$list"
echo ALL_DONE
