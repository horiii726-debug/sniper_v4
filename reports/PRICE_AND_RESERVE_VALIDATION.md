# Price and reserve definitions + validation — Phase 1.7

Every price used by radar, features, labels and simulation is one of these (units: lamports per raw token unit; pump.fun and
PumpSwap pump tokens have 6 decimals, so ratios between prices of one coin are unit-free; absolute prices are never compared across mints).

| Name | Definition | Used by |
|---|---|---|
| `marginal_price_post` | `quote_res / token_res` after the trade. Curve: virtual SOL / virtual token. PumpSwap: (real quote + signed virtual quote) / base reserve | ML price paths, labels, `ret_*`, `price_vs_*` |
| `trade_price_pool` | `pool_sol / tokens` (fees excluded both sides) | radar `lift` rule and backtest per-trade price (was trader-side `sol / tokens`, biased ±1–1.5% by fees) |
| `exit_value_for_size` | `sell_quote_out(quote_res, token_res, tokens, fee_bps)`: exact constant-product proceeds after fees for the position size | simulation fills and mark-to-market |
| `entry_tokens_for_size` | `buy_tokens_out(quote_res, token_res, lamports, fee_bps)` | simulation fills |

Validation (fresh data, slots 457793073–457793222): reserve state continuity 100% (curve, non-mayhem), 99.0–100% (PumpSwap per
instruction); exact pool math within 0.1% on 99.65–100% of trades per path (see DECODER_VALIDATION.md §1.5).

Known limits (later phases): `price_vs_vwap60` is a mean of per-event prices, not a VWAP (Phase 3/4 feature registry);
selling on a `complete` curve must be rejected in fills (Phase 6.1); mayhem mints excluded (curve virtual price not backed by real SOL).
