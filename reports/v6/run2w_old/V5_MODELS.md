# V5 models (Phase 4) — EXPLORATORY

Generated 2026-10-11 18:12:57. quick=False. Walk-forward with purge H=3600 s, min_train=1.

## Windows

| window | start | data_end | rows | signal rows | hazard rows |
|---|---|---|---|---|---|
| s05d_11h | 1791048811 | 1791053719 | 77258 | 49317 | 12178 |
| s02d_14h | 1791353021 | 1791361859 | 204652 | 120560 | 40163 |

## Folds

- **fold_s02d_14h**: train ['s05d_11h']; excluded by purge []
- **final**: train ['s05d_11h', 's02d_14h']; excluded by purge []

## Metrics (calibrated, test windows)

| scope | model | rows | pos | PR-AUC | ROC-AUC | Brier | Brier raw | log loss |
|---|---|---|---|---|---|---|---|---|
| s02d_14h | entry_bin (P bin>=5) | 120560 | 3274 | 0.2045 | 0.8561 | 0.0266 | - | mlogloss 0.6601 (raw 0.9586) |
| s02d_14h | entry_tp1 | 120560 | 12567 | 0.2566 | 0.7905 | 0.0881 | 0.1249 | 0.2987 |
| s02d_14h | rug_cb | 204652 | 27784 | 0.4618 | 0.8733 | 0.1064 | 0.0956 | 0.3288 |
| s02d_14h | rug_rfcpu | 204652 | 27784 | 0.4484 | 0.8621 | 0.0985 | 0.1098 | 0.3209 |
| s02d_14h | hold | 204652 | 31959 | 0.3138 | 0.7456 | 0.1226 | 0.1261 | 0.4167 |
| s02d_14h | hazard | 40163 | 5 | 0.0011 | 0.8811 | 0.0001 | 0.0001 | 0.0014 |
| s02d_14h | baseline_lr | 120560 | 12567 | 0.2531 | 0.7707 | 0.0855 | 0.0880 | 0.3260 |
| s02d_14h | rug | 204652 | 27784 | 0.4710 | 0.8718 | 0.1000 | 0.0988 | 0.3127 |
| POOLED | entry_bin (P bin>=5) | 120560 | 3274 | 0.2045 | 0.8561 | 0.0266 | - | mlogloss 0.6601 (raw 0.9586) |
| POOLED | entry_tp1 | 120560 | 12567 | 0.2566 | 0.7905 | 0.0881 | 0.1249 | 0.2987 |
| POOLED | rug_cb | 204652 | 27784 | 0.4618 | 0.8733 | 0.1064 | 0.0956 | 0.3288 |
| POOLED | rug_rfcpu | 204652 | 27784 | 0.4484 | 0.8621 | 0.0985 | 0.1098 | 0.3209 |
| POOLED | hold | 204652 | 31959 | 0.3138 | 0.7456 | 0.1226 | 0.1261 | 0.4167 |
| POOLED | hazard | 40163 | 5 | 0.0011 | 0.8811 | 0.0001 | 0.0001 | 0.0014 |
| POOLED | baseline_lr | 120560 | 12567 | 0.2531 | 0.7707 | 0.0855 | 0.0880 | 0.3260 |
| POOLED | rug | 204652 | 27784 | 0.4710 | 0.8718 | 0.1000 | 0.0988 | 0.3127 |

## Entry: top 1% by P(bin>=5) = p_bin5+p_bin6, reached >= 10x (signal rows)

| scope | n top | precision | base rate | lift |
|---|---|---|---|---|
| s02d_14h | 1206 | 0.4619 | 0.0340 | 13.58 |
| POOLED | 1206 | 0.4619 | 0.0340 | 13.58 |

## CatBoost vs logistic baseline (PR-AUC on y_tp1)

| scope | CatBoost | logistic |
|---|---|---|
| s02d_14h | 0.2566 | 0.2531 |
| pooled | 0.2566 | 0.2531 |

## Rug: CatBoost vs GPU RF vs CPU RF vs average (p_rug = mean of calibrated CatBoost and GPU RF)

