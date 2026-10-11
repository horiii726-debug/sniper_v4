# V5 results (EXPLORATORY, pre-gate, paper only)

Test windows (walk-forward, out-of-sample models): ["bf_1h"]

## Summary (latency 2 slots, base costs)

| metric | value |
|---|---|
| costs_sol | 0.014 |
| expectancy_excl_top1pct_sol | -0.015 |
| expectancy_sol | -0.012 |
| max_drawdown_pct_worst | 21.445 |
| mean_return_pct | -17.010 |
| median_return_pct | -17.010 |
| moonshot_capture | 0.000 |
| positive_windows | 0.000 |
| profit_factor | 0.135 |
| runners_10x | 28.000 |
| runners_captured_5x | 0.000 |
| total_pnl_sol | -0.142 |
| trades | 12.000 |
| win_rate | 0.333 |
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
| bf_1h | -17.010 | 12 | 0.333 | 0.135 | -0.012 | -0.015 | 21.445 | 0.014 | 28 | 0 | {"DataEnd":7,"LpDrop":2,"RugModel":1,"Trail":2} |

## Sensitivity

| variant | total PnL SOL | PF | median window % | win rate |
|---|---|---|---|---|
| latency1 | -0.133 | 0.019 | -16.148 | 0.333 |
| latency4 | -0.115 | 0.018 | -13.708 | 0.273 |
| fail030 | -0.185 | 0.070 | -22.364 | 0.267 |
| tip_x2 | -0.207 | 0.049 | -24.717 | 0.231 |
| latency4_fail030 | -0.123 | 0.008 | -14.835 | 0.273 |

Strategy: `{"bag_trail":0.55,"bet_sol":0.05,"d_max":0.6,"d_min":0.15,"dead_exit":0.699999988079071,"decision_rows":true,"decision_tick_s":2,"dev_kill_min_frac":0.5,"disaster_stop":null,"ev_min":-0.2,"exit_mode":"trail","fade_min_hold_s":10,"fade_persist_s":10,"fade_rate_frac":0.25,"fade_size_frac":0.5,"fade_tau_s":10.0,"hold_min":0.4000000059604645,"idle_s":120,"lp_drop_frac":0.6,"max_bundle_share":0.30000001192092896,"max_entries_per_min":3,"max_entry_ret":0.1,"max_hold_s":36000,"max_positions":18,"max_ret_60s_entry":0.5,"max_top10_share":0.5,"max_total_exposure":0.6,"mint_gate":true,"pre_tp_stop":0.35,"reentry_cooldown_s":300,"replace_margin":0.15,"rug_veto":0.5,"stop_grace_s":20,"time_stop_gain":0.1,"time_stop_max_buyers60":1.0,"time_stop_s":0,"tp1_mult":2.0,"tp1_sell_frac":0.5,"tp2_mult":5.0,"tp2_sell_frac":0.25,"trail_fixed":0.3,"trail_k":3.0}`

Costs: `{"ata_rent_lamports":2074080,"buy_fail_prob":0.15,"buy_slippage":0.2,"default_fee_bps":125,"fail_seed":0,"latency_slots":2,"priority_lamports":195000,"sell_fail_prob":0.05,"sell_max_tries":3,"sell_slippage":0.3,"start_sol":0.8433,"tip_lamports":300000,"tx_base_lamports":5000}`
