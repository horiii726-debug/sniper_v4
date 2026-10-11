#!/usr/bin/env bash
# V5 Phase 1 two-tier fetch for windows of config/sample_windows.json. Per window:
#   getBlock backfill (SLOTS slots ending at end_slot) -> G0.H1 check -> decode to the window event cache
#   -> delete the raw JSON segments (disk) -> follow every HOT coin for HOURS -> export trades (window + follow).
# Resumable: finished steps are skipped. Historical data only (no live recording).
# Usage: scripts/v5_fetch.sh [ROLE] [MAX_WINDOWS] [WINDOW_NAMES...]
#   ROLE = train (default) | final_holdout | all. With names given, they run in the given order.
# Env: SLOTS (default 16500 ~ 1 h at 0.218 s/slot), HOURS (10), RPS (12), CONC (10 getBlock in flight),
#      KEEP_RAW (1 = keep raw JSON segments for later multi-venue decoding; 0 = delete after decode).
set -u
cd "$(dirname "$0")/.."
ROLE=${1:-train}
MAX=${2:-1000}
shift 2 2>/dev/null || shift $#
SLOTS=${SLOTS:-16500}
HOURS=${HOURS:-10}
RPS=${RPS:-12}
CONC=${CONC:-10}
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
n=0
while read -r name end role; do
  [ -z "$name" ] && continue
  [ "$ROLE" != all ] && [ "$role" != "$ROLE" ] && continue
  [ $n -ge "$MAX" ] && break
  n=$((n + 1))
  d=data/v5_$name
  if [ -f $d/trades_full.done ]; then echo "$name already done"; continue; fi
  if [ ! -f $d/pump_events_v2.bin ]; then
    if [ ! -f $d/summary.json ]; then
      rm -rf $d
      $BIN backfill --out $d --end-slot "$end" --slots "$SLOTS" --concurrency "$CONC" > logs/v5_$name.backfill.out 2>&1 || { echo "$name backfill FAILED"; continue; }
    fi
    $BIN verify-backfill --rec $d > $d/verify_backfill.json 2>/dev/null
    h1=$(python3 -c "import json;print(json.load(open('$d/verify_backfill.json'))['G0_H1'])" 2>/dev/null || echo ERR)
    if [ "$h1" != PASS ]; then echo "$name G0.H1 $h1 -> not decoding"; continue; fi
    # the raw decode also writes creations.bin.zst (authoritative newborns); re-scan if a cache existed without it
    $BIN export-events --rec $d --out $d/trades.csv > /dev/null 2>&1 || { echo "$name decode FAILED"; continue; }
    [ -f $d/creations.bin.zst ] || $BIN scan-creations --rec $d > /dev/null 2>&1 || { echo "$name creation scan FAILED (raw kept)"; continue; }
    [ "$KEEP_RAW" = 0 ] && rm -f $d/raw-*.zst
  fi
  if [ ! -f $d/follow_summary.json ]; then
    $BIN follow --rec $d --hours "$HOURS" --rps "$RPS" > logs/v5_$name.follow.out 2>&1 || { echo "$name follow FAILED"; continue; }
  fi
  $BIN export-events --rec $d --out $d/trades.csv > /dev/null 2>&1 && touch $d/trades_full.done
  echo "$name done: $(python3 -c "import json;s=json.load(open('$d/follow_summary.json'));b=json.load(open('$d/summary.json'));print('getBlock',b['calls']['getBlock'],'| follow',s['calls'],'| hot',s['hot_coins_in_window'],s['status'])")"
done <<< "$list"
echo ALL_DONE
