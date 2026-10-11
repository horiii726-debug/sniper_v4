# V5 models (Phase 4) — EXPLORATORY

Generated 2026-10-11 21:58:09. quick=False. Walk-forward with purge H=3600 s, min_train=1.

## Windows

| window | start | data_end | rows | signal rows | hazard rows |
|---|---|---|---|---|---|
| s06d_18h | 1790911556 | 1790916445 | 57443 | 37403 | 8920 |
| s05d_11h | 1791048811 | 1791053719 | 74280 | 48617 | 12019 |
| s04d_04h | 1791186280 | 1791191150 | 49262 | 33318 | 8479 |
| s02d_14h | 1791353021 | 1791361859 | 201824 | 120507 | 39990 |

## Folds

- **fold_s05d_11h**: train ['s06d_18h']; excluded by purge []
- **fold_s04d_04h**: train ['s06d_18h', 's05d_11h']; excluded by purge []
- **fold_s02d_14h**: train ['s06d_18h', 's05d_11h', 's04d_04h']; excluded by purge []
- **final**: train ['s06d_18h', 's05d_11h', 's04d_04h', 's02d_14h']; excluded by purge []

## Metrics (calibrated, test windows)

| scope | model | rows | pos | PR-AUC | ROC-AUC | Brier | Brier raw | log loss |
|---|---|---|---|---|---|---|---|---|
| s05d_11h | entry_bin (P bin>=5) | 48617 | 416 | 0.0233 | 0.7808 | 0.0108 | - | mlogloss 0.6945 (raw 1.1256) |
| s05d_11h | entry_tp1 | 48617 | 5243 | 0.3043 | 0.8234 | 0.0846 | 0.1226 | 0.2795 |
| s05d_11h | rug_cb | 74280 | 10058 | 0.5066 | 0.8827 | 0.0875 | 0.0850 | 0.3409 |
| s05d_11h | rug_rfcpu | 74280 | 10058 | 0.4380 | 0.8711 | 0.0896 | 0.0955 | 0.3323 |
| s05d_11h | hold | 74280 | 11519 | 0.3381 | 0.7801 | 0.1137 | 0.1141 | 0.3568 |
| s05d_11h | hazard | 12019 | 0 | - | - | 0.0000 | 0.0000 | 0.0015 |
| s05d_11h | baseline_lr | 48617 | 5243 | 0.2411 | 0.7821 | 0.0875 | 0.0916 | 0.2923 |
| s05d_11h | rug | 74280 | 10058 | 0.4885 | 0.8825 | 0.0857 | 0.0836 | 0.3165 |
| s04d_04h | entry_bin (P bin>=5) | 33318 | 327 | 0.0485 | 0.8898 | 0.0095 | - | mlogloss 0.5197 (raw 0.8960) |
| s04d_04h | entry_tp1 | 33318 | 3028 | 0.3171 | 0.8842 | 0.0674 | 0.0965 | 0.2091 |
| s04d_04h | rug_cb | 49262 | 4158 | 0.5462 | 0.9303 | 0.0506 | 0.0502 | 0.1681 |
| s04d_04h | rug_rfcpu | 49262 | 4158 | 0.4373 | 0.9265 | 0.0525 | 0.0655 | 0.1639 |
| s04d_04h | hold | 49262 | 6514 | 0.3288 | 0.8234 | 0.0978 | 0.0983 | 0.3063 |
| s04d_04h | hazard | 8479 | 0 | - | - | 0.0000 | 0.0000 | 0.0000 |
| s04d_04h | baseline_lr | 33318 | 3028 | 0.2955 | 0.8618 | 0.0701 | 0.0707 | 0.2246 |
| s04d_04h | rug | 49262 | 4158 | 0.5220 | 0.9317 | 0.0507 | 0.0539 | 0.1642 |
| s02d_14h | entry_bin (P bin>=5) | 120507 | 722 | 0.0307 | 0.8704 | 0.0061 | - | mlogloss 0.5288 (raw 0.9149) |
| s02d_14h | entry_tp1 | 120507 | 9899 | 0.2618 | 0.8592 | 0.0666 | 0.1227 | 0.2142 |
| s02d_14h | rug_cb | 201824 | 20734 | 0.4579 | 0.8711 | 0.0712 | 0.0707 | 0.3222 |
| s02d_14h | rug_rfcpu | 201824 | 20734 | 0.4104 | 0.8617 | 0.0681 | 0.0877 | 0.2617 |
| s02d_14h | hold | 201824 | 27165 | 0.3432 | 0.8171 | 0.0997 | 0.0981 | 0.3161 |
| s02d_14h | hazard | 39990 | 5 | 0.2090 | 0.9882 | 0.0001 | 0.0001 | 0.0009 |
| s02d_14h | baseline_lr | 120507 | 9899 | 0.2724 | 0.8548 | 0.0646 | 0.0670 | 0.2273 |
| s02d_14h | rug | 201824 | 20734 | 0.4707 | 0.8712 | 0.0670 | 0.0741 | 0.2349 |
| POOLED | entry_bin (P bin>=5) | 202442 | 1465 | 0.0240 | 0.8345 | 0.0078 | - | mlogloss 0.5671 (raw 0.9624) |
| POOLED | entry_tp1 | 202442 | 18170 | 0.2742 | 0.8477 | 0.0710 | 0.1184 | 0.2290 |
| POOLED | rug_cb | 325366 | 34950 | 0.4814 | 0.8761 | 0.0718 | 0.0709 | 0.3031 |
| POOLED | rug_rfcpu | 325366 | 34950 | 0.4238 | 0.8735 | 0.0707 | 0.0861 | 0.2630 |
| POOLED | hold | 325366 | 45198 | 0.3379 | 0.8071 | 0.1026 | 0.1018 | 0.3239 |
| POOLED | hazard | 60488 | 5 | 0.0205 | 0.9084 | 0.0001 | 0.0001 | 0.0009 |
| POOLED | baseline_lr | 202442 | 18170 | 0.2648 | 0.8361 | 0.0710 | 0.0735 | 0.2425 |
| POOLED | rug | 325366 | 34950 | 0.4789 | 0.8802 | 0.0688 | 0.0732 | 0.2428 |