| scope | model | PR-AUC | ROC-AUC | Brier |
|---|---|---|---|---|
| s02d_14h | rug_cb | 0.4618 | 0.8733 | 0.1064 |
| s02d_14h | rug_rfgpu | - | - | - |
| s02d_14h | rug_rfcpu | 0.4484 | 0.8621 | 0.0985 |
| s02d_14h | rug | 0.4710 | 0.8718 | 0.1000 |
| pooled | rug_cb | 0.4618 | 0.8733 | 0.1064 |
| pooled | rug_rfgpu | - | - | - |
| pooled | rug_rfcpu | 0.4484 | 0.8621 | 0.0985 |
| pooled | rug | 0.4710 | 0.8718 | 0.1000 |

## Reliability (pooled, calibrated, 10 equal-width bins)

**entry_bin (P bin>=5)**: 0.0-0.1: 0.012/0.014/110931; 0.1-0.2: 0.179/0.131/1327; 0.2-0.3: 0.271/0.144/2964; 0.3-0.4: 0.388/0.192/1195; 0.4-0.5: 0.433/0.163/2786; 0.5-0.6: 0.549/0.288/987; 0.6-0.7: 0.608/0.578/348; 0.7-0.8: 0.727/0.278/18; 0.8-0.9: 0.822/0.000/3; 0.9-1.0: 0.999/0.000/1

**entry_tp1**: 0.0-0.1: 0.031/0.045/81733; 0.1-0.2: 0.165/0.199/30064; 0.2-0.3: 0.224/0.305/4580; 0.3-0.4: 0.341/0.294/511; 0.4-0.5: 0.436/0.339/1128; 0.5-0.6: 0.570/0.369/111; 0.6-0.7: 0.636/0.500/2; 0.7-0.8: 0.748/0.378/1023; 0.8-0.9: 0.828/0.406/1218; 0.9-1.0: 0.945/0.342/190

**rug_cb**: 0.0-0.1: 0.014/0.015/111027; 0.1-0.2: 0.130/0.045/16646; 0.2-0.3: 0.274/0.101/18360; 0.3-0.4: 0.355/0.206/6050; 0.4-0.5: 0.450/0.291/7650; 0.5-0.6: 0.556/0.346/7003; 0.6-0.7: 0.620/0.397/8564; 0.7-0.8: 0.763/0.467/24701; 0.8-0.9: 0.878/0.587/4421; 0.9-1.0: 0.942/0.570/230

**rug_rfcpu**: 0.0-0.1: 0.015/0.021/123730; 0.1-0.2: 0.155/0.042/10774; 0.2-0.3: 0.269/0.107/15129; 0.3-0.4: 0.380/0.282/8357; 0.4-0.5: 0.443/0.366/4019; 0.5-0.6: 0.536/0.457/11909; 0.6-0.7: 0.616/0.384/14767; 0.7-0.8: 0.715/0.478/8934; 0.8-0.9: 0.834/0.543/6888; 0.9-1.0: 0.926/0.779/145

**hold**: 0.0-0.1: 0.030/0.070/114901; 0.1-0.2: 0.164/0.245/66753; 0.2-0.3: 0.240/0.251/10831; 0.3-0.4: 0.319/0.277/4984; 0.4-0.5: 0.443/0.328/1242; 0.5-0.6: 0.572/0.319/1183; 0.6-0.7: 0.675/0.498/2420; 0.7-0.8: 0.746/0.375/8; 0.8-0.9: 0.847/0.659/1069; 0.9-1.0: 0.923/0.612/1261

**hazard**: 0.0-0.1: 0.000/0.000/40163

**baseline_lr**: 0.0-0.1: 0.034/0.034/67666; 0.1-0.2: 0.151/0.166/44886; 0.2-0.3: 0.266/0.246/2050; 0.3-0.4: 0.341/0.167/6; 0.4-0.5: 0.426/0.400/5713; 0.5-0.6: 0.506/0.000/6; 0.6-0.7: 0.697/1.000/1; 0.8-0.9: 0.862/0.280/25; 0.9-1.0: 0.996/0.145/207

