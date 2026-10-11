# Component gates — acceptance criteria (fixed 2026-10-11, BEFORE measuring on the V6 windows)

Rule: every component is tested alone, on its own job, with the metric below. Only PASS components are integrated.
A FAIL is investigated, fixed and re-tested; if it still fails it stays out and the reason is reported. Criteria are
not loosened after seeing results. "Cannot measure" = stays out of production. Live trading stays disabled
(CLAUDE.md: no real capital before Stage 9; no wallet private key is ever read).

Data: V6 windows from `scripts/v6_fetch_all.sh` (getBlock backfill + 1 h extension, every venue). Train/dev windows
only; the 3 `final_holdout` windows are evaluated once, at the end. n < 30 events = direction only (never a PASS).

| ID | Component | Test (its actual function) | PASS criterion |
|---|---|---|---|
| C1 | Data feed / backfill | `verify-backfill` per window | G0.H1 PASS on every window (each slot fetched or confirmed skipped), 0 fetch errors |
| C2 | Recording coverage | `leg-audit` on unfiltered recent blocks (>= 40) | >= 99.5 % of transactions with a token pool leg kept |
| C3a | Decoder: pump curve / PumpSwap | `pump-check` (pool math vs on-chain amounts) | existing pump-check criteria PASS |
| C3b | Decoder: generic pool legs | `generic_leg_check.py` vs validated-venue trades of the same coin within 10 s | median price ratio in [0.98, 1.02]; within 10 %: >= 85 % (other_venue), >= 95 % (stable_quoted) |
| C3c | On-chain SOL/USD | per-minute median of SOL/stable legs | >= 90 % of window minutes covered; minute-to-minute jumps < 5 % |
| C4 | Feature engine | causality test (prefix stream == full stream), unit tests | identical outputs; all tests pass |
| C5 | Radar | `radar-eval` on dev windows (authoritative births) | >= 10x runners HOT before +10 %: old coins >= 80 %, new coins >= 50 %; precision_2x of HOT coins > naive base rate on >= 60 % of windows; HOT <= 30 coins/min |
| C6 | Wallet analysis (scores, clusters) | drop-one feature-group ablation, leave-one-window-out | removing the group lowers PR-AUC (2x target) on >= 60 % of folds |
| C7a | Rug model | leave-one-window-out, y_rugdead | ROC-AUC >= 0.80 and PR-AUC >= 2 x base rate on >= 60 % of folds |
| C7b | Honeypot gate (mint flags) | unit test + entries into flagged mints in backtests | 0 entries into flagged mints |
| C7c | Honeypot sell simulation | needs `simulateTransaction` from a wallet address | cannot measure on history -> OUT until shadow mode |
| C8 | ML entry models | `model_families_v6.py` + `v5_train.py` walk-forward | CatBoost (or RF) beats logistic on PR-AUC (2x) on >= 60 % of folds; top-5 % lift >= 3 for >= 2x; calibration abs(mean p - mean y) < 0.05 in every bin with >= 100 rows |
| C9 | Ranking / EV | realized trade return by entry-EV quintile, test windows | top quintile mean return > bottom quintile on >= 60 % of windows |
| C10 | Entry gates (price guard, cooldown, caps, tick) | strategy ablation, same windows | removal lowers mean return on >= 60 % of windows (else removed) |
| C11 | Exit (U11-U14, 100 % sell) | exit variants on dev windows; unit tests | chosen variant has the best median dev-window return; one 100 % sell leg (unit test) |
| C12 | Sizing (fractional Kelly) | log-growth vs fixed bet on dev windows | only after C8 calibration PASS; 0.25 Kelly capped 0.10 raises median log-growth without raising worst DD > 5 pts |
| C13 | Account risk (E9: daily loss limit, loss-streak kill) | ablation | lowers worst-window max DD and does not lower median return by > 2 pts |
| C14 | Execution simulation | unit tests (slippage limit, retries, rent) + sensitivity | tests pass; latency 1/2/4 slots, fail 0.15/0.30, tip x2 reported |
| C15 | Live swap execution, latency | needs mainnet shadow / tx | cannot measure on history -> OUT (live disabled) |
| C16 | End-to-end (radar -> ML -> entry -> exit -> sim) | `v5-backtest` walk-forward, fixed strategy, 100 USD | existing acceptance: PF > 1.3, positive windows >= 60 %, expectancy excl. top 1 % > 0, positive at latency 4 + fail 0.30, moonshot capture >= 20 %; then the holdout windows once |
