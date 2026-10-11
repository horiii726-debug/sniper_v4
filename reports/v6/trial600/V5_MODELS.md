# V5 models (Phase 4) — EXPLORATORY

Generated 2026-10-11 14:11:57. quick=False. Walk-forward with purge H=600 s, min_train=1.

## Windows

| window | start | data_end | rows | signal rows | hazard rows |
|---|---|---|---|---|---|
| h1d | 1791499503 | 1791501732 | 56469 | 40538 | 10013 |
| bf_1h | 1791594841 | 1791596804 | 39100 | 27700 | 7035 |

## Folds

- **fold_bf_1h**: train ['h1d']; excluded by purge []
- **final**: train ['h1d', 'bf_1h']; excluded by purge []

## Metrics (calibrated, test windows)

| scope | model | rows | pos | PR-AUC | ROC-AUC | Brier | Brier raw | log loss |
|---|---|---|---|---|---|---|---|---|
| bf_1h | entry_bin (P bin>=5) | 27700 | 81 | 0.0385 | 0.9045 | 0.0029 | - | mlogloss 0.4880 (raw 0.9003) |
| bf_1h | entry_tp1 | 27700 | 1363 | 0.1987 | 0.8766 | 0.0417 | 0.0966 | 0.1483 |
| bf_1h | rug_cb | 39100 | 4162 | 0.3482 | 0.8771 | 0.0767 | 0.0755 | 0.2831 |
| bf_1h | rug_rfcpu | 39100 | 4162 | 0.3490 | 0.8812 | 0.0742 | 0.0962 | 0.2467 |
| bf_1h | hold | 39100 | 3100 | 0.2357 | 0.8529 | 0.0634 | 0.0637 | 0.2062 |
| bf_1h | hazard | 7035 | 0 | - | - | 0.0000 | 0.0000 | 0.0000 |
| bf_1h | baseline_lr | 27700 | 1363 | 0.1898 | 0.8706 | 0.0423 | 0.0425 | 0.1525 |
| POOLED | entry_bin (P bin>=5) | 27700 | 81 | 0.0385 | 0.9045 | 0.0029 | - | mlogloss 0.4880 (raw 0.9003) |
| POOLED | entry_tp1 | 27700 | 1363 | 0.1987 | 0.8766 | 0.0417 | 0.0966 | 0.1483 |
| POOLED | rug_cb | 39100 | 4162 | 0.3482 | 0.8771 | 0.0767 | 0.0755 | 0.2831 |
| POOLED | rug_rfcpu | 39100 | 4162 | 0.3490 | 0.8812 | 0.0742 | 0.0962 | 0.2467 |
| POOLED | hold | 39100 | 3100 | 0.2357 | 0.8529 | 0.0634 | 0.0637 | 0.2062 |
| POOLED | hazard | 7035 | 0 | - | - | 0.0000 | 0.0000 | 0.0000 |
| POOLED | baseline_lr | 27700 | 1363 | 0.1898 | 0.8706 | 0.0423 | 0.0425 | 0.1525 |

## Entry: top 1% by P(bin>=5) = p_bin5+p_bin6, reached >= 10x (signal rows)

| scope | n top | precision | base rate | lift |
|---|---|---|---|---|
| bf_1h | 277 | 0.0542 | 0.0033 | 16.30 |
| POOLED | 277 | 0.0542 | 0.0033 | 16.30 |

## CatBoost vs logistic baseline (PR-AUC on y_tp1)

| scope | CatBoost | logistic |
|---|---|---|
| bf_1h | 0.1987 | 0.1898 |
| pooled | 0.1987 | 0.1898 |

## Rug: CatBoost vs GPU RF vs CPU RF vs average (p_rug = mean of calibrated CatBoost and GPU RF)

| scope | model | PR-AUC | ROC-AUC | Brier |
|---|---|---|---|---|
| bf_1h | rug_cb | 0.3482 | 0.8771 | 0.0767 |
| bf_1h | rug_rfgpu | - | - | - |
| bf_1h | rug_rfcpu | 0.3490 | 0.8812 | 0.0742 |
| bf_1h | rug | - | - | - |
| pooled | rug_cb | 0.3482 | 0.8771 | 0.0767 |
| pooled | rug_rfgpu | - | - | - |
| pooled | rug_rfcpu | 0.3490 | 0.8812 | 0.0742 |
| pooled | rug | - | - | - |

## Reliability (pooled, calibrated, 10 equal-width bins)

**entry_bin (P bin>=5)**: 0.0-0.1: 0.008/0.003/27700

**entry_tp1**: 0.0-0.1: 0.020/0.025/23702; 0.1-0.2: 0.139/0.142/1759; 0.2-0.3: 0.218/0.232/2239

