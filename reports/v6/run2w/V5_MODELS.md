# V5 models (Phase 4) — EXPLORATORY

Generated 2026-10-11 19:26:41. quick=False. Walk-forward with purge H=3600 s, min_train=1.

## Windows

| window | start | data_end | rows | signal rows | hazard rows |
|---|---|---|---|---|---|
| s05d_11h | 1791048811 | 1791053719 | 77103 | 49309 | 12163 |
| s02d_14h | 1791353021 | 1791361859 | 204482 | 120541 | 40166 |

## Folds

- **fold_s02d_14h**: train ['s05d_11h']; excluded by purge []
- **final**: train ['s05d_11h', 's02d_14h']; excluded by purge []

## Metrics (calibrated, test windows)

| scope | model | rows | pos | PR-AUC | ROC-AUC | Brier | Brier raw | log loss |
|---|---|---|---|---|---|---|---|---|
| s02d_14h | entry_bin (P bin>=5) | 120541 | 1941 | 0.0437 | 0.7422 | 0.0171 | - | mlogloss 0.5919 (raw 0.9498) |
| s02d_14h | entry_tp1 | 120541 | 11168 | 0.2607 | 0.8142 | 0.0778 | 0.1088 | 0.2978 |
| s02d_14h | rug_cb | 204482 | 27654 | 0.4613 | 0.8757 | 0.1060 | 0.0948 | 0.3477 |
| s02d_14h | rug_rfcpu | 204482 | 27654 | 0.4452 | 0.8605 | 0.0974 | 0.1084 | 0.3211 |
| s02d_14h | hold | 204482 | 30005 | 0.3110 | 0.7653 | 0.1146 | 0.1164 | 0.4255 |
| s02d_14h | hazard | 40166 | 5 | 0.0035 | 0.9327 | 0.0001 | 0.0001 | 0.0013 |
| s02d_14h | baseline_lr | 120541 | 11168 | 0.2513 | 0.7870 | 0.0754 | 0.0787 | 0.2889 |
| s02d_14h | rug | 204482 | 27654 | 0.4691 | 0.8736 | 0.0990 | 0.0980 | 0.3112 |
| POOLED | entry_bin (P bin>=5) | 120541 | 1941 | 0.0437 | 0.7422 | 0.0171 | - | mlogloss 0.5919 (raw 0.9498) |
| POOLED | entry_tp1 | 120541 | 11168 | 0.2607 | 0.8142 | 0.0778 | 0.1088 | 0.2978 |
| POOLED | rug_cb | 204482 | 27654 | 0.4613 | 0.8757 | 0.1060 | 0.0948 | 0.3477 |
| POOLED | rug_rfcpu | 204482 | 27654 | 0.4452 | 0.8605 | 0.0974 | 0.1084 | 0.3211 |
| POOLED | hold | 204482 | 30005 | 0.3110 | 0.7653 | 0.1146 | 0.1164 | 0.4255 |
| POOLED | hazard | 40166 | 5 | 0.0035 | 0.9327 | 0.0001 | 0.0001 | 0.0013 |
| POOLED | baseline_lr | 120541 | 11168 | 0.2513 | 0.7870 | 0.0754 | 0.0787 | 0.2889 |
| POOLED | rug | 204482 | 27654 | 0.4691 | 0.8736 | 0.0990 | 0.0980 | 0.3112 |

## Entry: top 1% by P(bin>=5) = p_bin5+p_bin6, reached >= 10x (signal rows)

| scope | n top | precision | base rate | lift |
|---|---|---|---|---|
| s02d_14h | 1205 | 0.1054 | 0.0167 | 6.32 |
| POOLED | 1205 | 0.1054 | 0.0167 | 6.32 |

## CatBoost vs logistic baseline (PR-AUC on y_tp1)

| scope | CatBoost | logistic |
|---|---|---|
| s02d_14h | 0.2607 | 0.2513 |
| pooled | 0.2607 | 0.2513 |

## Rug: CatBoost vs GPU RF vs CPU RF vs average (p_rug = mean of calibrated CatBoost and GPU RF)

