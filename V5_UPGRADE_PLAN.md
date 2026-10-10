# sniper_v4 → V5 upgrade plan (instructions for Claude Code)

Place this file in the repo root next to `CLAUDE.md`. The user writes in Indonesian; reply in Indonesian, keep code,
comments and reports in English.

## 0. Rules for this plan

- Architecture stays: **Rust = feed, decoder, features, inference, strategy, execution** (hot path).
  **Python = offline only** (labels analysis, training, calibration). No LLM / AI-agent components anywhere.
- Every feature is computed by ONE Rust code path (`crates/app/src/ml_sim.rs::Engine`) for both training snapshots and
  the simulator. No future data in any feature (§27).
- Paper / backtest only. No live trading, no wallet keys.
- `CLAUDE.md` forbids ML and backtests before Gate 0 and Stage 1 pass. **Before starting, ask the user to confirm that
  this plan is allowed to run as exploratory research despite that rule.** Do not assume it.
  *(Confirmed by the user on 2026-10-10: run as exploratory research, paper/backtest only; stay in Rust.)*
- Work phase by phase. At the end of each phase: run `cargo test`, `scripts/ci.sh`, report measured numbers, ask the
  user before the next phase.

## 1. Diagnosis this plan fixes (measured in `reports/`)

1. Windows are 33–44 min, labels look 15–30 min ahead and are censored by the window end; positions are force-sold at
   `EndOfWindow`. A coin that ran 3898x peaked at the last second of its window.
2. The radar flags almost every ≥10x runner as HOT before +30%, but entries are blocked, mostly by `max_positions`
   (slots full of weak coins), then by rule filters (`few_buyers_60s`, `median_buy_too_big`, `buy_sell_ratio_too_high`).
3. Exits kill winners within seconds (Scout, MomentumExhausted, DumpPredicted, StrongReversal, GiveBack 35%, DevSell on
   any sell). No trade ever held to 5x.
4. Entry model target (`P(peak >= 2x in 30 min)`) does not match the exit policy; probabilities are uncalibrated
   (p2 = 0.5–0.9 but most trades never exceed 1.1x).
5. Thresholds tuned by a 120–216 point grid on 2 windows with 1–8 trades each (overfit).
6. `age_s`, `newborn`, `mult_since_first` are measured from the window start, not from the coin's real creation.

## 2. Phase 1 — Long data (two-tier backfill)

1. Keep `sniper backfill` for the full window (1–2 h, all trades of all coins).
2. New command `sniper follow --rec data/X --hours 10`:
   - for every coin that became HOT in the window, page `getSignaturesForAddress` on its bonding-curve account and its
     PumpSwap pool from the HOT time until HOT + 10 h (or until the coin is dead, see 3.2);
   - fetch each transaction, decode with the SAME decoder (`crates/decoder`), write `follow.bin.zst`;
   - `backtest::load` merges window + follow events into one time-ordered `Events`.
3. Real creation time: oldest signature of the bonding-curve account → `created_at` per mint.
4. Fetch ≥ 20 windows from `config/sample_windows.json` (keep its `final_holdout` windows untouched until Phase 8).
5. Measure and report Helius credits and disk per window first; ask the user how many windows to fetch.

## 3. Phase 2 — Labels (`ml_sim.rs::labels`, `write_snapshots`)

Horizon `H = 36000 s` (10 h), equal to `max_hold_s`.

3.1 Labels per snapshot at time t, p0 = first pool price at ≥ t + latency:
- `y_tp1` (triple barrier): 1 if price ≥ 2.0·p0 before ≤ 0.65·p0 and before t + H, else 0.
- `y_bin` (multiclass on peak multiple before the −65% barrier, within H):
  `0: rug <0.35 | 1: 0.35–1 | 2: 1–2 | 3: 2–5 | 4: 5–10 | 5: 10–50 | 6: ≥50`.
- `y_hold` (for open positions): 1 if price ≥ 2·p_t before ≤ 0.5·p_t within H.
- `y_dead_10m`: 1 if the coin dies within the next 10 min (hazard target).
- `y_logmax = ln(max price within H / p0)` (regression / ranking).

3.2 Censoring: keep a snapshot only if data covers [t, t + H] OR the coin is dead before the gap
(dead = no real trade for 30 min AND price < 0.1·peak). Report how many rows were dropped.

3.3 Decision rows: add column `is_signal = 1` on the radar `Promote` event and on every later re-trigger.
The entry model trains ONLY on `is_signal = 1` rows (meta-labeling: radar = primary signal, model = take/skip/size).

3.4 Sample weight per row = `1 / rows_of_that_coin` so busy coins do not dominate.

## 4. Phase 3 — Features (Rust `Engine::features`, same path for train and sim)

Fix: `age_s`, `newborn`, `mult_since_first` from `created_at` and the creation price.