**rug**: 0.0-0.1: 0.016/0.018/113630; 0.1-0.2: 0.143/0.033/21077; 0.2-0.3: 0.254/0.103/10690; 0.3-0.4: 0.349/0.208/8264; 0.4-0.5: 0.451/0.331/7219; 0.5-0.6: 0.554/0.385/10631; 0.6-0.7: 0.657/0.436/16279; 0.7-0.8: 0.749/0.488/11350; 0.8-0.9: 0.836/0.548/5331; 0.9-1.0: 0.909/0.707/181

## Top-30 feature importance (CatBoost, mean over all fold models)

**entry_bin**: fee_bps 8.552, rd_liq_real_sol 7.06, quote_res_sol 4.894, top10_supply_share 4.357, market_vol_10m 3.925, f1_trades_since_first 3.83, f6_holders 3.678, early20_delay_med_s 3.653, kyle_lambda_60s 2.744, rd_split_max_sol_liq_300s 2.673, attention_share_5m 2.624, cvd_300s 2.38, mult_since_first 2.255, price_vs_low300 2.158, wallet_rate_med_60s 2.082, curve_progress 2.022, creator_buy_sol 1.98, f1_sol_in_curve 1.931, profit_overhang 1.642, age_s 1.491, new_wallet_share_60s 1.45, early_retention 1.405, dev_hold_share 1.403, f1_sol_per_trade 1.343, new_coins_per_min 1.288, f8_last_dt 1.229, price_vs_high300 1.228, vol_60s 1.124, dev_net_sol 1.119, retail_share_60s 1.116

**entry_tp1**: top10_supply_share 15.121, fee_bps 11.965, log_quote_res 4.603, price_vs_low300 3.946, price_vs_high300 3.494, f8_hawkes_slow 3.023, early20_delay_med_s 2.773, f8_last_dt 2.669, f6_holders 2.483, retail_share_60s 2.398, dev_net_sol 2.346, dev_hold_share 2.277, vol_60s 2.093, age_s 2.064, quote_res_sol 1.835, kyle_lambda_60s 1.53, rd_liq_real_sol 1.449, f1_trades_since_first 1.421, buy_q_60s 1.383, mult_since_first 1.383, rd_split_max_sol_liq_300s 1.348, attention_share_5m 1.295, market_vol_10m 1.192, rd_state 1.141, wallet_rate_med_60s 1.136, f8_dt_p90_20 1.095, f1_sol_in_curve 1.078, cvd_300s 1.003, creator_buy_sol 0.864, profit_overhang 0.796

**rug_cb**: kyle_lambda_60s 6.997, early20_delay_med_s 6.289, fee_bps 5.971, early_retention 5.036, f6_holders 3.872, profit_overhang 3.761, rd_liq_real_sol 3.239, log_quote_res 3.193, mult_since_first 3.176, f2_top10_holder_share 3.084, top10_supply_share 3.009, p90_buy_60s 2.693, rd_dust_sol_share_60s 2.486, quote_res_sol 2.465, max_buy_60s 2.41, curve_progress 2.181, f2_early20_supply_share 2.168, market_vol_10m 1.861, wallet_rate_med_60s 1.847, f8_hawkes_slow 1.555, rd_split_max_n_300s 1.545, f8_dt_p90_20 1.465, creator_buy_sol 1.396, rd_cusum_alarms 1.224, f1_sol_in_curve 1.216, median_buy_60s 1.148, n_trades_300s 1.082, rd_nfl_60s 0.888, vol_60s 0.845, dev_hold_share 0.844

**hold**: fee_bps 8.401, top10_supply_share 7.491, early20_delay_med_s 6.766, rd_liq_real_sol 4.234, f2_top10_holder_share 3.688, f2_early20_supply_share 3.421, price_vs_high300 3.375, log_quote_res 3.228, quote_res_sol 3.028, f2_bundle_buy_share 2.977, early_retention 2.642, f6_holders 2.528, vol_60s 2.373, mult_since_first 2.363, market_vol_10m 2.17, rd_cusum_alarms 1.984, attention_share_5m 1.982, kyle_lambda_60s 1.948, dust_share_60s 1.933, f8_hawkes_slow 1.73, rd_split_max_n_300s 1.459, price_vs_low300 1.405, rd_dust_n_60s 1.338, f8_hawkes_fast 1.092, p90_buy_60s 1.057, max_buy_60s 0.99, rd_split_max_sol_liq_300s 0.99, age_s 0.916, f1_trades_since_first 0.864, creator_buy_sol 0.828