## Entry: top 1% by P(bin>=5) = p_bin5+p_bin6, reached >= 10x (signal rows)

| scope | n top | precision | base rate | lift |
|---|---|---|---|---|
| s05d_11h | 486 | 0.0350 | 0.0101 | 3.46 |
| s04d_04h | 333 | 0.0571 | 0.0105 | 5.45 |
| s02d_14h | 1205 | 0.0639 | 0.0065 | 9.76 |
| POOLED | 2024 | 0.0459 | 0.0081 | 5.71 |

## CatBoost vs logistic baseline (PR-AUC on y_tp1)

| scope | CatBoost | logistic |
|---|---|---|
| s05d_11h | 0.3043 | 0.2411 |
| s04d_04h | 0.3171 | 0.2955 |
| s02d_14h | 0.2618 | 0.2724 |
| pooled | 0.2742 | 0.2648 |

## Rug: CatBoost vs GPU RF vs CPU RF vs average (p_rug = mean of calibrated CatBoost and GPU RF)

| scope | model | PR-AUC | ROC-AUC | Brier |
|---|---|---|---|---|
| s05d_11h | rug_cb | 0.5066 | 0.8827 | 0.0875 |
| s05d_11h | rug_rfgpu | - | - | - |
| s05d_11h | rug_rfcpu | 0.4380 | 0.8711 | 0.0896 |
| s05d_11h | rug | 0.4885 | 0.8825 | 0.0857 |
| s04d_04h | rug_cb | 0.5462 | 0.9303 | 0.0506 |
| s04d_04h | rug_rfgpu | - | - | - |
| s04d_04h | rug_rfcpu | 0.4373 | 0.9265 | 0.0525 |
| s04d_04h | rug | 0.5220 | 0.9317 | 0.0507 |
| s02d_14h | rug_cb | 0.4579 | 0.8711 | 0.0712 |
| s02d_14h | rug_rfgpu | - | - | - |
| s02d_14h | rug_rfcpu | 0.4104 | 0.8617 | 0.0681 |
| s02d_14h | rug | 0.4707 | 0.8712 | 0.0670 |
| pooled | rug_cb | 0.4814 | 0.8761 | 0.0718 |
| pooled | rug_rfgpu | - | - | - |
| pooled | rug_rfcpu | 0.4238 | 0.8735 | 0.0707 |
| pooled | rug | 0.4789 | 0.8802 | 0.0688 |

## Reliability (pooled, calibrated, 10 equal-width bins)

**entry_bin (P bin>=5)**: 0.0-0.1: 0.013/0.007/198803; 0.1-0.2: 0.121/0.026/2915; 0.2-0.3: 0.237/0.061/510; 0.3-0.4: 0.324/0.017/60; 0.4-0.5: 0.453/0.014/74; 0.5-0.6: 0.554/0.000/49; 0.6-0.7: 0.623/0.000/31