| scope | model | PR-AUC | ROC-AUC | Brier |
|---|---|---|---|---|
| s02d_14h | rug_cb | 0.4613 | 0.8757 | 0.1060 |
| s02d_14h | rug_rfgpu | - | - | - |
| s02d_14h | rug_rfcpu | 0.4452 | 0.8605 | 0.0974 |
| s02d_14h | rug | 0.4691 | 0.8736 | 0.0990 |
| pooled | rug_cb | 0.4613 | 0.8757 | 0.1060 |
| pooled | rug_rfgpu | - | - | - |
| pooled | rug_rfcpu | 0.4452 | 0.8605 | 0.0974 |
| pooled | rug | 0.4691 | 0.8736 | 0.0990 |

## Reliability (pooled, calibrated, 10 equal-width bins)

**entry_bin (P bin>=5)**: 0.0-0.1: 0.009/0.014/118569; 0.1-0.2: 0.168/0.156/397; 0.2-0.3: 0.281/0.108/111; 0.3-0.4: 0.379/0.149/994; 0.4-0.5: 0.418/0.051/256; 0.6-0.7: 0.639/0.000/214

**entry_tp1**: 0.0-0.1: 0.022/0.038/85866; 0.1-0.2: 0.158/0.201/29187; 0.2-0.3: 0.240/0.284/2306; 0.3-0.4: 0.359/0.388/467; 0.4-0.5: 0.495/0.448/368; 0.5-0.6: 0.543/0.471/558; 0.6-0.7: 0.618/0.362/235; 0.7-0.8: 0.721/0.421/114; 0.8-0.9: 0.809/0.575/106; 0.9-1.0: 0.956/0.446/1334

**rug_cb**: 0.0-0.1: 0.015/0.014/112093; 0.1-0.2: 0.146/0.060/10471; 0.2-0.3: 0.251/0.066/21884; 0.3-0.4: 0.363/0.184/8192; 0.4-0.5: 0.432/0.308/8746; 0.5-0.6: 0.552/0.394/10071; 0.6-0.7: 0.662/0.404/6582; 0.7-0.8: 0.752/0.477/10245; 0.8-0.9: 0.829/0.502/13015; 0.9-1.0: 0.916/0.557/3183

**rug_rfcpu**: 0.0-0.1: 0.016/0.021/128083; 0.1-0.2: 0.127/0.039/5660; 0.2-0.3: 0.256/0.100/13711; 0.3-0.4: 0.372/0.288/12953; 0.4-0.5: 0.439/0.417/2855; 0.5-0.6: 0.552/0.431/17296; 0.6-0.7: 0.627/0.392/7989; 0.7-0.8: 0.729/0.460/8745; 0.8-0.9: 0.818/0.543/7077; 0.9-1.0: 0.926/0.717/113

**hold**: 0.0-0.1: 0.021/0.036/104246; 0.1-0.2: 0.155/0.243/72203; 0.2-0.3: 0.231/0.265/19015; 0.3-0.4: 0.327/0.220/1662; 0.4-0.5: 0.460/0.241/1794; 0.5-0.6: 0.557/0.457/2879; 0.6-0.7: 0.607/0.000/1; 0.7-0.8: 0.749/0.600/195; 0.8-0.9: 0.846/0.631/1100; 0.9-1.0: 0.987/0.570/1387

**hazard**: 0.0-0.1: 0.000/0.000/40166

**baseline_lr**: 0.0-0.1: 0.032/0.030/78146; 0.1-0.2: 0.156/0.186/36760; 0.2-0.3: 0.258/0.220/1166; 0.3-0.4: 0.326/0.380/3623; 0.4-0.5: 0.470/0.443/846

**rug**: 0.0-0.1: 0.018/0.017/115904; 0.1-0.2: 0.142/0.037/18556; 0.2-0.3: 0.253/0.084/9891; 0.3-0.4: 0.351/0.213/11014; 0.4-0.5: 0.452/0.363/8236; 0.5-0.6: 0.554/0.397/9377; 0.6-0.7: 0.653/0.465/12104; 0.7-0.8: 0.746/0.451/12185; 0.8-0.9: 0.830/0.548/6915; 0.9-1.0: 0.917/0.563/300

