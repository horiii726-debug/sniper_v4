# v7 — every model alone, combined system, and the QFV radar (EXPLORATORY, 1 test window)

- Branch `v6-fix` (base `origin/v6-impl` 9c2565c + signed-virtual-quote / mayhem decoder fixes).
- Data: v6 release bundles `processed.v2` (decoded BEFORE those two decoder fixes). Chronological split:
  train s06d_18h (2026-10-02) + s05d_11h (2026-10-03), test s04d_04h (2026-10-05). 33,318 test decision rows (radar signals),
  30 coins reached >= 10x within 1 h (peak before the -65 % barrier).
- Command: `LABEL_H=3600 python3 python/ml/v7_eval.py --train data/q_s06d_18h data/q_s05d_11h --test data/q_s04d_04h`
  → `QUICK_old3w.json`, `QUICK_old3w_seq.json`.
- One test window and 30 positive coins: direction only, not a performance claim.

## Target >= 10x (base rate 1.0 %)

| Family | AUC | PR-AUC | precision top 1 % |
|---|---|---|---|
| lightgbm | **0.906** | **0.055** | **5.1 %** |
| catboost | 0.888 | 0.047 | 4.2 % |
| combined (CatBoost + Markov + HMM + Kalman + PCA + QFV) | 0.886 | 0.045 | 4.8 % |
| combined without QFV | 0.886 | 0.045 | 3.9 % |
| random_forest | 0.879 | 0.042 | 3.6 % |
| logistic | 0.871 | 0.041 | 4.2 % |
| pca20_logistic (linear algebra) | 0.861 | 0.038 | 0.0 % |
| hmm_filter (forward-only posterior) | 0.859 | 0.035 | 0.3 % |
| markov_state | 0.690 | 0.017 | 6.0 % |
| qfv_radar (user formula) | 0.679 | 0.016 | 1.2 % |
| quantum_kernel (simulated ZZ map) | 0.602 | 0.013 | 0.9 % |
| kalman_trend | 0.552 | 0.022 | 3.9 % |

Rug/dead (base 5.2 %): random_forest AUC 0.944 / PR-AUC 0.482, lightgbm 0.929 / 0.488, catboost 0.918 / 0.442;
combined 0.917 / 0.422 (without QFV 0.922 / 0.435); qfv_radar 0.792 / 0.178.

## QFV radar (quantum_qfv_formula.pdf)

`P = sin²(π / (1 + exp(σ² / V_ratio)))`, σ² = variance of inter-trade intervals (s) over 5 min, V_ratio = SOL volume 5 min / 1 h.
P is monotone decreasing in x = σ²/V_ratio (P > 0.75 ⇔ x < ln 2), so it is a one-statistic rule, not a calibrated probability.

| Band | rows | rate >= 2x | rate >= 10x | >= 10x coins |
|---|---|---|---|---|
| P < 0.50 | 24,108 | 6.5 % | 0.49 % | 12 |
| 0.50–0.75 | 2,049 | 15.1 % | 3.3 % | 10 |
| P > 0.75 | 6,584 | 22.7 % | 2.0 % | 21 |

The band carries signal (10x rate ×4 between P > 0.75 and P < 0.5), but its top ranks (P ≈ 1: perfectly regular, bot-like
intervals) are poor (precision 1.2 % at top 1 %), and added to the ML system it gives no gain (PR-AUC 0.045 with and without).

## Decision
- QFV, quantum kernel, HMM, Kalman, Markov and PCA add no out-of-sample value on top of the tree models → **not in the
  trading system**; kept as diagnostics only. Re-check on the fresh 2026-10-16 windows before closing.
- Candidate entry model: LightGBM (to be confirmed walk-forward on >= 3 windows, then hyperparameters frozen on a validation window).
