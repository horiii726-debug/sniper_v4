# V5 upgrade — status report (EXPLORATORY, pre-gate, paper only)

Snapshot: 2026-10-10 ~22:25 UTC, branch `v5-upgrade`. Plan: `V5_UPGRADE_PLAN.md`. V5 runs as exploratory research
with the user's explicit approval (2026-10-10) despite the CLAUDE.md stage-gate rule (Gate 0 is still BLOCKED).
No live trading, no wallet keys; everything is backtest on historical Helius data.

## Ringkasan (Bahasa Indonesia)

- **Sudah jadi dan dites:** `sniper follow` (data 10 jam setelah HOT), label V5 (horizon 10 jam, sensor), 35 fitur
  baru (termasuk ukuran buy/sell, kecepatan trader, konsentrasi terhadap total supply), skor wallet offline,
  crate Rust `model` (CatBoost + isotonic, parity Rust–Python lulus), strategi V5 (entry EV, slot, exit bertahap
  TP1/TP2/trailing, tanpa jual paksa akhir window), evaluasi walk-forward + grid 24 + sensitivitas + Monte Carlo.
- **Hasil yang sudah dites (2 window V4 lama, 33 menit, tanpa ML, tanpa data follow):** V4 $100 → $50,12 dan $49,79;
  V5 tanpa ML $100 → **$105,00** dan $78,97. Exit V5 memegang runner 12,5x sampai 12,3x (+258%), V4 menjualnya di 1,0x.
  Belum terbukti menang: hanya 2 window pendek, satu positif satu negatif, runner tertangkap 1 dari 20.
- **Belum jadi:** pengambilan ≥ 20 window (window 1 sedang diunduh), training model V5 di GPU, hasil walk-forward
  V5 penuh, grid, Monte Carlo, holdout, cek penerimaan.
- **Masalah utama:** rate limit Helius Free (~5 slot/detik, 1 window 1 jam ≈ 50+ menit + follow), seleksi sinyal
  (radar ~2.000 sinyal per 33 menit, runner ~1%) butuh model, data mentah terlalu besar untuk GitHub.

## 1. Done (code + tests)

