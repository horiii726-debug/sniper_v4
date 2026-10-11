#!/usr/bin/env bash
# Fetch every window of config/sample_windows.json (train + final holdout, config order) until the Helius quota ends.
# RPC fetching (window + extension backfill) runs in this loop; the CPU work of each fetched window (G0.H1 checks,
# extend-window, decode of all venues, snapshots) runs in the background through scripts/v6_fetch.sh, so the RPC is
# never idle. Window 1 (s02d_14h) keeps its raw segments (decoder re-validation sample); the others drop raw after
# decode (disk). Decision window SLOTS=8250 (config slots_per_window), extension EXT_SLOTS=10000 (> LABEL_H=3600 s).
# Stops after 3 consecutive fetch failures (quota exhausted or plan error).
set -u
cd "$(dirname "$0")/.."
BIN=./target/release/sniper
CONC=${CONC:-10}
process() { KEEP_RAW=$2 SLOTS=$3 EXT_SLOTS=$4 scripts/v6_fetch.sh all 1 "$1" > "logs/process_$1.out" 2>&1; echo "$(date -u +%FT%TZ) processed $1: $(tail -2 logs/process_$1.out | head -1)"; }
fetch() { # dir end_slot slots log
  [ -f "$1/summary.json" ] && return 0
  rm -rf "$1"
  $BIN backfill --out "$1" --end-slot "$2" --slots "$3" --concurrency "$CONC" > "$4" 2>&1
}
[ -f data/v5_s02d_14h/snapshots_v5.done ] || process s02d_14h 1 16500 16500 &
fails=0
while read -r name end; do
  [ "$name" = s02d_14h ] && continue
  d=data/v5_$name
  x=data/v5_${name}_ext
  [ -f $d/snapshots_v5.done ] && continue
  ok=1
  fetch $d "$end" 8250 logs/v5_$name.backfill.out || ok=0
  if [ $ok = 1 ] && [ ! -f $d/decision_window.json ]; then
    fetch $x "$((end + 10000))" 10000 logs/v5_${name}_ext.backfill.out || ok=0
  fi
  if [ $ok = 1 ]; then
    fails=0
    echo "$(date -u +%FT%TZ) fetched $name"
    process "$name" 0 8250 10000 &
  else
    fails=$((fails + 1))
    echo "$(date -u +%FT%TZ) fetch FAILED $name (consecutive $fails)"
    [ $fails -ge 3 ] && { echo "3 consecutive fetch failures: stopping (quota?)"; break; }
  fi
done < <(python3 -c "import json;[print(w['name'],w['end_slot']) for w in json.load(open('config/sample_windows.json'))['windows']]")
wait
echo FETCH_ALL_DONE