**entry_tp1**: 0.0-0.1: 0.013/0.017/135632; 0.1-0.2: 0.141/0.174/15989; 0.2-0.3: 0.268/0.238/30243; 0.3-0.4: 0.351/0.262/17538; 0.4-0.5: 0.457/0.365/1628; 0.5-0.6: 0.520/0.352/270; 0.6-0.7: 0.612/0.493/743; 0.7-0.8: 0.721/0.533/349; 0.8-0.9: 0.800/0.667/15; 0.9-1.0: 1.000/0.686/35

**rug_cb**: 0.0-0.1: 0.015/0.024/246852; 0.1-0.2: 0.156/0.162/8399; 0.2-0.3: 0.242/0.177/3306; 0.3-0.4: 0.338/0.271/28115; 0.4-0.5: 0.452/0.367/10323; 0.5-0.6: 0.544/0.480/12527; 0.6-0.7: 0.672/0.565/4639; 0.7-0.8: 0.731/0.611/4790; 0.8-0.9: 0.814/0.697/859; 0.9-1.0: 0.985/0.617/5556

**rug_rfcpu**: 0.0-0.1: 0.011/0.021/245692; 0.1-0.2: 0.146/0.146/14441; 0.2-0.3: 0.263/0.334/6591; 0.3-0.4: 0.360/0.427/2725; 0.4-0.5: 0.420/0.406/32444; 0.5-0.6: 0.541/0.463/17900; 0.6-0.7: 0.676/0.518/3866; 0.7-0.8: 0.748/0.517/180; 0.8-0.9: 0.811/0.520/1172; 0.9-1.0: 0.993/0.577/355

**hold**: 0.0-0.1: 0.016/0.019/164708; 0.1-0.2: 0.147/0.114/13499; 0.2-0.3: 0.244/0.239/90025; 0.3-0.4: 0.332/0.282/29911; 0.4-0.5: 0.435/0.375/24846; 0.5-0.6: 0.580/0.531/2045; 0.6-0.7: 0.663/0.247/332

**hazard**: 0.0-0.1: 0.000/0.000/60480; 0.1-0.2: 0.156/0.000/8

**baseline_lr**: 0.0-0.1: 0.011/0.012/121060; 0.1-0.2: 0.152/0.141/33812; 0.2-0.3: 0.231/0.235/40429; 0.3-0.4: 0.377/0.213/994; 0.4-0.5: 0.418/0.360/6147

**rug**: 0.0-0.1: 0.014/0.021/243564; 0.1-0.2: 0.145/0.138/12486; 0.2-0.3: 0.246/0.180/8506; 0.3-0.4: 0.364/0.310/18979; 0.4-0.5: 0.446/0.408/15809; 0.5-0.6: 0.545/0.508/14799; 0.6-0.7: 0.650/0.606/5700; 0.7-0.8: 0.741/0.675/2773; 0.8-0.9: 0.838/0.516/2313; 0.9-1.0: 0.955/0.485/437

## Top-30 feature importance (CatBoost, mean over all fold models)

**entry_bin**: market_vol_10m 5.883, rd_split_max_sol_liq_300s 4.402, creator_buy_sol 4.267, early20_delay_med_s 4.021, top10_supply_share 3.759, price_vs_low300 3.475, f6_holders 3.029, quote_res_sol 2.876, f1_trades_since_first 2.777, wallet_rate_med_60s 2.446, curve_progress 2.377, vol_60s 2.373, new_coins_per_min 2.312, f1_sol_in_curve 2.296, mult_since_first 2.293, dev_hold_share 1.971, f1_sol_per_trade 1.961, fee_bps 1.903, attention_share_5m 1.881, buy_sol_5s 1.78, dev_net_sol 1.766, median_buy_60s 1.72, rd_liq_real_sol 1.685, kyle_lambda_60s 1.642, cvd_300s 1.638, new_wallet_share_60s 1.506, profit_overhang 1.477, f8_dt_cv20 1.343, p90_buy_60s 1.188, rd_dust_sol_60s 1.094

