#!/usr/bin/env bash
# Fetch every window of config/sample_windows.json (train + final holdout, config order) until the Helius quota ends.
# Window 1 (s02d_14h) keeps its raw segments (decoder re-validation sample); the others drop raw after decode
# (disk). Decision window SLOTS=8250 (config slots_per_window), extension EXT_SLOTS=10000 (> LABEL_H=3600 s of slots).
# Stops after 3 consecutive backfill failures (quota exhausted or plan error).
set -u
cd "$(dirname "$0")/.."
KEEP_RAW=1 SLOTS=16500 EXT_SLOTS=16500 scripts/v6_fetch.sh all 1 s02d_14h
fails=0
for name in $(python3 -c "import json;[print(w['name']) for w in json.load(open('config/sample_windows.json'))['windows']]"); do
  [ "$name" = s02d_14h ] && continue
  out=$(KEEP_RAW=0 SLOTS=8250 EXT_SLOTS=10000 scripts/v6_fetch.sh all 1 "$name" 2>&1 | tail -2 | head -1)
  echo "$(date -u +%FT%TZ) $out"
  case "$out" in *FAILED*|*G0.H1*) fails=$((fails + 1));; *) fails=0;; esac
  [ $fails -ge 3 ] && { echo "3 consecutive failures: stopping (quota?)"; break; }
done
echo FETCH_ALL_DONE
