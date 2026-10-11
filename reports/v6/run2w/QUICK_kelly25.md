# V5 results (EXPLORATORY, pre-gate, paper only)

Test windows (walk-forward, out-of-sample models): ["s02d_14h"]

## Summary (latency 2 slots, base costs)

| metric | value |
|---|---|
| costs_sol | 0.012 |
| expectancy_excl_top1pct_sol | 0.006 |
| expectancy_sol | 0.014 |
| max_drawdown_pct_worst | 6.904 |
| mean_return_pct | 20.033 |
| median_return_pct | 20.033 |
| moonshot_capture | 0.000 |
| positive_windows | 1.000 |
| profit_factor | 14.755 |
| runners_10x | 83.000 |
| runners_captured_5x | 0.000 |
| total_pnl_sol | 0.169 |
| trades | 12.000 |
| win_rate | 0.500 |
| windows | 1.000 |

## Acceptance (V5 §9)

| criterion | pass |
|---|---|
| all_pass | false |
| expectancy_excl_top1pct_positive | true |
| moonshot_capture_ge_20pct | false |
| positive_at_latency4_fail030 | true |
| positive_windows_ge_60pct | true |
| profit_factor_gt_1_3 | true |

## Per window

| window | return % | trades | win rate | PF | exp. SOL | exp. excl top1% | max DD % | costs SOL | 10x runners | caught >=5x | final exits |
|---|---|---|---|---|---|---|---|---|---|---|---|
| s02d_14h | 20.033 | 12 | 0.500 | 14.755 | 0.014 | 0.006 | 6.904 | 0.012 | 83 | 0 | {"AdaptiveTrail":2,"DataEnd":3,"RugModel":2,"TimeStop":5} |

## Sensitivity

| variant | total PnL SOL | PF | median window % | win rate |
|---|---|---|---|---|
| latency1 | 0.153 | 7.125 | 17.751 | 0.227 |
| latency4 | 0.148 | 17.445 | 17.524 | 0.545 |
| fail030 | 0.166 | 20.040 | 19.635 | 0.545 |
| tip_x2 | 0.149 | 14.232 | 17.641 | 0.545 |
| latency4_fail030 | 0.149 | 17.304 | 17.559 | 0.545 |

## Monte Carlo

| quantity | p5 | p25 | p50 | p75 | p95 | mean | P(<=0) |
|---|---|---|---|---|---|---|---|
| boot_expectancy_sol | 0.003 | 0.007 | 0.013 | 0.019 | 0.030 | 0.014 | 0.006 |
| boot_mean_window_return_pct | 20.033 | 20.033 | 20.033 | 20.033 | 20.033 | 20.033 | 0.000 |
| boot_profit_factor | 2.933 | 7.787 | 13.876 | 23.424 | 46.240 | 18.313 | 0.000 |
| boot_win_rate | 0.250 | 0.417 | 0.500 | 0.583 | 0.750 | 0.499 | 0.000 |
| exec_median_window_return_pct | 19.823 | 19.823 | 21.006 | 21.006 | 21.006 | 20.415 | 0.000 |
| exec_profit_factor | 46.454 | 46.454 | 46.454 | 46.454 | 46.454 | 46.454 | 0.000 |
| exec_total_pnl_sol | 0.168 | 0.168 | 0.177 | 0.177 | 0.177 | 0.172 | 0.000 |
| exec_win_rate | 0.857 | 0.857 | 1.000 | 1.000 | 1.000 | 0.929 | 0.000 |

execution reruns: 2, bootstrap resamples: 10000

Strategy: `{"bag_trail":0.55,"bet_equity_frac":null,"bet_sol":0.04685,"c18_venues":true,"d_max":0.6,"d_min":0.15,"dead_exit":0.699999988079071,"decision_rows":true,"decision_tick_s":2,"dev_kill_min_frac":0.5,"disaster_stop":null,"entry_on_watch":false,"ev_min":0.0,"exit_mode":"adaptive_trail","fade_min_hold_s":10,"fade_persist_s":10,"fade_rate_frac":0.25,"fade_size_frac":0.5,"fade_tau_s":10.0,"hold_min":0.4000000059604645,"idle_s":120,"kelly_frac":0.25,"kelly_min_bet_sol":0.02,"loss_streak":0,"loss_streak_pause_s":900,"lp_drop_frac":0.6,"max_bundle_share":0.30000001192092896,"max_entries_per_min":1000000,"max_entry_ret":0.1,"max_hold_s":36000,"max_loss_frac":null,"max_positions":18,"max_ret_60s_entry":0.5,"max_top10_share":0.5,"max_total_exposure":1.0,"mint_gate":true,"pre_tp_stop":0.35,"reentry_cooldown_s":300,"replace_margin":0.15,"rug_veto":0.5,"stop_grace_s":20,"time_stop_gain":0.1,"time_stop_max_buyers60":3.0,"time_stop_s":300,"tp1_mult":2.0,"tp1_sell_frac":0.5,"tp2_mult":5.0,"tp2_sell_frac":0.25,"trail_fixed":0.3,"trail_k":3.0}`

Costs: `{"ata_rent_lamports":2074080,"buy_fail_prob":0.15,"buy_slippage":0.2,"default_fee_bps":125,"fail_seed":0,"latency_slots":2,"priority_lamports":195000,"sell_fail_prob":0.05,"sell_max_tries":3,"sell_slippage":0.3,"start_sol":0.8433,"tip_lamports":300000,"tx_base_lamports":5000}`