## Top-30 feature importance (CatBoost, mean over all fold models)

**entry_bin**: fee_bps 7.393, quote_res_sol 6.75, top10_supply_share 5.304, f6_holders 4.505, early20_delay_med_s 3.933, market_vol_10m 3.844, price_vs_low300 3.488, rd_liq_real_sol 3.103, f1_trades_since_first 2.778, creator_buy_sol 2.727, rd_split_max_sol_liq_300s 2.425, profit_overhang 2.301, kyle_lambda_60s 2.218, mult_since_first 2.177, dev_hold_share 1.979, early_retention 1.72, f1_sol_per_trade 1.702, vol_60s 1.667, dev_net_sol 1.634, age_s 1.628, attention_share_5m 1.48, retail_share_60s 1.48, cvd_300s 1.457, f1_sol_in_curve 1.379, wallet_rate_med_60s 1.332, f8_last_dt 1.264, new_wallet_share_60s 1.264, buy_sol_5s 1.229, median_buy_60s 1.143, curve_progress 1.125

**entry_tp1**: top10_supply_share 12.313, fee_bps 5.938, price_vs_low300 4.319, quote_res_sol 4.033, kyle_lambda_60s 3.881, log_quote_res 3.862, early20_delay_med_s 3.845, age_s 3.598, rd_split_max_sol_liq_300s 3.106, dev_net_sol 3.025, vol_60s 2.466, f8_hawkes_fast 2.463, cvd_300s 2.361, mult_since_first 2.197, dev_hold_share 1.984, f1_sol_per_trade 1.974, rd_liq_real_sol 1.958, buy_q_60s 1.705, market_vol_10m 1.687, price_vs_high300 1.594, f8_last_dt 1.478, wallet_rate_med_60s 1.222, f8_dt_p90_20 1.201, f8_hawkes_slow 1.2, rd_cusum_alarms 1.154, curve_progress 1.137, attention_share_5m 1.127, f1_sol_in_curve 1.067, f6_holders 1.021, buy_sol_60s 1.011

**rug_cb**: kyle_lambda_60s 7.513, fee_bps 7.158, early20_delay_med_s 5.459, early_retention 4.933, p90_buy_60s 4.521, f6_holders 4.154, rd_liq_real_sol 3.583, mult_since_first 2.981, profit_overhang 2.971, f2_top10_holder_share 2.715, top10_supply_share 2.529, rd_dust_sol_share_60s 2.345, market_vol_10m 2.119, quote_res_sol 1.861, curve_progress 1.722, wallet_rate_med_60s 1.72, log_quote_res 1.7, max_buy_60s 1.629, f8_hawkes_slow 1.619, vol_60s 1.586, f2_early20_supply_share 1.531, rd_split_max_n_300s 1.527, n_trades_60s 1.48, n_trades_300s 1.445, creator_buy_sol 1.249, attention_share_5m 1.153, f8_dt_p90_20 1.074, f2_bundle_buy_share 1.051, median_buy_60s 1.051, rd_dust_sol_60s 1.036

**hold**: early20_delay_med_s 8.261, early_retention 5.121, f2_early20_supply_share 4.711, rd_liq_real_sol 4.301, quote_res_sol 4.268, f2_top10_holder_share 4.105, log_quote_res 4.056, top10_supply_share 3.818, f2_bundle_buy_share 3.769, kyle_lambda_60s 3.659, mult_since_first 3.076, dust_share_60s 2.768, fee_bps 2.727, rd_cusum_alarms 2.227, vol_60s 2.091, attention_share_5m 1.878, rd_dust_n_60s 1.797, market_vol_10m 1.787, median_buy_60s 1.647, f6_holders 1.591, f1_trades_since_first 1.398, rd_split_max_sol_liq_300s 1.388, price_vs_low300 1.352, f8_hawkes_slow 1.311, p90_buy_60s 1.266, f8_dt_p90_20 1.083, wallet_rate_med_60s 1.067, retail_share_60s 1.037, creator_buy_sol 1.014, rd_decisions_in_state 0.95

