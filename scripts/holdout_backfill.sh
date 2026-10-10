#!/usr/bin/env bash
# Holdout periods (never used to build the radar): ~1 day, ~7 days, ~30 days before NOW_SLOT=455087899.
# 0.2173 s/slot measured -> 397,600 slots/day. Each window 8,250 slots (~30 min chain time).
set -u
cd /root/sniper_v4
for spec in "h1d 454690299" "h7d 452304699" "h30d 443159899"; do
  set -- $spec
  [ -f data/$1/summary.json ] && continue
  rm -rf data/$1
  ./target/release/sniper backfill --out data/$1 --end-slot $2 --slots 8250 --concurrency 3 > logs/$1.out 2>&1
done
echo ALL_DONE
