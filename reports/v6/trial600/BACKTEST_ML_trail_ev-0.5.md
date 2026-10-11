# V5 results (EXPLORATORY, pre-gate, paper only)

Test windows (walk-forward, out-of-sample models): ["bf_1h"]

## Summary (latency 2 slots, base costs)

| metric | value |
|---|---|
| costs_sol | 0.012 |
| expectancy_excl_top1pct_sol | -0.008 |
| expectancy_sol | -0.007 |
| max_drawdown_pct_worst | 10.526 |
| mean_return_pct | -8.669 |
| median_return_pct | -8.669 |
| moonshot_capture | 0.000 |
| positive_windows | 0.000 |
| profit_factor | 0.111 |
| runners_10x | 28.000 |
| runners_captured_5x | 0.000 |
| total_pnl_sol | -0.073 |
| trades | 11.000 |
| win_rate | 0.273 |
| windows | 1.000 |

## Acceptance (V5 §9)

| criterion | pass |
|---|---|
| all_pass | false |
| expectancy_excl_top1pct_positive | false |
| moonshot_capture_ge_20pct | false |
| positive_at_latency4_fail030 | false |
| positive_windows_ge_60pct | false |
| profit_factor_gt_1_3 | false |

## Per window

| window | return % | trades | win rate | PF | exp. SOL | exp. excl top1% | max DD % | costs SOL | 10x runners | caught >=5x | final exits |
|---|---|---|---|---|---|---|---|---|---|---|---|
| bf_1h | -8.669 | 11 | 0.273 | 0.111 | -0.007 | -0.008 | 10.526 | 0.012 | 28 | 0 | {"DataEnd":8,"DevSell":1,"LpDrop":1,"Trail":1} |

## Sensitivity

| variant | total PnL SOL | PF | median window % | win rate |
|---|---|---|---|---|
| latency1 | -0.101 | 0.032 | -12.105 | 0.308 |
| latency4 | -0.126 | 0.023 | -15.082 | 0.250 |
| fail030 | -0.125 | 0.083 | -15.247 | 0.364 |
| tip_x2 | -0.079 | 0.090 | -9.487 | 0.091 |
| latency4_fail030 | -0.187 | 0.017 | -22.386 | 0.286 |

Strategy: `{"bag_trail":0.55,"bet_sol":0.05,"d_max":0.6,"d_min":0.15,"dead_exit":0.699999988079071,"decision_rows":true,"decision_tick_s":2,"dev_kill_min_frac":0.5,"disaster_stop":null,"ev_min":-0.5,"exit_mode":"trail","fade_min_hold_s":10,"fade_persist_s":10,"fade_rate_frac":0.25,"fade_size_frac":0.5,"fade_tau_s":10.0,"hold_min":0.4000000059604645,"idle_s":120,"lp_drop_frac":0.6,"max_bundle_share":0.30000001192092896,"max_entries_per_min":3,"max_entry_ret":0.1,"max_hold_s":36000,"max_positions":18,"max_ret_60s_entry":0.5,"max_top10_share":0.5,"max_total_exposure":0.6,"mint_gate":true,"pre_tp_stop":0.35,"reentry_cooldown_s":300,"replace_margin":0.15,"rug_veto":0.5,"stop_grace_s":20,"time_stop_gain":0.1,"time_stop_max_buyers60":1.0,"time_stop_s":0,"tp1_mult":2.0,"tp1_sell_frac":0.5,"tp2_mult":5.0,"tp2_sell_frac":0.25,"trail_fixed":0.3,"trail_k":3.0}`

Costs: `{"ata_rent_lamports":2074080,"buy_fail_prob":0.15,"buy_slippage":0.2,"default_fee_bps":125,"fail_seed":0,"latency_slots":2,"priority_lamports":195000,"sell_fail_prob":0.05,"sell_max_tries":3,"sell_slippage":0.3,"start_sol":0.8433,"tip_lamports":300000,"tx_base_lamports":5000}`