**entry_tp1**: top10_supply_share 6.839, price_vs_low300 6.646, log_quote_res 4.685, vol_60s 3.79, creator_buy_sol 3.477, rd_split_max_sol_liq_300s 3.404, early20_delay_med_s 3.32, fee_bps 2.892, curve_progress 2.748, price_vs_high300 2.618, f8_last_dt 2.609, quote_res_sol 2.489, f1_sol_in_curve 2.397, f1_sol_per_trade 2.381, age_s 2.27, market_vol_10m 2.223, mult_since_first 2.06, wallet_rate_med_60s 2.042, kyle_lambda_60s 2.04, dev_net_sol 1.975, f1_trades_since_first 1.756, attention_share_5m 1.727, rd_liq_real_sol 1.578, new_coins_per_min 1.49, dev_hold_share 1.389, rd_cusum_alarms 1.22, dust_share_60s 1.174, profit_overhang 1.119, rd_nfl_60s 0.953, ret_300s 0.939

**rug_cb**: early20_delay_med_s 6.619, fee_bps 6.24, kyle_lambda_60s 5.28, f6_holders 5.218, rd_liq_real_sol 5.077, early_retention 4.365, log_quote_res 4.161, mult_since_first 3.703, wallet_rate_med_60s 2.963, f2_early20_supply_share 2.88, creator_buy_sol 2.735, quote_res_sol 2.719, rd_dust_n_60s 2.212, dev_hold_share 1.857, market_vol_10m 1.764, f2_top10_holder_share 1.764, profit_overhang 1.651, top10_supply_share 1.525, attention_share_5m 1.51, f2_bundle_buy_share 1.499, price_vs_low300 1.412, rd_split_max_sol_liq_300s 1.335, vol_60s 1.227, f1_sol_in_curve 1.198, median_buy_60s 1.166, rd_cusum_alarms 1.148, sellers_60s 1.057, repeat_buyer_share_60s 0.973, p90_buy_60s 0.931, f1_trades_since_first 0.931

**hold**: rd_liq_real_sol 11.872, log_quote_res 11.102, quote_res_sol 7.982, early20_delay_med_s 4.349, f2_early20_supply_share 3.559, rd_cusum_alarms 3.346, f2_top10_holder_share 3.263, kyle_lambda_60s 2.966, early_retention 2.829, market_vol_10m 2.427, rd_dust_n_60s 2.241, vol_60s 2.197, fee_bps 2.099, f2_bundle_buy_share 2.063, mult_since_first 2.02, attention_share_5m 1.983, price_vs_low300 1.803, top10_supply_share 1.782, f6_holders 1.773, dust_share_60s 1.747, median_buy_60s 1.512, rd_split_max_n_300s 1.428, profit_overhang 1.338, f8_hawkes_slow 1.176, rd_dust_top_cluster_share_60s 1.011, f1_trades_since_first 1.008, wallet_rate_med_60s 0.987, holder_vel_60s 0.887, rd_split_max_sol_liq_300s 0.865, n_trades_15s 0.726

**hazard**: rd_kind_new 7.324, rd_state 5.807, newborn 5.401, rd_cusum_pop 4.713, created_known 4.53, creator_sell_sol 3.298, f2_bundle_txs_60s 2.683, rd_cusum_alarm_age_s 2.592, creator_sold 2.507, on_amm 2.289, rd_birth_src 1.904, rd_decisions_in_state 1.83, dev_ever_sold 1.805, rd_secs_in_state 1.749, age_s 1.647, creator_buy_sol 1.496, cvd_300s 1.419, hhi_buy_60s 1.401, mult_since_first 1.368, f8_cluster_share20 1.335, vol_60s 1.312, f8_dt_p90_20 1.29, log_quote_res 1.285, accel 1.245, attention_share_5m 1.222, buy_sol_60s 1.163, quote_res_sol 1.053, rd_liq_real_sol 0.973, dev_net_sol 0.941, f8_dt_median20 0.916

## Runtime and utilization

Wall 1161.3 s (load 2.8 s, parallel training 1148.2 s, sum of task wall times 4021.8 s). GPU slots 3 per card on [0].

| device | util mean % | p50 | max | share >=50% | share >=90% | mem max MiB |
|---|---|---|---|---|---|---|
| gpu0 | 64.5 | 91.0 | 100.0 | 0.634 | 0.549 | 6366.0 |
| cpu | 10.8 | - | 22.2 | - | - | - |

Per task:

