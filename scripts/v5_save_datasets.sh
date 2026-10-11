#!/usr/bin/env bash
# Copy the decoded Helius data of every finished V5 window (data/v5_<name>/) into the versioned
# datasets/v5_<name>/ directory, zstd-compressed and split into < 95 MB parts (GitHub file limit).
# Raw getBlock JSON (raw-*.zst, ~3.7 GB per 1 h window) is NOT versioned: it is deleted after decoding, and
# every pump.fun / PumpSwap trade in it is kept in the decoded files below.
#   pump_events_v2.bin  window events (all coins, all trades, wallet table)   -> pump_events_v2.bin.zst[.partNN]
#   follow.bin.zst      10 h follow-up of every HOT coin                      -> follow.bin.zst[.partNN]
#   trades.csv          window + follow trades, one row per trade (export)    -> trades.csv.zst[.partNN]
#   *.json              backfill / verify / follow / snapshot summaries
# Restore: cat X.partNN... > X (in name order), then zstd -d where needed. See datasets/README.md.
set -euo pipefail
cd "$(dirname "$0")/.."
LIMIT=$((95 * 1024 * 1024))
put() { # src dst: copy (already compressed) file, split when over the limit
  local src=$1 dst=$2
  rm -f "${dst:?}" "${dst:?}".part*
  if [ "$(stat -c %s "$src")" -gt $LIMIT ]; then
    split -b 95m -d -a 2 "$src" "$dst.part"
  else
    cp "$src" "$dst"
  fi
}
for d in data/v5_*/; do
  d=${d%/}
  name=$(basename "$d")
  [ -f "$d/pump_events_v2.bin" ] || { echo "$name: not decoded yet, skipped"; continue; }
  out=datasets/$name
  mkdir -p "$out"
  for j in summary.json verify_backfill.json follow_summary.json snapshots_v5_stats.json wallet_scores_meta.json; do
    [ -f "$d/$j" ] && cp "$d/$j" "$out/$j"
  done
  [ -f "$d/summary.json" ] && cp "$d/summary.json" "$out/backfill_summary.json" && rm -f "$out/summary.json"
  tmp=$(mktemp -d)
  zstd -q -19 -T0 -f "$d/pump_events_v2.bin" -o "$tmp/pump_events_v2.bin.zst"
  put "$tmp/pump_events_v2.bin.zst" "$out/pump_events_v2.bin.zst"
  [ -f "$d/follow.bin.zst" ] && put "$d/follow.bin.zst" "$out/follow.bin.zst"
  [ -f "$d/creations.bin.zst" ] && put "$d/creations.bin.zst" "$out/creations.bin.zst"
  if [ -f "$d/trades.csv" ]; then
    zstd -q -19 -T0 -f "$d/trades.csv" -o "$tmp/trades.csv.zst"
    put "$tmp/trades.csv.zst" "$out/trades.csv.zst"
  fi
  [ -f "$d/wallet_scores.csv" ] && zstd -q -19 -T0 -f "$d/wallet_scores.csv" -o "$out/wallet_scores.csv.zst"
  rm -rf "${tmp:?}"
  echo "$name: $(du -sh "$out" | cut -f1) -> $out"
done