| Phase | Item | Where | Tested |
|---|---|---|---|
| 1 | `sniper follow --rec data/X --hours 10`: per HOT coin, `getSignaturesForAddress` on the mint (newest → window end), `getTransaction` oldest first, same `PumpDecoder`, dead-coin stop (no real trade 30 min AND price < 0.1·peak), global rate limiter, per-method call counters | `crates/feed/src/follow.rs`, `crates/app/src/follow.rs` | unit tests (scan paging, merge, dead rule) |
| 1 | Real creation time + creation price from the mint's first transaction (`CreateEvent` / `CreatePoolEvent`) | `crates/decoder/src/pump.rs::decode_creations` | via follow (needs live run) |
| 1 | `backtest::load` merges `follow.bin.zst` (unique intra-slot order keys, wallet ids mapped by address); old V4 caches (`pump_events.bin`, no wallet table) still load | `crates/app/src/backtest.rs` | unit test + loaded `datasets/bf_1h`, `datasets/h1d` |
| 1 | Pipeline script: backfill → G0.H1 → decode → delete raw → follow → export; resumable | `scripts/v5_fetch.sh` | running on window 1 |
| 1 | Versioned copy of decoded Helius data per window (split < 95 MB) | `scripts/v5_save_datasets.sh` | syntax only (no finished window yet) |
| 2 | Labels over H = 36,000 s: `y_tp1`, `y_bin` (7 classes, −65% barrier), `y_peak_barrier`, `y_hold`, `y_dead_10m`, `y_logmax`, `y_end_h`, `y_rugdead`, `y_t2x`; censoring rule §3.2 with dropped-row counts; `is_signal` (radar Promote, forced snapshot) and `in_window`; adaptive snapshot spacing 2 s / 10 s / 30 s | `crates/app/src/ml_sim.rs` | unit tests on hand-made paths, censoring, death times |
| 3 | `age_s`, `newborn`, `mult_since_first` from the real creation; 35 new features appended (V4's 72 keep their indices): smart wallets ×6, crowd growth ×3, size distribution ×8 (p90 buy/sell, median sell, whales, retail, HHI, buy/sell vs liquidity), order flow ×4 (OFI, CVD, Kyle λ, 1 s vol), overhang ×2, curve ×2, regime ×4, trader speed ×3, `created_known`, `log_quote_res`, `top10_supply_share` | `crates/app/src/ml_sim.rs` | causality test (prefix vs full stream identical) |
| 3 | Offline wallet scores (shrunk mean log return per closed trip, bot / flipper / sniper flags, co-buying clusters), strictly before each window start | `python/research/wallet_scores.py` | 15 tests; adversarial review found 2 real bugs (cross-window duplicates, oversell) → fixed |
| 4 | Walk-forward GPU training (CatBoost on both GPUs, GPU random forest via XGBoost CUDA, CPU RF, logistic baseline, isotonic calibration, purge ≥ H) | `python/ml/v5_train.py`, `v5_common.py` | being built + reviewed by a workflow at snapshot time (WIP) |
| 5 | Rust CatBoost JSON evaluator + isotonic tables + feature map | `crates/model` | 16 tests incl. 10,000-row parity vs Python |
| 6 | V5 strategy: EV entry at in-window signal rows, filters, slot replacement by hold-EV, max entries/min, exposure cap, PRE_TP stop with grace, TP1/TP2 partial sells, trailing (max(bag_trail, k·vol), +0.1 while P(hold) ≥ hold_min), always-on exits (rug, dev sell fraction, LP drop, max hold, hazard), `DataEnd` instead of `EndOfWindow` | `crates/app/src/strategy_v5.rs` | unit tests (ladder timing/units, EV gate) |
| 7 | Costs: pool fee, base + priority + tip per tx (partial sells pay their own), buy failures with seed, latency | `strategy_v5.rs::CostsV5` | in unit tests |
| 8 | `sniper v5-backtest`: walk-forward on fold scores, R_k from each fold's training windows (their own OOS scores), 24-point grid selected by MEDIAN return, sensitivity (latency 1/4, fail 0.30, tip ×2, latency4+fail0.30), acceptance check (5 criteria), Monte Carlo (execution-seed reruns, trade bootstrap, window bootstrap) → `reports/V5_RESULTS.{json,md}` | `crates/app/src/v5_eval.rs` | compiles; needs fold scores (no V5 data yet) |
| — | `sniper v5-compare`: V4 rule strategy vs V5 corrections without ML, per-runner table | `v5_eval.rs::compare` | run on 2 V4 windows (below) |

Test totals at snapshot: app 12 passed, model 16 passed, decoder/feed/events/others unchanged and passing, wallet scores 15 passed.

## 2. Results so far (only tested numbers)

### 2.1 V4 vs V5 corrections without ML (`reports/V5_COMPARE_v4windows.json`)

Data: the only two windows available on this VPS (`datasets/bf_1h`, `datasets/h1d`, V4 format, ~33 min each,
no follow-up data, no wallet addresses). Same costs (latency 2 slots, 15% buy failure, 0.0005 SOL/tx, pool fees), $100 start.

| Window | Strategy | $100 → | PnL | Trades | Win rate | Profit factor | 10x runners | held ≥ 5x |
|---|---|---|---|---|---|---|---|---|
| bf_1h | V4 (frozen rule params) | $50.12 | −$49.88 | 156 | 18% | 0.68 | 11 | 1 |
| bf_1h | V5 no ML | **$105.00** | **+$5.00** | 15 | 20% | **1.41** | 11 | 1 |
| h1d | V4 | $49.79 | −$50.21 | 157 | 25% | 0.64 | 9 | 1 |
| h1d | V5 no ML | $78.97 | −$21.03 | 18 | 22% | 0.22 | 9 | 0 |

- V5's exit ladder held `ATcNwcCC` (12.5x after signal) through TP1 → TP2 → trail to 12.3x (+258% on the position);
  V4 entered the same coin and hard-stopped at 1.0x.
- Not a proof that V5 wins: 2 short windows, one positive one negative; 11 of 15 V5 positions on bf_1h were closed by
  `DataEnd` (window data ends after 33 min) — exactly what the follow-up data fixes.

### 2.2 Why radar-detected runners were missed (per-coin diagnosis on the same windows)

1. **V4:** slots full of weak coins (`max_positions` blocked the 37x runner 41 times), `dev_sold_recently`, rule filters;
   exits killed winners (37x runner sold at 4.1x by `Mee`, others `HardStop` at ~1.0x).
2. **V5 without a model:** the radar emits ~1,900–2,700 signals per 33 min and only ~1% become ≥ 10x. First-come entries
   fill the exposure cap; runners are then blocked by `exposure_cap` / `max_entries_per_min`. The missing piece is
   **selection** (the EV model), not detection.
3. **Short windows:** positions cannot be held to the move (forced `DataEnd`/`EndOfWindow`).

### 2.3 Bugs found and fixed during testing

- Plan filter `max_top10_share` applied to V4's `f2_top10_holder_share` (share of OBSERVED holdings, ≈ 1 at the first
  signal) blocked 98% of signals including every runner → new feature `top10_supply_share` (vs total supply) used by the filter.
- Price-unit bug in the V5 entry price (SOL vs lamports per token) made TP1/TP2 fire instantly → fixed; the test now
  asserts TP1/TP2 timing and the multiple's range.
- Wallet scores: cross-window duplicate trades (follow-up rows carry synthetic intra-slot keys) and oversells
  (tokens bought elsewhere) → fixed with regression tests.