**hazard**: rd_cusum_pop 6.513, rd_state 5.892, newborn 5.212, on_amm 3.547, f8_cluster_share20 3.24, rd_kind_new 3.183, created_known 3.033, mult_since_first 2.917, creator_sold 2.193, f8_dt_p90_20 2.132, rd_birth_src 2.027, curve_progress 1.997, dev_ever_sold 1.995, rd_liq_real_sol 1.91, log_quote_res 1.881, curve_speed 1.745, f1_sol_per_trade 1.713, creator_buy_sol 1.704, buyer_growth 1.69, rd_cusum_alarm_age_s 1.685, f1_sol_in_curve 1.65, rd_split_max_sol_liq_300s 1.633, kyle_lambda_60s 1.495, f8_last_dt 1.436, n_trades_300s 1.402, quote_res_sol 1.395, f8_hawkes_slow 1.379, rd_secs_in_state 1.357, dust_share_60s 1.23, secs_since_hot 1.14

## Runtime and utilization

Wall 499.1 s (load 2.8 s, parallel training 489.3 s, sum of task wall times 1422.8 s). GPU slots 3 per card on [0].

| device | util mean % | p50 | max | share >=50% | share >=90% | mem max MiB |
|---|---|---|---|---|---|---|
| gpu0 | 57.0 | 87.0 | 100.0 | 0.601 | 0.177 | 6144.0 |
| cpu | 11.5 | - | 23.7 | - | - | - |

Per task:

| fold | model | pool | wall s | inner s | best iter | train rows | calibrated | inner split |
|---|---|---|---|---|---|---|---|---|
| final | baseline_lr | cpu | 100.0 | 54.3 | - | 169877 | True | inner train 1 windows (purged), calibration ['s02d_14h'] |
| final | entry_bin | gpu0 | 94.7 | 59.9 | 70 | 169877 | True | inner train 1 windows (purged), calibration ['s02d_14h'] |
| final | entry_tp1 | gpu0 | 133.7 | 132.6 | 22 | 169877 | True | inner train 1 windows (purged), calibration ['s02d_14h'] |
| final | hazard | gpu0 | 30.0 | 26.2 | 106 | 52341 | False | inner train 1 windows (purged), calibration ['s02d_14h'] |
| final | hold | gpu0 | 86.0 | 57.7 | 38 | 281910 | True | inner train 1 windows (purged), calibration ['s02d_14h'] |
| final | rug_cb | gpu0 | 140.4 | 119.9 | 54 | 281910 | True | inner train 1 windows (purged), calibration ['s02d_14h'] |
| final | rug_rfcpu | cpu | 218.6 | 51.4 | - | 281910 | True | inner train 1 windows (purged), calibration ['s02d_14h'] |
| fold_s02d_14h | baseline_lr | cpu | 8.5 | 4.6 | - | 49317 | True | single window: time split at 75% quantile (no purge) |
| fold_s02d_14h | entry_bin | gpu0 | 118.7 | 80.1 | 227 | 49317 | True | single window: time split at 75% quantile (no purge) |
| fold_s02d_14h | entry_tp1 | gpu0 | 29.0 | 27.0 | 82 | 49317 | True | single window: time split at 75% quantile (no purge) |
| fold_s02d_14h | hazard | gpu0 | 239.3 | 121.4 | 2999 | 12178 | False | single window: time split at 75% quantile (no purge) |
| fold_s02d_14h | hold | gpu0 | 29.2 | 17.9 | 241 | 77258 | True | single window: time split at 75% quantile (no purge) |
| fold_s02d_14h | rug_cb | gpu0 | 85.5 | 80.8 | 119 | 77258 | True | single window: time split at 75% quantile (no purge) |
| fold_s02d_14h | rug_rfcpu | cpu | 109.2 | 40.7 | - | 77258 | True | single window: time split at 75% quantile (no purge) |