| fold | model | pool | wall s | inner s | best iter | train rows | calibrated | inner split |
|---|---|---|---|---|---|---|---|---|
| final | baseline_lr | cpu | 61.7 | 20.9 | - | 239845 | True | inner train 3 windows (purged), calibration ['s02d_14h'] |
| final | entry_bin | gpu0 | 139.1 | 120.3 | 107 | 239845 | True | inner train 3 windows (purged), calibration ['s02d_14h'] |
| final | entry_tp1 | gpu0 | 125.5 | 116.4 | 59 | 239845 | True | inner train 3 windows (purged), calibration ['s02d_14h'] |
| final | hazard | gpu0 | 570.4 | 496.8 | 2826 | 69408 | False | inner train 3 windows (purged), calibration ['s02d_14h'] |
| final | hold | gpu0 | 127.6 | 112.1 | 40 | 382809 | True | inner train 3 windows (purged), calibration ['s02d_14h'] |
| final | rug_cb | gpu0 | 123.6 | 109.1 | 53 | 382809 | True | inner train 3 windows (purged), calibration ['s02d_14h'] |
| final | rug_rfcpu | cpu | 408.7 | 130.2 | - | 382809 | True | inner train 3 windows (purged), calibration ['s02d_14h'] |
| fold_s02d_14h | baseline_lr | cpu | 46.7 | 29.9 | - | 119338 | True | inner train 2 windows (purged), calibration ['s04d_04h'] |
| fold_s02d_14h | entry_bin | gpu0 | 195.1 | 139.0 | 138 | 119338 | True | inner train 2 windows (purged), calibration ['s04d_04h'] |
| fold_s02d_14h | entry_tp1 | gpu0 | 41.8 | 33.7 | 81 | 119338 | True | inner train 2 windows (purged), calibration ['s04d_04h'] |
| fold_s02d_14h | hazard | gpu0 | 524.7 | 363.4 | 2994 | 29418 | False | inner train 2 windows (purged), calibration ['s04d_04h'] |
| fold_s02d_14h | hold | gpu0 | 96.7 | 94.5 | 30 | 180985 | True | inner train 2 windows (purged), calibration ['s04d_04h'] |
| fold_s02d_14h | rug_cb | gpu0 | 128.0 | 105.6 | 111 | 180985 | True | inner train 2 windows (purged), calibration ['s04d_04h'] |
| fold_s02d_14h | rug_rfcpu | cpu | 256.3 | 103.5 | - | 180985 | True | inner train 2 windows (purged), calibration ['s04d_04h'] |
| fold_s04d_04h | baseline_lr | cpu | 28.9 | 15.0 | - | 86020 | True | inner train 1 windows (purged), calibration ['s05d_11h'] |
| fold_s04d_04h | entry_bin | gpu0 | 170.9 | 137.2 | 89 | 86020 | True | inner train 1 windows (purged), calibration ['s05d_11h'] |
| fold_s04d_04h | entry_tp1 | gpu0 | 109.7 | 99.5 | 148 | 86020 | True | inner train 1 windows (purged), calibration ['s05d_11h'] |
| fold_s04d_04h | hazard | gpu0 | 180.8 | 103.2 | 554 | 20939 | False | inner train 1 windows (purged), calibration ['s05d_11h'] |
| fold_s04d_04h | hold | gpu0 | 38.2 | 29.6 | 67 | 131723 | True | inner train 1 windows (purged), calibration ['s05d_11h'] |
| fold_s04d_04h | rug_cb | gpu0 | 80.7 | 77.6 | 47 | 131723 | True | inner train 1 windows (purged), calibration ['s05d_11h'] |
| fold_s04d_04h | rug_rfcpu | cpu | 147.8 | 69.2 | - | 131723 | True | inner train 1 windows (purged), calibration ['s05d_11h'] |
| fold_s05d_11h | baseline_lr | cpu | 12.5 | 8.5 | - | 37403 | True | single window: time split at 75% quantile (no purge) |
| fold_s05d_11h | entry_bin | gpu0 | 80.4 | 44.3 | 124 | 37403 | True | single window: time split at 75% quantile (no purge) |
| fold_s05d_11h | entry_tp1 | gpu0 | 93.5 | 77.6 | 347 | 37403 | True | single window: time split at 75% quantile (no purge) |
| fold_s05d_11h | hazard | gpu0 | 33.7 | 31.7 | 34 | 8920 | False | single window: time split at 75% quantile (no purge) |
| fold_s05d_11h | hold | gpu0 | 63.7 | 60.9 | 52 | 57443 | True | single window: time split at 75% quantile (no purge) |
| fold_s05d_11h | rug_cb | gpu0 | 98.3 | 87.6 | 151 | 57443 | True | single window: time split at 75% quantile (no purge) |
| fold_s05d_11h | rug_rfcpu | cpu | 36.8 | 20.0 | - | 57443 | True | single window: time split at 75% quantile (no purge) |
