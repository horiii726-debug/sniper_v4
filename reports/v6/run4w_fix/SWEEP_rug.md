Test windows: s02d_14h, s04d_04h, s05d_11h; decision time 148 min in total.

| variant | 100 USD -> (chained) | per window | trades | win rate | PF | max DD % | 10x coins | caught >= 5x |
|---|---|---|---|---|---|---|---|---|
| rug_exit_off | 168.4 | [106.7, 99.5, 158.6] | 173 | 28% | 1.30 | 70.2 | 145 | 3 |
| rug_veto_off | 158.5 | [112.6, 99.5, 141.5] | 191 | 30% | 1.23 | 68.5 | 145 | 3 |
| base | 157.8 | [106.2, 99.0, 150.0] | 153 | 33% | 1.34 | 41.9 | 145 | 3 |
| rug_off_guard_off | 115.8 | [235.8, 80.1, 61.3] | 300 | 34% | 1.25 | 50.4 | 144 | 8 |
| rug_veto_off_exit_on | 112.2 | [70.1, 99.0, 161.6] | 135 | 30% | 1.21 | 41.0 | 145 | 3 |

Per-venue funnel of >= 10x coins, variant `rug_veto_off` (last barrier of each coin):

| venue | 10x coins | entered | last barriers |
|---|---|---|---|
| pumpfun_curve | 112 | 11 | ev_below_min 32, price_guard 31, no_score 22, entered 11, low_liquidity 9, no_cash 3, mint_unsafe 2, pool_unknown 1, slots_full 1 |
| meteora_dbc | 21 | 6 | entered 6, price_guard 6, ev_below_min 5, no_score 2, slots_full 1, no_cash 1 |
| meteora_damm_v2 | 10 | 1 | ev_below_min 6, price_guard 2, entered 1, no_score 1 |
| pumpswap_amm | 2 | 0 | ev_below_min 2 |
