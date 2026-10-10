#!/usr/bin/env bash
# Backfill the sampled windows in config/sample_windows.json. Per window: getBlock backfill -> G0.H1 check ->
# decode to the compact pump event cache + snapshots -> delete the raw JSON segments (disk). Resumable.
set -u
cd /root/sniper_v4
python3 -c "import json;[print(w['name'],w['end_slot']) for w in json.load(open('config/sample_windows.json'))['windows']]" | while read name end; do
  d=data/$name
  [ -f $d/pump_events_v2.bin ] && [ -f $d/snapshots.csv ] && continue
  if [ ! -f $d/summary.json ]; then
    rm -rf $d
    ./target/release/sniper backfill --out $d --end-slot $end --slots 8250 --concurrency 4 > logs/$name.out 2>&1 || { echo "$name backfill FAILED"; continue; }
  fi
  ./target/release/sniper verify-backfill --rec $d > $d/verify_backfill.json 2>/dev/null
  ./target/release/sniper snapshots --rec $d >> logs/$name.out 2>&1 || { echo "$name snapshots FAILED"; continue; }
  rm -f $d/raw-*.zst
  echo "$name done $(python3 -c "import json;print(json.load(open('$d/verify_backfill.json'))['G0_H1'])")"
done
echo ALL_DONE