Add (all causal, all expressed as logs or ratios to liquidity):

| Group | Feature | Formula |
|---|---|---|
| Smart wallets | `smart_buy_sol_60s` | Σ sol_i · [score_w > θ] over buys in 60 s |
| | `smart_share_60s` | smart_buy_sol_60s / buy_sol_60s |
| | `n_smart_buyers_300s` | distinct buyers with score_w > θ |
| | `flipper_share_60s` | buy SOL from wallets whose median hold < 60 s |
| | `sniper_share_early` | supply held by wallets that buy in the first 3 slots of many coins |
| | `cluster_buy_share` | buy SOL from wallets in a co-buying cluster (see below) |
| Crowd growth | `buyer_growth` | buyers_60s / (buyers in the previous 60 s + 1) |
| | `holder_vel_60s` | Δholders / 60 |
| | `hawkes_branching` | n = α/β fitted on the last 100 buy arrivals (λ(t) = μ + Σ α e^(−β(t−t_i))) |
| Size distribution | `hhi_buy_60s` | Σ (s_i / Σs)² |
| | `retail_share_60s` | buy SOL in 0.05–2 SOL trades / buy SOL |
| | `whale_buys_60s` | count of buys > 5 SOL |
| Order flow | `ofi_60s` | (buy_sol − sell_sol) / quote_reserve |
| | `cvd_300s` | cumulative (buy_sol − sell_sol) over 300 s / quote_reserve |
| | `kyle_lambda_60s` | OLS slope of Δln P on signed SOL flow, 60 s |
| | `vol_60s` | std of 1 s log returns over 60 s (also used for exits) |
| Overhang | `early_retention` | tokens still held by first 20 buyers / tokens they bought |
| | `profit_overhang` | Σ bal_w · max(P − avg_cost_w, 0) / Σ bal_w · P |
| Curve | `curve_speed` | Δprogress / Δt over 60 s |
| | `eta_migration_s` | (1 − progress) / curve_speed |
| Regime | `attention_share_5m` | coin volume 5 m / all-coin volume 5 m |
| | `new_coins_per_min`, `market_vol_10m`, `sol_ret_1h` | market-wide, same for all coins |

Wallet score (offline job `python/research/wallet_scores.py` → `data/wallet_scores.bin`, loaded by Rust):
- only trades CLOSED before the start of the window being scored (rolling, no leakage);
- `score_w = (n_w · mean_logret_w + k · mean_logret_all) / (n_w + k)`, k = 10;
- exclude wallets flagged as bots: median hold < 10 s on > 80% of trades, or > 50 coins/day.
- co-buying clusters: wallet × coin matrix A (1 if bought in first 5 min), C = AᵀA, keep pairs with ≥ 3 shared
  coins, connected components = clusters.

Re-run the existing ablation (`python/ml/ablation.py`) on the new groups; keep a group only if it improves
out-of-sample strategy expectancy, not just AUC.

## 5. Phase 4 — Models (Python, offline)

| Model | Target | Rows | Algorithm |
|---|---|---|---|
| Entry (meta-label) | `y_bin` multiclass + `y_tp1` binary | `is_signal = 1` | CatBoost, class weights, sample weights |
| Rug | `y_rugdead` | all rows | CatBoost + RandomForest (`class_weight="balanced"`), average of calibrated outputs |
| Hold | `y_hold` | rows while a position would be open | CatBoost |
| Hazard | `y_dead_10m` | one row per coin-minute | CatBoost (discrete-time hazard) |
| Baseline | `y_tp1` | `is_signal = 1` | Logistic regression (sanity check; CatBoost must beat it) |

- Calibration: isotonic regression fitted on inner out-of-sample folds; export as a piecewise-linear table.
- Validation: walk-forward by time, train on earlier days, test on later days, purge gap ≥ H between train and test.
- Report per model: PR-AUC, Brier score, reliability table (10 bins), precision of the top 1% for ≥ 10x.

## 6. Phase 5 — Rust inference (`crates/model`, new)

- Load CatBoost models exported with `save_model(format="json")`; evaluate oblivious trees in Rust.
- Load isotonic tables.
- Parity test: 10,000 rows, Rust vs Python, max abs difference ≤ 1e-6. Measure p50/p99 inference time per snapshot.
- `ml_sim.rs` uses Rust inference directly instead of score CSV files.

## 7. Phase 6 — Strategy (`ml_sim.rs::simulate`, `MlStrategy`)

7.1 Remove: `EndOfWindow` force-sell (replace with `max_hold_s`), `Scout`, `MomentumExhausted` (`p_cont`),
`DumpPredicted`, `StrongReversal`, `GiveBackFromPeak`, Kelly on `p_up2`, the large `tune()` grid,
the rule filters that block whales (`median_buy_too_big`, `buy_sell_ratio_too_high`) in the entry path.

