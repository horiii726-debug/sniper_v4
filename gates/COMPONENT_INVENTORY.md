# Component inventory V4 -> V6 (branch v6-impl, 2026-10-11)

Branch check: origin/main (V4, 13 commits), origin/v5-upgrade (V5, 14), origin/v6-components (15) are fully contained in
v6-impl (0 commits and 0 files missing). Code: 10 Rust crates (16.6k lines), Python offline 2.6k lines, 28 CLI commands.

Status: ACTIVE = in the V6 pipeline; LEGACY = kept as comparison/regression, replaced after a test; PENDING = exists,
gate test not yet run on V6 data; OUT = cannot be measured on history (live disabled).

## Data / infrastructure
| Component | Where | Status |
|---|---|---|
| Live recorder (gRPC LaserStream, WebSocket, RPC) | feed/grpc.rs, ws.rs, recorder | PENDING (LaserStream rejected on Free plan) |
| Historical backfill getBlock (+ program-agnostic leg filter) | feed/backfill.rs | ACTIVE, C1/C2 |
| Replay + deterministic hash | replay, events | ACTIVE |
| Follow-up per coin (10 h labels) | feed/follow.rs, app/follow.rs | LEGACY (too expensive on Free plan; replaced by 1 h extension) |
| Extension backfill + decision window | main.rs ExtendWindow | ACTIVE |
| Census (venue activity) | app/census.rs | ACTIVE (diagnostic) |
| Program verification (verify-chain) | main.rs | ACTIVE |

## Decoding
| pump.fun curve + PumpSwap (IDL, event logs) | decoder/pump.rs, idl.rs | ACTIVE, C3a |
| Creation / migration events (authoritative births) | decoder/pump.rs | ACTIVE |
| Balance-delta trader decoder | decoder/balance_delta.rs | ACTIVE |
| Single-venue decoder (Raydium AMM/CPMM/CLMM/LaunchLab, Meteora DLMM/DAMM/DBC, Orca, Moonshot) | decoder/venue_pool.rs | ACTIVE |
| Pool legs (any program, routes, USDC/USDT quotes, native curves) | decoder/pool_legs.rs | ACTIVE, C3b |
| Arbitrage round-trip tagging | app/backtest.rs | ACTIVE |
| On-chain SOL/USD | app/backtest.rs | ACTIVE, C3c |

## Radar
| V6 radar: new/old coin paths, flow vs liquidity, clusters, dust, WATCH/HOT/Decide | radar/lib.rs, app/radar_feed.rs | ACTIVE, C5 (partial FAIL) |
| CUSUM change-point (robust, population baseline) | radar/cusum.rs | PENDING (no effect measured so far) |
| V4 radar (frozen reference) | radar/v4_ref.rs, radar_params_v4_legacy.json | LEGACY (worse in every ablation) |
| Radar tuning / eval (tiers, recall before +10 %) | app/radar_eval.rs | ACTIVE |

## Features (130, one causal Rust engine ml_sim::Engine)
Groups: flow counts, buy/sell size, acceleration/EWMA, price returns, trader mix, dev/creator, curve/pool, bundles /
concentration, wash/multi-sell, holders, slot bursts, Hawkes arrivals, smart wallets, crowd growth, order flow vs
liquidity (OFI, CVD, Kyle lambda, vol), overhang, regime, trader speed, radar state, radar CUSUM, radar clusters/NFL,
radar dust/split. Status: causality C4 PASS; per-group value C6 PENDING (only price returns proven so far).

## Wallet / safety
| Wallet scores, bots, flippers, snipers, clusters | python/research/wallet_scores.py, smart_wallets.py | PENDING (needs more windows) |
| Dev family (linked wallets, cumulative kill switch) | ml_sim dev_family | ACTIVE (no dev_family.csv yet: creator only) |
| Mint safety (freeze/mint authority, Token-2022 hooks/fees/delegate) | app/mint_safety.rs | ACTIVE, C7b |
| Observed honeypot / LP-rug / dev-dump analysis | python/research/safety_labels.py | ACTIVE (diagnostic) |
| Sell simulation before buy | - | OUT (needs wallet simulateTransaction) |

## ML (Python training, Rust inference)
| V5 models: entry_bin (7-class), entry_tp1, rug (CatBoost + RF), hold, hazard, logistic baseline; isotonic calibration; walk-forward with purge | python/ml/v5_train.py | ACTIVE, C8 |
| Model families: logistic, PCA + logistic, RF, CatBoost | python/ml/model_families*.py | ACTIVE |
| Feature-group ablation | python/ml/feature_ablation.py | ACTIVE |
| Rust CatBoost inference + Python parity | crates/model, python/ml/model_parity.py | ACTIVE (live path); backtests use score files |
| V4 models / nested LOWO / compare_models / train_eval | python/ml/*.py, config/ml_strategy_params_*.json | LEGACY |

## Strategy / execution
| V5/V6 allocator: decision tick 2 s, EV ranking, replacement, caps | app/strategy_v5.rs | ACTIVE, C9/C10 |
| Price guard P5 (buy before the rise), cooldown, per-minute cap | strategy_v5.rs | ACTIVE |
| Exits: U11 Peak Trailing, U12 Adaptive Trailing, U13 Fade + drawdown, Momentum Fade, Ladder (comparison), U14 emergency (DevSell, LpDrop, RugModel, Hazard) | strategy_v5.rs | ACTIVE, C11 |
| Time stop: classic and flow-confirmed | strategy_v5.rs | ACTIVE (flow version better) |
| Account risk E9 (loss limit, loss-streak pause) | strategy_v5.rs | PENDING (never triggered yet) |
| Entry on WATCH (M1 meta-labeling) | strategy_v5.rs | PENDING (needs ML) |
| Wallet execution sim: latency, fees, tip, buy fail, slippage limits, sell retries, ATA rent | strategy_v5.rs | ACTIVE, C14 |
| Fractional Kelly sizing (M9) | - | NOT BUILT (allowed only after calibration PASS, C12) |
| V4 ML strategy (exits DumpPredicted, StrongReversal, MomentumExhausted, Scout, GiveBackFromPeak, HardStop) | app/ml_sim.rs simulate | LEGACY (spec: off in momentum mode); re-test on V6 data PENDING |
| V4 rule backtest (MEE exit, trailing) | app/backtest.rs | LEGACY (comparison) |
| Live swap execution / Jito / latency | - | OUT (live disabled until full validation) |
