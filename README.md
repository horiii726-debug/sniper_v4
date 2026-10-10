# Solana DEX Sniper — V4 (research build)

Speed-first Solana memecoin system built to the spec `SOLANA_DEX_SNIPER_V4_FINAL.md`, with stage-gated
development (`CLAUDE.md`, `gates/`). **No live trading.** Everything below runs on recorded or backfilled
mainnet data.

## Status (2026-10-10)

| Area | State |
|---|---|
| Stage 0 feed (LaserStream gRPC + WS fallback, recorder, deterministic replay, gap tracking) | Built. Historical gate G0.H1/H2/G0.3/G0.4 measured PASS; live-only criteria deferred (Helius Free plan has no mainnet gRPC) |
| Decoders | Balance-delta reference decoder (all venues); IDL-driven pump.fun + PumpSwap event decoder: side 100%, amounts 99.94% vs reference; pool math validated (curve 99.96%, PumpSwap 98.8% exact to 0.1%) |
| Radar V4 (newborn auto-HOT + awakening rules) | Frozen params caught 98–100% of newborn and 92–94% of established runners before +30% on unseen windows |
| ML (CatBoost GPU; CatBoost+RandomForest ensemble compared) | Entry P(peak >= 2x / 5x in 30 min), continuation, rug (dies and never recovers), dump-in-30 s, dev-dump models. Dump / dev-dump models AUC 0.89–0.96 out of sample |
| Strategy / $100 backtest | **Exploratory, not profitable yet.** See `reports/`. Main blocker: too little history (4 windows x ~35 min) to learn rare moonshots |

## Layout

```
crates/config     env loader, secret redaction
crates/latency    ns clocks, histograms
crates/events     raw messages, normalization, dedup, slot-gap tracker, pipeline
crates/recorder   crash-tolerant zstd recorder / reader
crates/feed       LaserStream gRPC, Solana WS fallback, JSON-RPC backfill
crates/replay     deterministic replay, latency / recovery reports
crates/decoder    balance-delta reference decoder, IDL borsh reader, pump.fun / PumpSwap events
crates/radar      rule-based radar (V4 §7–§8, §11 subset)
crates/app        `sniper` CLI: record, backfill, replay, verify-*, census, radar-*, backtest*, snapshots, ml-*
python/           offline research: tier analysis, forensics, CatBoost training, ablation, nested CV
config/           venue registry (official sources), official IDLs, frozen radar / strategy params
gates/            stage gate reports
reports/          backtest and model reports (exploratory)
```

## Main commands

```
cargo build --release
./target/release/sniper backfill --out data/X --slots 8250            # historical mainnet window
./target/release/sniper verify-backfill --rec data/X                   # G0.H1
./target/release/sniper replay --rec data/X --out derived.txt          # G0.3 determinism
./target/release/sniper snapshots --rec data/X                         # causal features + labels
python3 python/ml/nested_lowo.py data/A,data/B,data/C,data/D           # CatBoost GPU scores
./target/release/sniper backtest-lowo --rec data/A --rec data/B ...    # $100 per window, nested CV
```

## Secrets

Keys come from `.env` only (`.env.example` lists names). `.env`, keypairs, `data/` and `logs/` are git-ignored.
`scripts/ci.sh` fails if a key appears in logs or tracked files.