**rug_cb**: 0.0-0.1: 0.034/0.016/28490; 0.1-0.2: 0.133/0.325/995; 0.2-0.3: 0.252/0.288/1126; 0.3-0.4: 0.367/0.361/479; 0.4-0.5: 0.419/0.361/7690; 0.5-0.6: 0.558/0.570/200; 0.6-0.7: 0.627/0.000/1; 0.7-0.8: 0.745/1.000/2; 0.9-1.0: 0.998/0.077/117

**rug_rfcpu**: 0.0-0.1: 0.009/0.005/25908; 0.1-0.2: 0.130/0.158/2881; 0.2-0.3: 0.237/0.332/367; 0.3-0.4: 0.394/0.334/8624; 0.4-0.5: 0.474/0.448/630; 0.5-0.6: 0.530/0.414/659; 0.8-0.9: 0.859/0.000/1; 0.9-1.0: 1.000/0.433/30

**hold**: 0.0-0.1: 0.013/0.010/25668; 0.1-0.2: 0.117/0.158/5432; 0.2-0.3: 0.287/0.249/7999; 0.5-0.6: 0.500/0.000/1

**hazard**: 0.0-0.1: 0.000/0.000/7035

**baseline_lr**: 0.0-0.1: 0.020/0.008/19933; 0.1-0.2: 0.144/0.143/6703; 0.2-0.3: 0.245/0.329/231; 0.3-0.4: 0.301/0.201/827; 0.4-0.5: 0.450/0.000/1; 0.5-0.6: 0.538/0.500/2; 0.6-0.7: 0.667/0.667/3

## Top-30 feature importance (CatBoost, mean over all fold models)

**entry_bin**: wallet_rate_med_60s 7.328, dev_hold_share 6.838, quote_res_sol 4.952, market_vol_10m 4.68, rd_split_max_sol_liq_300s 4.466, price_vs_low300 4.063, vol_60s 4.016, top10_supply_share 3.956, dev_net_sol 3.559, curve_progress 3.39, fee_bps 3.201, attention_share_5m 2.484, repeat_buyer_share_60s 2.138, rd_liq_real_sol 2.044, creator_buy_sol 1.955, rd_split_max_n_300s 1.875, f8_dt_cv20 1.865, cvd_300s 1.682, new_coins_per_min 1.497, price_vs_high300 1.465, mult_since_first 1.355, f1_sol_in_curve 1.345, f6_holders 1.275, buy_sol_15s 1.271, f8_cluster_share20 1.251, median_buy_60s 1.244, buy_sol_5s 1.045, profit_overhang 1.019, rd_dust_sol_60s 0.995, f1_sol_per_trade 0.915

**entry_tp1**: rd_split_max_sol_liq_300s 10.25, dev_net_sol 9.651, vol_60s 7.876, creator_buy_sol 6.51, wallet_rate_med_60s 6.298, dev_hold_share 5.228, market_vol_10m 4.423, quote_res_sol 4.142, top10_supply_share 3.638, log_quote_res 3.351, price_vs_high300 3.018, profit_overhang 2.564, median_buy_60s 2.439, price_vs_low300 1.883, f1_sol_per_trade 1.751, new_coins_per_min 1.737, curve_progress 1.494, attention_share_5m 1.187, new_wallet_share_60s 1.067, early_retention 1.064, f1_sol_in_curve 1.018, mult_since_first 0.986, f8_last_dt 0.866, accel 0.817, fee_bps 0.815, buy_sol_5s 0.791, buy_q_60s 0.756, fast_wallet_share_60s 0.739, kyle_lambda_60s 0.668, secs_since_real_buy 0.646

**rug_cb**: early20_delay_med_s 10.72, early_retention 7.015, f2_early20_supply_share 5.393, kyle_lambda_60s 4.968, quote_res_sol 4.733, f6_holders 3.214, rd_liq_real_sol 3.051, rd_split_max_n_300s 2.948, p90_sell_60s 2.77, top10_supply_share 2.709, f2_top10_holder_share 2.668, fee_bps 2.263, rd_dust_sol_60s 2.171, hhi_buy_60s 1.898, n_trades_60s 1.872, market_vol_10m 1.82, n_trades_300s 1.772, rd_dust_n_60s 1.689, rd_split_max_sol_liq_300s 1.625, new_wallet_share_60s 1.575, rd_state 1.383, attention_share_5m 1.295, median_sell_60s 1.275, f1_sol_in_curve 1.227, p90_buy_60s 1.211, profit_overhang 1.148, rd_kind_new 1.139, f8_hawkes_slow 1.096, creator_buy_sol 1.081, ewma_slow 1.072