### 2.4 Helius fetch measurements (window 1 = `s02d_14h`, 16,500 slots ≈ 1 h, in progress)

- Throughput ≈ 5.6 → 4.7 slots/s with ~35–40% `getBlock` retries (rate limited): ≈ 55–60 min per 1 h window backfill.
- Raw JSON on disk ≈ 2.75 GB at 78% (≈ 3.5 GB per window), ≈ 52 GB JSON transferred at 78%.
- Follow-up cost (getSignaturesForAddress / getTransaction counts) and Helius credits per window: not measured yet.

## 3. Not done yet

1. Fetch ≥ 20 train windows + 3 `final_holdout` windows (window 1 running; the rest queued after the cost report).
2. Run `wallet_scores.py`, `sniper snapshots` on real V5 windows.
3. Train V5 models on GPU (`v5_train.py`) — needs ≥ 4 windows with follow-up data.
4. Wire Rust inference (`crates/model`) into `strategy_v5` as a `Scorer` (score files from Python work already).
5. Walk-forward V5 backtest, grid, sensitivity, Monte Carlo, acceptance → `reports/V5_RESULTS.md`.
6. Holdout run once after freezing (§9).
7. Feature-group ablation on strategy expectancy (§4 last paragraph).
8. Stage-gate work from CLAUDE.md (Gate 0 live criteria, Stage 1 sub-gates) — untouched by V5.

## 4. Problems / risks / deviations (complete list)

1. **Helius Free plan rate limit** — the bottleneck of the whole project now (≈ 1 h of wall time per 1 h window plus
   follow-up). Credits per window unknown until read from the dashboard (Usage) before/after a window. A paid plan
   would speed this up far more than hardware.
2. **Gate 0 BLOCKED** (no mainnet LaserStream on the Free plan) — V5 is exploratory by user decision.
3. **No V5 window finished yet** → no V5 models, no V5 walk-forward numbers. Only the V4-window comparison exists.
4. **The previous VPS's `data/` (28 V4 windows) is not on this machine**; only 2 V4 windows are versioned in `datasets/`.
5. **Raw Helius data cannot go to GitHub** (≈ 3.5 GB per window; 100 MB file limit). The decoded data (all
   pump.fun / PumpSwap trades + follow-up) is versioned via `scripts/v5_save_datasets.sh`. Other venues' raw txs are not kept.
6. **Signal selection:** ~1% of radar signals are ≥ 10x runners; without the EV model capture stays near zero.
7. **Deviation:** follow-up scans the MINT address (covers curve, migration and pool in one stream) instead of curve + pool.
8. **Deviation:** R_k is measured NET of costs, so EV does not subtract the roundtrip cost a second time.
9. **Deviation:** `hawkes_branching` uses a moment estimator (Fano factor) instead of an MLE fit (speed, causality).
10. **Deviation:** wallet scores are per-window CSV files (`<dir>/wallet_scores.csv`) instead of one `data/wallet_scores.bin`.
11. Regime features are −1 after the window end (only followed coins have data there); entries only inside the window.
12. Follow-up intra-slot order comes from the signature list position (getTransaction has no tx index).
13. Creation time is unknown for very active old coins when the history scan hits its page cap (`created_known` = 0).
14. Rust-side rug probability uses CatBoost only; the CatBoost + RF average exists only in Python score files.
15. **V4 regressions to be aware of:** `sniper snapshots` now writes `snapshots_v5.csv.zst` (V5 labels) instead of the
    V4 `snapshots.csv`, so the V4 Python scripts (`nested_lowo.py` etc.) need the old binary/commit; snapshot cadence
    changed (adaptive + forced on Promote), so old V4 score files keyed by event order may not match new snapshot rows.
    The V4 rule backtest and simulator code are unchanged.
16. Open decision: bot flag has no minimum trip count (≈ 75% of flagged bots have ≤ 2 closed trips).
17. `crates/model` and `python/ml/v5_train.py` were still under adversarial review when this snapshot was committed.
18. Plan starting value `max_top10_share = 0.5` assumed a supply-based concentration; see 2.3.
19. Market-wide features before the window start are warm-up limited (deques start empty at the window start).

## 5. How to continue

```
scripts/v5_fetch.sh train 30                      # all train windows (resumable)
scripts/v5_fetch.sh final_holdout 3               # holdouts (do not evaluate before Phase 8)
python3 python/research/wallet_scores.py data/v5_*
./target/release/sniper snapshots --rec data/v5_A --rec data/v5_B ...
python3 python/ml/v5_train.py data/v5_A data/v5_B ...           # both GPUs
./target/release/sniper v5-backtest --rec data/v5_A ... --grid --mc-runs 2000
scripts/v5_save_datasets.sh                       # version the decoded Helius data
```