**hazard**: rd_cusum_pop 9.104, newborn 6.51, created_known 5.229, rd_state 4.173, rd_kind_new 3.94, creator_sell_sol 3.207, dev_ever_sold 2.828, creator_sold 2.753, creator_buy_sol 2.642, log_quote_res 2.138, f1_sol_in_curve 2.076, rd_liq_real_sol 2.047, kyle_lambda_60s 2.039, rd_secs_in_state 2.028, f8_dt_p90_20 1.968, rd_split_max_sol_liq_300s 1.786, f8_cluster_share20 1.727, on_amm 1.717, f1_sol_per_trade 1.631, mult_since_first 1.581, hhi_buy_60s 1.539, buyer_growth 1.48, attention_share_5m 1.369, rd_decisions_in_state 1.362, hawkes_branching 1.288, vol_60s 1.26, rd_cusum_uc 1.25, f8_hawkes_slow 1.22, rd_cusum_alarm_age_s 1.185, rd_birth_src 1.158

## Runtime and utilization

Wall 498.6 s (load 2.8 s, parallel training 488.7 s, sum of task wall times 1436.4 s). GPU slots 3 per card on [0].

| device | util mean % | p50 | max | share >=50% | share >=90% | mem max MiB |
|---|---|---|---|---|---|---|
| gpu0 | 56.5 | 86.0 | 100.0 | 0.593 | 0.171 | 6144.0 |
| cpu | 10.7 | - | 23.7 | - | - | - |

Per task:

| fold | model | pool | wall s | inner s | best iter | train rows | calibrated | inner split |
|---|---|---|---|---|---|---|---|---|
| final | baseline_lr | cpu | 109.7 | 92.7 | - | 169850 | True | inner train 1 windows (purged), calibration ['s02d_14h'] |
| final | entry_bin | gpu0 | 89.7 | 62.4 | 75 | 169850 | True | inner train 1 windows (purged), calibration ['s02d_14h'] |
| final | entry_tp1 | gpu0 | 125.5 | 120.9 | 19 | 169850 | True | inner train 1 windows (purged), calibration ['s02d_14h'] |
| final | hazard | gpu0 | 41.5 | 35.8 | 144 | 52329 | False | inner train 1 windows (purged), calibration ['s02d_14h'] |
| final | hold | gpu0 | 108.7 | 92.8 | 38 | 281585 | True | inner train 1 windows (purged), calibration ['s02d_14h'] |
| final | rug_cb | gpu0 | 65.1 | 48.5 | 47 | 281585 | True | inner train 1 windows (purged), calibration ['s02d_14h'] |
| final | rug_rfcpu | cpu | 216.6 | 48.9 | - | 281585 | True | inner train 1 windows (purged), calibration ['s02d_14h'] |
| fold_s02d_14h | baseline_lr | cpu | 10.7 | 4.7 | - | 49309 | True | single window: time split at 75% quantile (no purge) |
| fold_s02d_14h | entry_bin | gpu0 | 128.0 | 122.7 | 109 | 49309 | True | single window: time split at 75% quantile (no purge) |
| fold_s02d_14h | entry_tp1 | gpu0 | 44.8 | 40.7 | 180 | 49309 | True | single window: time split at 75% quantile (no purge) |
| fold_s02d_14h | hazard | gpu0 | 238.5 | 123.9 | 2999 | 12163 | False | single window: time split at 75% quantile (no purge) |
| fold_s02d_14h | hold | gpu0 | 46.5 | 40.4 | 135 | 77103 | True | single window: time split at 75% quantile (no purge) |
| fold_s02d_14h | rug_cb | gpu0 | 107.9 | 100.1 | 132 | 77103 | True | single window: time split at 75% quantile (no purge) |
| fold_s02d_14h | rug_rfcpu | cpu | 103.2 | 38.7 | - | 77103 | True | single window: time split at 75% quantile (no purge) |