**hold**: log_quote_res 6.363, rd_liq_real_sol 4.928, price_vs_high300 4.823, fee_bps 4.225, early_retention 4.037, early20_delay_med_s 3.57, kyle_lambda_60s 3.121, rd_split_max_n_300s 2.97, n_trades_300s 2.966, f6_holders 2.844, top10_supply_share 2.819, dev_hold_share 2.467, rd_split_max_sol_liq_300s 2.437, quote_res_sol 2.425, attention_share_5m 2.189, ewma_slow 2.163, price_vs_low300 2.105, market_vol_10m 2.025, mult_since_first 1.961, on_amm 1.638, rd_decisions_in_state 1.588, f1_trades_since_first 1.576, vol_60s 1.533, age_s 1.305, creator_buy_sol 1.238, repeat_buyer_share_60s 1.201, f2_early20_supply_share 1.175, sell_q_60s 1.156, rd_dust_sol_share_60s 1.087, curve_progress 1.038

**hazard**: creator_sold 5.512, on_amm 5.1, rd_cusum_pop 4.613, curve_progress 4.498, dev_ever_sold 4.408, rd_state 3.632, f8_hawkes_slow 3.468, log_quote_res 2.864, creator_sell_sol 2.83, rd_liq_real_sol 2.774, newborn 2.755, rd_secs_in_state 2.687, rd_kind_new 2.556, n_trades_300s 2.318, f1_sol_in_curve 1.99, accel 1.981, flow_imb_60s 1.653, dust_share_60s 1.601, f6_holders 1.579, rd_birth_src 1.559, dev_net_sol 1.52, creator_buy_sol 1.488, quote_res_sol 1.354, early_retention 1.347, rd_decisions_in_state 1.333, early20_delay_med_s 1.323, p90_sell_60s 1.318, f1_sol_per_trade 1.308, f8_cluster_share20 1.272, age_s 1.237

## Runtime and utilization

Wall 692.5 s (load 1.2 s, parallel training 689.1 s, sum of task wall times 1706.4 s). GPU slots 3 per card on [0].

| device | util mean % | p50 | max | share >=50% | share >=90% | mem max MiB |
|---|---|---|---|---|---|---|
| gpu0 | 87.8 | 99.0 | 100.0 | 0.885 | 0.875 | 5748.0 |
| cpu | 10.5 | - | 24.0 | - | - | - |

Per task:

| fold | model | pool | wall s | inner s | best iter | train rows | calibrated | inner split |
|---|---|---|---|---|---|---|---|---|
| final | baseline_lr | cpu | 24.8 | 5.1 | - | 68238 | True | inner train 1 windows (purged), calibration ['bf_1h'] |
| final | entry_bin | gpu0 | 98.6 | 95.7 | 62 | 68238 | True | inner train 1 windows (purged), calibration ['bf_1h'] |
| final | entry_tp1 | gpu0 | 37.7 | 34.3 | 44 | 68238 | True | inner train 1 windows (purged), calibration ['bf_1h'] |
| final | hazard | gpu0 | 529.5 | 254.5 | 2999 | 17048 | False | inner train 1 windows (purged), calibration ['bf_1h'] |
| final | hold | gpu0 | 27.3 | 24.9 | 67 | 95569 | True | inner train 1 windows (purged), calibration ['bf_1h'] |
| final | rug_cb | gpu0 | 63.6 | 63.0 | 32 | 95569 | True | inner train 1 windows (purged), calibration ['bf_1h'] |
| final | rug_rfcpu | cpu | 59.0 | 21.0 | - | 95569 | True | inner train 1 windows (purged), calibration ['bf_1h'] |
| fold_bf_1h | baseline_lr | cpu | 26.1 | 22.4 | - | 40538 | True | single window: time split at 75% quantile (no purge) |
| fold_bf_1h | entry_bin | gpu0 | 97.1 | 96.0 | 68 | 40538 | True | single window: time split at 75% quantile (no purge) |
| fold_bf_1h | entry_tp1 | gpu0 | 38.1 | 36.9 | 45 | 40538 | True | single window: time split at 75% quantile (no purge) |
| fold_bf_1h | hazard | gpu0 | 520.3 | 258.8 | 2999 | 10013 | False | single window: time split at 75% quantile (no purge) |
| fold_bf_1h | hold | gpu0 | 67.8 | 64.0 | 85 | 56469 | True | single window: time split at 75% quantile (no purge) |
| fold_bf_1h | rug_cb | gpu0 | 59.7 | 57.9 | 24 | 56469 | True | single window: time split at 75% quantile (no purge) |
| fold_bf_1h | rug_rfcpu | cpu | 56.8 | 20.7 | - | 56469 | True | single window: time split at 75% quantile (no purge) |