7.2 Entry by expected value at signal rows:

```
EV = Σ_k P_cal(bin_k) · R_k − roundtrip_cost
R_k = mean realized return of the exit policy for coins in bin k, measured on training windows only
enter if EV ≥ ev_min, rug_p < rug_veto, ret_60s ≤ max_ret_60s_entry,
         top10_share ≤ max_top10_share, bundle_share ≤ max_bundle_share
```

7.3 Slots: rank candidates by EV. If slots are full and a candidate's EV exceeds the weakest open position's current
EV (from the hold model) by `replace_margin`, sell the weakest and enter. Limit `max_entries_per_min`.

7.4 Exit state machine (positions support partial sells):

```
PRE_TP  : stop if value ≤ (1 − pre_tp_stop)·cost, ignored for the first stop_grace_s seconds
          → TP1 at tp1_mult: sell tp1_sell_frac
BAG1    : → TP2 at tp2_mult: sell tp2_sell_frac
BAG1/2  : trailing exit if price ≤ peak · (1 − trail), trail = max(bag_trail, trail_k · vol_60s)
          hold-model override: while P(y_hold) ≥ hold_min, widen trail by 0.1
ALWAYS  : rug model ≥ rug_veto; creator sells ≥ dev_kill_min_frac of their balance;
          pool quote reserve drops > lp_drop_frac in 60 s (non-curve pools); age ≥ max_hold_s;
          hazard P(dead in 10 min) ≥ dead_exit
```

7.5 New `MlStrategy` parameters and starting values (tune only a few, see Phase 8):

```json
{ "ev_min": 0.15, "rug_veto": 0.5, "max_ret_60s_entry": 0.5, "max_top10_share": 0.5, "max_bundle_share": 0.3,
  "max_positions": 18, "replace_margin": 0.15, "max_entries_per_min": 3,
  "bet_sol": 0.05, "max_total_exposure": 0.6,
  "pre_tp_stop": 0.35, "stop_grace_s": 20,
  "tp1_mult": 2.0, "tp1_sell_frac": 0.5, "tp2_mult": 5.0, "tp2_sell_frac": 0.25,
  "bag_trail": 0.55, "trail_k": 3.0, "hold_min": 0.4,
  "dev_kill_min_frac": 0.5, "lp_drop_frac": 0.6, "dead_exit": 0.7, "max_hold_s": 36000 }
```

7.6 Sizing: fixed `bet_sol` until calibration passes (reliability error < 0.05 per bin). Only then optional
fractional Kelly on the bin distribution: `f* = argmax_f Σ_k P_k · ln(1 + f · R_k)`, use 0.25 · f*, cap 0.10.

## 8. Phase 7 — Costs and execution realism

- Venue fee from each pool's `fee_bps`; tx cost = base + priority fee + tip as parameters; `buy_fail_prob`;
  partial sells pay their own tx cost.
- Sensitivity runs: latency 1 / 2 / 4 slots, fail prob 0.15 / 0.30, tip ×2. Report all.

## 9. Phase 8 — Validation and acceptance

- Grid ≤ 24 combinations over `ev_min`, `bag_trail`, `tp1_mult`, `pre_tp_stop`. Select by the MEDIAN dev-window
  return (not the worst, not the mean).
- Run `final_holdout` windows once, after everything is frozen.
- Report `reports/V5_RESULTS.md`: per window return, trades, win rate, profit factor, expectancy,
  expectancy without the top 1% trades, max drawdown, costs, exits by reason, and moonshot capture
  (share of ≥ 10x runners where we held a position that reached ≥ 5x).

Acceptance (all must hold on test windows, at latency 2 slots):
1. Expectancy without the top 1% trades > 0.
2. Profit factor > 1.3.
3. ≥ 60% of test windows positive.
4. Moonshot capture ≥ 20% of ≥ 10x runners.
5. Still positive at latency 4 slots and fail prob 0.30.

If acceptance fails, report which part fails and stop; do not loosen the criteria without the user's written OK.

## 10. References (method sources)

- Pump.fun success as real-time probability driven by trader behavior and attention: arXiv 2602.14860
- Launch-phase behavior traces predict post-migration risk: arXiv 2602.13480 (MELT / MemeTrans)
- Early rug prediction from the first 5 min, gradient boosting, pump.fun + Raydium: arXiv 2608.20271
- Solana rug pull dataset: arXiv 2504.07132 (SolRPDS); on-chain rug detection: arXiv 2603.24625
- Graduation labels from off-chain collectors are noisy; compute labels on-chain: arXiv 2607.02823
- Base rate: graduation < 2%: arXiv 2512.11850
- Triple barrier + meta-labeling: López de Prado, *Advances in Financial Machine Learning* (2018)
- Trailing stop as optimal stopping: arXiv 1701.03960
