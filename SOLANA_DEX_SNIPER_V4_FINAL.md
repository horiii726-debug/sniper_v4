# SOLANA DEX MEMECOIN SNIPER — V4 FINAL
## On-Chain DEX-Universe Radar + Fast Momentum + Scale-In + Full Momentum Exit + DEV Kill-Switch + Mandatory Stage-Gate Development

Version: **V4** (supersedes `PRE_GMGN_SOLANA_SNIPER_V3_FINAL.md`)
Date: 2026-10-10

---

# 0. HOW TO USE THIS DOCUMENT

- This is the single implementation spec. V3 is fully merged into it, so V3 is not needed alongside it.
- Development is **STAGE-GATED** (§31). A stage may not start until the previous stage's gate is **PASS** and its gate report is committed.
- `MUST` / `MUST NOT` are mandatory.
- **Engineering targets** (latency, recall, accuracy) are concrete numbers. They may be **tightened** freely. Loosening one requires a written justification in the gate report.
- **Strategy parameters** (probability thresholds, multipliers, tranche fractions, exit thresholds) MUST be learned and validated out-of-sample (OOS). They MUST NOT be invented.
- Secrets handling: §30.

## 0.1 V4 changes vs V3

1. **Universe = every Solana DEX pool that DexScreener would list, read directly from the chain.** No DexScreener or GMGN API at runtime. DexScreener is an offline benchmark only.
2. **Three radars.** NEWBORN (launching), ESTABLISHED (already listed), AWAKENING/BREAKOUT (about to move).
3. **Fast feature engine.** Every feature updates on every event. Windows are seconds-scale (slot / 5s / 15s / 30s / 60s), plus fast-vs-slow EWMA, cohort-relative z-scores and CUSUM. The 5m/1h/6h/24h windows are context only and **never the primary trigger**.
4. **Trader-quality layer.** Real makers vs bots/wash, wallet funding clusters, smart-wallet reputation, and bundle/sniper detection.
5. **Sellability / Token-2022 safety checks** before any BUY.
6. **ML inference runs in Rust.** Training is offline.
7. **Scale-in.** Up to 3 buys into the same token, into strength only.
8. **No partial take-profit.** The position is held while momentum lives and exits **100%** when momentum is exhausted, or on a DEV/rug/time/hard-stop trigger.
9. **Re-entry** is allowed through the full entry gate if momentum re-ignites.
10. **Labels target thousands of percent** (10x/20x/50x/100x).
11. **Mandatory stage-gate development.** The Radar gate (Stage 1) is the heaviest.
12. **V3 document defects fixed.** These were duplicate section numbers 42–46, a stray CVD unit test, and broken code fences.

---

# 1. OBJECTIVE

Build a speed-first Solana meme-coin trading system that:

1. Observes **every swap on every supported Solana DEX/launchpad** directly from on-chain data. This covers new launches, already-listed tokens and dormant tokens.
2. Detects within **seconds** whether real trading activity (buy/sell transactions, real traders, real money) is igniting.
3. Estimates the probability of very large expansion (10x, 20x, 50x, 100x+) and of rug or collapse.
4. Enters early, then **adds into strength** when confirmation arrives.
5. Holds while momentum is alive and **exits 100%** when momentum is exhausted.
6. Monitors the creator/DEV and linked wallets continuously, and exits 100% on creator liquidation events.
7. Keeps the hot path extremely small and fast. Everything heavy runs in the background or offline.

Optimization target:

\[
P(OrganicExpansion \mid X_t)\uparrow \;\land\; P(Rug \mid X_t)\downarrow
\]

subject to realistic net capture after slippage, price impact, fees, priority fees, tips, failure probability and latency.

> SPEED FIRST. ONLY CALCULATE INFORMATION NEEDED FOR THE CURRENT DECISION.

---

# 2. CORE PRINCIPLES

1. **On-chain is the only runtime source of truth.** DexScreener and GMGN are never on the decision path.
2. **Event-driven, never polling**, on every critical path.
3. **Two hot paths.** ENTRY/MANAGEMENT and DEV EMERGENCY. Emergency always preempts.
4. **Early observation is not finality.** Preconfirmation and preprocessed data are for speed. Processed/confirmed data is used for reconciliation.
5. **No partial take-profit.** An exit is always 100% of the position. Splitting one exit into several transactions because of liquidity is allowed (§22.6). It is execution, not take-profit.
6. **Pyramid into strength only.** Never average down.
7. **The DEV kill-switch is deterministic** and independent from ML, sizing and momentum.
8. **No feature may use the future** (§27).
9. **Advanced research methods stay offline** until they prove incremental OOS value (§28).
10. **Build in stages. Every stage must pass its gate** (§31).

---

# 3. INFRASTRUCTURE & DATA SOURCES

## 3.1 Feed hierarchy (fastest to slowest)

```text
PRECONFIRMATIONS            (leader executed locally; earliest signal)
RAW SHREDS / SHRED-DERIVED
PREPROCESSED TRANSACTIONS   (decoded shred-derived, before processed)
PROCESSED gRPC (LaserStream / Yellowstone)
WebSocket
RPC polling                 (never on critical path)
DexScreener / GMGN indexing (offline benchmark only)
```

- Solana has **no Ethereum-style public mempool**. Nobody can observe an "intention to rug" before the transaction exists.
- Never claim a latency advantage without measuring it (§24).
- Do not depend on Jito ShredStream. Its sunset was announced for 2026-09-05.

## 3.2 Provider: Helius (current account)

Available on the dashboard:

| Product | Use in this system | Path |
|---|---|---|
| LaserStream gRPC | Primary live stream (processed + historical replay on reconnect) | Hot |
| Preprocessed Transactions | Early signal feed | Hot |
| Preconfirmations | Earliest signal feed | Hot |
| Shreds | Optional earliest raw feed | Hot |
| WebSocket | Fallback stream, account subscriptions | Hot fallback |
| RPC (JSON-RPC) | Blockhash, account fetch on cold start, reconciliation | Warm |
| Sender | Low-latency transaction submission | Execution |
| Enhanced API (parse tx, tx history) | Creator/wallet history, decoder verification | **Offline only** |
| Webhooks | Not used on the hot path | — |

**Plan note.** The account is currently on the **Free plan** ("performance is limited"). LaserStream mainnet, Preprocessed, Preconfirmations and Shreds may require a paid plan, so check Billing.

Development path:

- **Stage 0 (client code):** devnet LaserStream gRPC. Devnet has no real memecoin market, so it only tests the code.
- **Stage 0–1 (real data):** mainnet WebSocket/RPC on the free plan to start recording real data, accepting the limits.
- **Before the Stage 1 performance gate (1F/1G) and before any live stage:** upgrade to a plan with mainnet LaserStream plus an early feed.

Endpoints and keys are loaded from environment variables only (§30). Use the **official LaserStream Rust client** (the dashboard lists Rust, Go and TypeScript clients).

## 3.3 Multi-feed redundancy

```text
EARLY FEED A (preconf) ─┐
EARLY FEED B (preproc) ─┼─> CANONICAL EVENT BUS ─> P0 DEV DISPATCH ─> EMERGENCY EXIT
PROCESSED FEED (gRPC)  ─┘            │
                                     └─> ENTRY / MANAGEMENT PATH
```

- Deduplicate by `signature + instruction index + event kind`.
- Every early observation is reconciled against processed state. Failed transactions and non-canonical signals are retracted.
- On reconnect, resume from the last processed slot (historical replay) and detect and backfill gaps.

## 3.4 Hardware / deployment

- Co-locate in the same region as the feed endpoint (currently `ewr` / Newark). Measure, don't assume.
- Dedicated cores for: feed decode, P0 dispatch, feature/model, execution. Pin threads.
- No GC language on the hot path. Rust only.

---

# 4. VENUE COVERAGE — "THE DEXSCREENER UNIVERSE"

DexScreener lists pools that exist on-chain. Subscribing directly to the same programs gives the same universe **earlier**.

| Venue | Program ID (MUST VERIFY against official source before use) |
|---|---|
| Pump.fun bonding curve | `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P` |
| PumpSwap AMM | `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA` |
| Raydium AMM v4 | `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8` |
| Raydium CPMM | `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C` |
| Raydium CLMM | `CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK` |
| Raydium LaunchLab | `LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj` |
| Meteora DLMM | `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo` |
| Meteora DAMM v2 | `cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG` |
| Meteora Dynamic Bonding Curve | `dbcij3LWUppWqq96dh6gJWwBifmcGfLSB5D4DuSMaqN` |
| Orca Whirlpool | `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc` |
| Moonshot | `MoonCVVNZFSYkqNXP6bxHLPL6QQJiMagDL3qcqUQTrG` |

Rules:

- Program IDs are config, not code constants. The verification source and date are recorded in config.
- Each venue gets its own decoder, pool-math module and transaction builder. Everything shares one engine.
- Unknown instructions on a supported program are counted and alerted (gate metric §31, Stage 1A).
- Aggregator-routed swaps (e.g. Jupiter) are observed through the inner venue instructions.

---

# 5. ARCHITECTURE OVERVIEW

```text
                         SOLANA (all supported DEX programs)
                                        |
                          EARLIEST PRACTICAL FEEDS (§3)
                                        |
                              CANONICAL EVENT BUS
                     +------------------+------------------+
                     |                                     |
              ENTRY/MANAGEMENT PATH                  DEV P0 PATH (§18)
                     |                                     |
              VENUE DECODERS                         creator / linked
                     |                               watchlist lookup
              TOKEN UNIVERSE (HOT/WARM/COLD)               |
                     |                               SELL / LP REMOVE?
     +---------------+----------------+                    |
     |               |                |                KILL-SWITCH
  RADAR 1         RADAR 2          RADAR 3                 |
  NEWBORN       ESTABLISHED   AWAKENING/BREAKOUT       EXIT 100%
     +---------------+----------------+
                     |
          FAST FEATURES (§11) + TRADER QUALITY (§12)
          + LAUNCH STRUCTURE (§13) + SAFETY (§14)
          + CREATOR REPUTATION CACHE (§17)
                     |
          RUST MODELS: ROCKET / RUG / CONTINUATION (§19)
                     |
          ENTRY + SCALE-IN (§20)  ── SIZING (§21)
                     |
               POSITION ACTIVE
                     |
          MOMENTUM EXHAUSTION EXIT (§22) ── EXIT 100%
                     |
               RECONCILE / RECORD
```

---

# 6. EVENT PRIORITY

```text
P0 = DEV direct SELL
P1 = DEV/linked LP REMOVE
P2 = DEV-linked wallet SELL (high confidence)
P3 = critical authority/structural change (mint, freeze, Token-2022 extension change)
P4 = suspicious DEV transfer
P5 = swaps on tokens with an OPEN POSITION
P6 = swaps on HOT tokens
P7 = swaps on WARM/COLD tokens
P8 = analytics-only
```

- Higher priority preempts lower.
- P0–P4 run on a separate queue and core and never wait behind P5–P8.

---

# 7. TOKEN UNIVERSE & TIERS

All tokens and pools on supported venues live in RAM, keyed by mint, with per-pool sub-state.

| Tier | Who | Update cost |
|---|---|---|
| **HOT** | Newborn (age < T_newborn); tokens with an open position; tokens whose fast activity crossed the promotion rule | Full features on every event, model eval |
| **WARM** | Active tokens below the HOT rule | Counters + EWMA on every event; features lazily |
| **COLD** | Inactive tokens | Minimal counters; wake on the first event |
| **DEAD** | Rugged / blacklisted / no liquidity | Ignored except DEV/LP events |

Promotion and demotion:

- **COLD/WARM → HOT:** cheap O(1) rules on fast EWMA / CUSUM / cohort z-score (§11). These are generous on purpose: missing a runner is worse than watching a non-runner.
- **HOT → WARM:** only after sustained inactivity, and **never** while a position is open.
- **Blacklist → DEAD:** rugged, honeypot, or DEV-liquidated tokens are permanent for that mint.
- **Memory:** COLD eviction to disk is allowed. The universe index (mint → pool → tier) stays in RAM.

---

# 8. TOKEN LIFECYCLE & THE THREE RADARS

```text
CREATED ─> BONDING_CURVE ─> MIGRATED ─> ESTABLISHED ─> DORMANT ─> AWAKENING ─> EXPANSION
                                                          ^                         |
                                                          +-------------------------+
```

## 8.1 RADAR 1 — NEWBORN (age 0 to ~30 min, or until migration + N min)

- Triggered by `TOKEN_CREATE`, `BONDING_CURVE_CREATE`, `POOL_CREATE`, `INITIAL_LIQUIDITY` and `FIRST_SWAP`.
- TokenState is created in RAM immediately.
- Focus: launch structure (bundles, snipers, DEV share), real-buyer arrival speed, flow and creator reputation.
- **Migration** (bonding curve → AMM pool) is a first-class event. The pools are linked under the same mint and the state is continuous across the migration.

## 8.2 RADAR 2 — ESTABLISHED (already listed / trading)

- Tokens already trading on AMMs.
- DexScreener-style context windows (5m/1h/6h/24h): txns, volume, makers, price change, liquidity, volume/liquidity and volume/market cap.
- Purpose: keep a healthy WARM set and supply baselines for Radar 3.

## 8.3 RADAR 3 — AWAKENING / BREAKOUT (about to move)

- A token of any age whose **fast** activity deviates sharply from its **own slow baseline and its cohort**.
- Signals include fast real-buyer rate vs slow, CUSUM alarm, new-wallet inflow, smart-wallet entries, volume/liquidity spike, sell-side drying up, and price compression followed by breakout.
- Fires in seconds, not minutes.

---

# 9. DECODING & TRADE CLASSIFICATION

For each economic swap:

```text
BUY : target token -> trader ; quote asset -> pool
SELL: target token -> pool   ; quote asset -> trader
```

- **Routed swaps.** One economic trade counts once, not once per hop. Maintain `economic_trade_id`, `route_id` and `pool_interaction_id`.
  - Economic trade → trader, maker and buyer counts.
  - Pool interaction → reserves, liquidity and price impact.
- **Trader identity** = the signer/owner whose token balance changed. It is not the router or program account.
- **Failed transactions** never update economic state. Early-observed transactions that later fail are retracted.
- **Amounts** are kept in raw integer units (u64/u128) plus decimals. Floats are used only for derived features.
- **Reference decoder.** An independent balance-delta decoder (pre/post token and SOL balances) exists for verification (Stage 1A). It is not used for live decisions.

---

# 10. TOKEN STATE (HOT-PATH, RAM)

```text
identity : mint, decimals, token_program (SPL / Token-2022), creator, venue(s), pool(s),
           first_seen_slot, first_seen_ns, lifecycle_state, tier
market   : price (quote/base), liquidity_quote, reserves, market_cap, fdv
counters : buy_count, sell_count, buy_volume, sell_volume   (per window, §11)
makers   : unique_buyers, unique_sellers, new_buyers, real_buyers, real_sellers
fast     : EWMA states (§11.3), CUSUM states (§11.5), slot buckets, last-N ring buffers
structure: creator_balance, creator_share, bundle_share, sniper_share, insider_share,
           top10_share_ex_pools, holder_count
safety   : mint_authority, freeze_authority, token2022_extensions, sellability_flag
dev      : last_creator_event, last_creator_sell, last_creator_transfer, last_lp_action,
           emergency_state
smart    : smart_buyer_count, smart_net_flow, smart_sellers
validity : per-feature validity flags (insufficient data / NaN guard)
```

Every quantity carries explicit units (§29).

---

# 11. FAST FEATURE ENGINE

Principle: **update on every event, in O(1), and never wait for a window to close.**

## 11.1 Window families

| Family | Windows | Purpose |
|---|---|---|
| Slot | current slot, last 4 slots | Bundles, same-slot buyer bursts |
| Short time | 5s, 15s, 30s, 60s | **Primary trigger features** |
| Count | last 20, last 50 economic trades | Robust when time is irregular |
| Context | 5m, 1h, 6h, 24h | DexScreener-style context only |

Time windows are implemented as ring buckets (e.g. 1s buckets for ≤ 60s; 10s buckets for ≤ 5m; 1m buckets beyond). They add and expire in O(1).

## 11.2 Core rates and flow (per window w)

\[
NewBuyerRate_w=\frac{NewRealBuyers_w}{w},\qquad TxRate_w=\frac{EconomicTrades_w}{w}
\]

\[
SignedFlow_w = BuyVolume_w - SellVolume_w
\]

\[
FlowImbalance_w=\frac{BuyVolume_w-SellVolume_w}{BuyVolume_w+SellVolume_w+\epsilon}
\]

If both sides are zero, the value is `0` and the `inactive` flag is set.

Also computed: `BuyMakerShare_w = real_buyers / (real_buyers + real_sellers)`, `MedianBuySize_w`, `MedianSellSize_w` and `VolumeToLiquidity_w`.

## 11.3 EWMA fast vs slow (the acceleration engine)

For an event at time `t` contributing value `v`:

\[
R(t)=R(t_{prev})\,e^{-(t-t_{prev})/\tau}+\frac{v}{\tau}
\]

`R` is a rate per second. The half-life is `h = τ ln 2`.

- Fast: τ_f ∈ {3s, 10s}. Slow: τ_s ∈ {30s, 120s}. These are initial values and are tuned offline.
- They are tracked for real buyers, economic trades, buy volume, sell volume, smart-wallet buys and new wallets.

Acceleration:

\[
AccelRatio=\frac{R_f+\epsilon}{R_s+\epsilon},\qquad AccelDiff=R_f-R_s
\]

These are valid only when the minimum event count is met (validity flag).

## 11.4 Cohort-relative features (for tokens without history)

A newborn has no "own baseline". It is compared with tokens **of the same age** instead.

- Age buckets: 0–10s, 10–30s, 30–60s, 1–2m, 2–5m, 5–15m, 15–60m, 1–6h, >6h.
- A background job maintains rolling robust stats per bucket (median, MAD) from tokens observed **before t**. It refreshes every few seconds and is read lock-free.

\[
z_{cohort}=\frac{x-\mathrm{median}_a}{1.4826\cdot \mathrm{MAD}_a+\epsilon}
\]

Example: "at age 30s this token has 4.1σ more real buyers than a typical newborn."

## 11.5 CUSUM change detection (O(1), hot path)

\[
S_t=\max\big(0,\;S_{t-1}+(x_t-\mu_0-k)\big),\qquad \text{alarm if } S_t>h
\]

- Run on real-buyer arrivals and on buy volume.
- `μ0` is the token's own slow baseline, or the cohort baseline for newborns.
- `k` and `h` are tuned offline. An alarm promotes the token to HOT and is used as a feature.
- V3 placed CUSUM offline. V4 moves it to the hot path because it is O(1). It must still pass Stage 3/5 validation.

## 11.6 Trade arrival

`Δt_i = t_i − t_{i−1}` between economic trades. Store the median, P90 and P99 of Δt over the last N trades, and `time_since_last_real_buy`. These are used for inactivity detection (§22).

## 11.7 Price features

- Return over 5s/15s/30s/60s.
- Distance from local high/low and from rolling VWAP.
- Compression: range over the last N trades divided by its longer-term average, used for breakout detection.

---

# 12. TRADER QUALITY LAYER

Raw transaction and maker counts are easy to fake. V4 decisions use **real** activity.

## 12.1 Real maker filter

A wallet's trade is **not real** (excluded from real counts but kept in raw counts) when any validated rule fires:

- micro-trade below the minimum notional (dust buys used to inflate makers);
- round-trip by the same wallet within a short time or the same slot (wash);
- the wallet is in the same funding cluster as the creator, a bundle or a known bot farm;
- the wallet is a known MEV/sandwich or volume-bot address (background list).

\[
RealMakerRatio=\frac{RealMakers}{TotalMakers+\epsilon}
\]

## 12.2 Wallet age & novelty

- `wallet_first_seen_slot` comes from a background index. If it is unknown, set the `unknown_age` flag. Never treat unknown as zero.
- `NewWalletShare`: the share of buyers never seen trading before t.
- `ReturningWalletShare`: the share of buyers that already traded this token, a sign of churn rather than new money.

## 12.3 Funding clusters

- A background indexer links wallets by their SOL funding source. Rules: a common funder, fanned out within a short time, with similar amounts.
- Live: observed transfers between watched wallets update clusters incrementally.
- `ClusterBuyShare`: the share of buy volume coming from the largest cluster.

## 12.4 Smart-wallet reputation

This works like creator reputation, but for traders. It is computed in the background and cached in RAM.

- Inputs per wallet (history ≤ t only): how early it entered tokens that later reached ≥5x/10x/20x, realized PnL, hold behaviour, sample count, recency weight and coverage.
- Wallets that are insiders of those runners (creator cluster, bundle) are **excluded**. Insider profit is not skill.
- Live features: `smart_buyer_count_fast`, `smart_net_flow_fast` and `smart_sellers_fast`.
- The class follows creator reputation: UNKNOWN / INSUFFICIENT_DATA / PROVEN_POSITIVE / NEUTRAL.

## 12.5 Crowding

`known_sniper_bot_buys`: a heavy competing-bot presence early means worse fills and faster dumps. This is a feature, not a rule.

---

# 13. LAUNCH STRUCTURE (BUNDLES, SNIPERS, CONCENTRATION)

- **Bundle detection.** Buys in the creation slot or the first few slots by wallets linked by funding, timing or amount pattern, or in the same block bundle when observable.

\[
BundleSupplyShare=\frac{\text{tokens acquired by bundle wallets}}{\text{circulating supply}+\epsilon}
\]

- **Sniper share.** The supply held by wallets that bought within the first N slots.
- **Insider share** = creator + linked + bundle + sniper clusters, deduplicated.
- **Holder concentration.** `top10_share_ex_pools` excludes pool, vault and bonding-curve accounts. It is maintained incrementally from pre/post token balances in decoded transactions.
- **Creator share** (continuous, no 99% rule):

\[
DevShare=\frac{DevBalance}{CirculatingSupply+\epsilon}
\]

  60–70% creator control is already severe supply risk.
- **DEV-dominance mode.** Above a validated concentration threshold, DEV/LP events dominate all decisions (§18).

---

# 14. SAFETY & SELLABILITY (CHEAP, PRE-BUY)

Checked at discovery and re-checked on every authority or extension event:

- mint authority (can more supply be minted?);
- freeze authority (can our account be frozen?);
- **Token-2022 extensions:** transfer hook, transfer fee, permanent delegate, default account state frozen, non-transferable, confidential transfer;
- metadata/update authority;
- pool/LP control (who can remove liquidity; whether LP is burned or locked when observable);
- **sellability:** for a token with unusual extensions or programs, simulate a sell offline/warm before the first BUY. A failed simulation means a hard reject.

These are risk features, not automatic verdicts, **except** confirmed non-sellability, which is always a hard reject.

---

# 15. LIQUIDITY, POOL MATH & EXECUTABLE DEPTH

Store `total_liquidity`, `active_liquidity`, `LP_add`, `LP_remove`, `liquidity_change` and `liquidity_velocity`.

Displayed liquidity is **not** executable depth.

## 15.1 CPMM (constant product) and bonding curves using virtual reserves

\[
xy=k,\qquad P_0=\frac{y}{x},\qquad q_{eff}=q(1-f),\qquad \Delta x=\frac{x\,q_{eff}}{y+q_{eff}}
\]

\[
P_{exec}=\frac{q}{\Delta x},\qquad PriceImpact=\frac{P_{exec}-P_0}{P_0}
\]

**Never** use `x*y=k` for concentrated-liquidity (CLMM, Whirlpool) or bin-based (DLMM) pools. Each venue uses its exact pool math, including tick/bin traversal and that venue's fee schedule.

## 15.2 Executable depth

\[
Depth_s=\max(q)\ \text{ s.t. } \left|\frac{P_{exec}(q)}{P_0}-1\right|\le s
\]

Maintain `buy_depth_{0.5%,1%,2%}` and `sell_depth_{0.5%,1%,2%}`. **Sell depth for the full position size** is mandatory before every BUY and every scale-in add.

---

# 16. DEV MONITORING EVENTS

```text
DEV_TRANSFER, DEV_DIRECT_SELL, DEV_LINKED_WALLET_RECEIVES, DEV_LINKED_WALLET_SELL,
LP_REMOVE, LP_ADD, AUTHORITY_CHANGE, MINT, FREEZE-RELATED, TOKEN2022_EXTENSION_CHANGE,
DEX_INTERACTION
```

Derived quantities:

\[
DevTransferRatio=\frac{Transferred}{DevBalanceBefore+\epsilon},\quad
DevSellShareOfPosition=\frac{DevSold}{DevBefore+\epsilon},\quad
LinkedSellShare=\frac{SellVol_{dev+linked}}{TotalSellVol+\epsilon}
\]

\[
LiquidityRemovalRatio=\frac{L_{before}-L_{after}}{L_{before}+\epsilon}
\]

A transfer alone is a **warning** (store the destination, its age, its relationship and the time to its next action). A linked-wallet sell is a strong liquidation signal.

---

# 17. CREATOR REPUTATION ENGINE (PRE-BUY, CACHED)

## 17.1 Purpose

The question this engine answers: *Has this creator, or a high-confidence creator-linked cluster, repeatedly launched tokens that produced genuine large expansions without creator-led rugs?*

It is computed in the background and looked up in RAM in O(1) on the live path.

## 17.2 Outputs

`creator_score`, `creator_rug_risk`, `creator_runner_rate`, `creator_sample_count`, `creator_cluster_confidence`, `creator_reputation_class` (UNKNOWN / INSUFFICIENT_DATA / PROVEN_POSITIVE / NEUTRAL / HIGH_RISK) and `creator_history_age`.

A creator is never labelled PROVEN_POSITIVE with a small sample or incomplete coverage.

## 17.3 Historical outcomes per creator/cluster

```text
launch_count
runner_{2x,5x,10x,20x,50x,100x}_count, runner_{500pct,1000pct}_count
median / P75 / P90 / max peak_multiple
median_time_to_{2x,5x,10x}
rug_count, rug_rate, creator_dump_count, creator_lp_remove_count,
linked_cluster_liquidation_count
```

Units: +100% = 2x, +400% = 5x, +500% = 6x, +1000% = 11x. Percent and multiple are never mixed without explicit units.

## 17.4 Rug label policy

The label is reproducible, configurable and sensitivity-tested. Its ingredients are: severe post-launch drawdown, creator or linked liquidation, LP removal, sellability collapse, and coordinated dump.

- "Went down" ≠ rug.
- "Reached 10x" ≠ good creator. Creator behaviour around the outcome matters.

## 17.5 Score

The score uses a transparent feature vector: `log1p(launch_count)`, runner rates, rug/dump/LP-remove rates, median peak, recency-weighted rates (\(w_i=e^{-\lambda\Delta t_i}\), with λ never chosen on the test set), cluster confidence and history coverage.

- The score is either a calibrated model or a transparent weighted score, and it is validated OOS.
- `0 observed rugs` with incomplete history ≠ `proven zero rugs`. Coverage metadata is mandatory: observed launches, observed wallets, period, source coverage, unknown-wallet probability and cluster confidence.

## 17.6 Decision role

Reputation is a **prior/risk modifier, never a BUY trigger**.

- Good creator + weak market → no buy.
- Unknown creator + excellent market → may qualify.
- Bad creator + excellent market → strong penalty or hard reject (when validated).

## 17.7 Wallet cluster discovery

The creator wallet is always the primary identity. Linkage evidence includes:

- common funder;
- repeated transfers to or from the creator;
- same-launch participation;
- timing;
- coordinated selling across launches.

Each link gets `link_confidence ∈ [0,1]`. Only wallets above the OOS-validated threshold become emergency candidates.

---

# 18. DEV KILL-SWITCH (P0) & LIVE WATCHLIST

## 18.1 Watchlist (created at BUY, in RAM)

```text
creator wallet                    PRIORITY 0  (always, even if it holds 1 token or 0)
high-confidence linked wallets    PRIORITY 1
LP/pool controller                PRIORITY 1
authority accounts                PRIORITY 2
newly discovered related wallets  PRIORITY 2
```

## 18.2 Hot path

```text
EARLIEST OBSERVABLE TX -> DECODE -> CREATOR/LINKED ID CHECK -> TARGET-TOKEN SELL / LP REMOVE?
  -> KILL-SWITCH -> BUILD MINIMAL SELL TX (pre-armed) -> SIGN -> SEND VIA ALL FAST PATHS
```

The kill-switch does **not** wait for ML, buyer decline, candle close, CVD, DexScreener, GMGN, a database query or a graph rebuild.

## 18.3 Rules

- **Validated direct creator SELL of the target token → EXIT 100%,** whatever the size (0.1% or 100%). The trigger is identity + action, not size.
- **High-confidence linked-wallet SELL → EXIT 100%.** A lower-confidence link → DEV_WARNING, which feeds the exhaustion model and blocks scale-in.
- **Creator-linked LP REMOVE → EXIT 100%.**
- **DEV transfer to a new wallet:** record it, classify it, add the destination to the watchlist **immediately** (asynchronous enrichment later), and move to DEV_ARMED. Scale-in is blocked.
- Transfer and sell in the same transaction: react as early as observable. Never claim front-running.

## 18.4 State machine

```text
NORMAL -> DEV_WARNING -> DEV_ARMED -> DEV_LIQUIDATION -> RUG_EMERGENCY -> POSITION_CLOSED
```

No lower-priority event can reset a higher state. Dynamic sizing never overrides the kill-switch.

## 18.5 False-positive control

The identity detector must be precise. It is tested against: treasury transfer, creator direct sale, linked sale, unrelated whale sale, router intermediate account, LP rebalance, legitimate LP add/remove, failed tx, reverted tx, non-canonical early signal, and reconciliation correction.

A whale sell, a price fall or a large holder transfer alone is **not** a creator-rug trigger.

---

# 19. MODELS (TRAINED OFFLINE, INFERENCE IN RUST)

## 19.1 Models

| Model | Output | Used for |
|---|---|---|
| **Rocket** | P(peak ≥ m× within H), m ∈ {2,5,10,20,50,100} | Entry, scale-in, sizing |
| **Rug** | P(rug within H_r) | Entry veto, sizing, add veto |
| **Continuation** | P(momentum continues over next Δ), i.e. P(price higher by ≥ a before lower by b) | Scale-in and **exit** |

Monotonicity is enforced: P(≥10x) ≤ P(≥5x) ≤ P(≥2x).

## 19.2 Features

All features from §11–§17: fast rates/EWMA/acceleration/cohort z/CUSUM, flow, real makers, smart wallets, wallet novelty, clusters, bundle/sniper/insider shares, DEV share, safety flags, liquidity/depth/impact, creator reputation and lifecycle phase. Context windows (5m/1h/6h/24h) are included as context features.

## 19.3 Why CatBoost

The data has non-linear interactions, mixed numeric/categorical inputs and missing values. CatBoost must still **beat the baselines OOS** (Stage 5). It is never assumed to be profitable.

## 19.4 Rust inference

- Training happens in Python offline. The model is exported and inference runs in Rust, through the official CatBoost model-apply binding or ONNX (e.g. the `ort` crate).
- Every model file carries a version hash. Hot reload is atomic. Python and Rust predictions must match (Stage 6 gate).
- Evaluation throttle per HOT token: on every significant state change, at most once per slot. Open positions are always evaluated on every event.

## 19.5 Labels

- Measured from **decision time t**, using the **executable** price reachable at t + realistic latency (not mid price).
- `peak_multiple_H`, `reached_m×_before_rug`, `time_to_m×`, `rug_H` and `continuation_Δ`.
- Future data is used only for labels.

---

# 20. ENTRY & SCALE-IN (UP TO 3 BUYS, INTO STRENGTH ONLY)

## 20.1 Entry gate (every buy)

```text
safety pass (sellable, no hard reject)
  -> emergency_state == NORMAL
  -> creator not hard-rejected
  -> Rocket/Rug/Continuation from Rust models
  -> NetEV > 0 (§23)
  -> full-position sell depth OK
  -> portfolio limits OK
  -> BUY
```

## 20.2 Tranches

```text
T1 SCOUT     earliest strong signal; small fraction s1 of per-coin cap
T2 CONFIRM   real makers keep rising, flow positive, smart wallets entering,
             no DEV warning, P(≥10x) up vs T1
T3 BREAKOUT  acceleration persists / breakout of compression / post-migration strength,
             continuation high, depth sufficient for TOTAL position
```

`s1 + s2 + s3 ≤ 1` of the per-coin cap. The fractions are learned OOS.

## 20.3 Add rules (all MUST hold for T2/T3)

1. `emergency_state == NORMAL` (no DEV_WARNING/ARMED).
2. P(rug) now ≤ P(rug) at the previous fill + δ, and ≤ the rug cap.
3. P(≥10x) or Continuation **improved** vs the previous fill.
4. Real-buyer fast rate and flow are positive over the fast window.
5. **Price ≥ previous fill price.** Pyramid up, never average down.
6. Minimum time/slots since the last fill (no self-chasing in one burst).
7. Price impact of the add ≤ the limit. Distance above fast VWAP ≤ the limit (anti-chase, learned).
8. Sell depth for the **total position after the add** is within the slippage limit.
9. Per-coin cap and portfolio cap respected.

## 20.4 After every fill

- The pre-armed sell context is rebuilt for the **total** position (§23.2).
- The watchlist and emergency state persist across tranches.
- Average entry and per-tranche fills are recorded.

## 20.5 Validation requirement

Scale-in must beat single-entry on OOS risk-adjusted results (Stage 5). Otherwise it is disabled (T1 only, at the full size allowed by sizing).

---

# 21. POSITION SIZING — DYNAMIC KELLY FRAMEWORK

Binary approximation:

\[
f^*=\frac{bp-q}{b},\qquad f_{kelly}=\alpha f^*,\ 0<\alpha<1
\]

- Prefer an empirical expected-log-growth formulation over the modelled multi-outcome return distribution.
- Kelly is only a proposal.

\[
Risk_{raw}=f_{kelly}\times M_{creator}\times M_{market}\times M_{rug}
\]

\[
Risk_{final}=\min(Risk_{raw},Risk_{max},Risk_{depth},Risk_{slippage},Risk_{portfolio})
\]

- `M_creator = g(CreatorScore, SampleSize, Confidence)`. The sample-size term stops one lucky launch from creating oversized risk.
- `M_market = h(X_t)` is driven by fast activity, real makers, flow, smart wallets, liquidity and depth.
- `M_rug` decreases with P(rug), bundle/insider share and DEV share.
- `Risk_final` is the **per-coin cap across all tranches**.
- A good creator never overrides an illiquid or weak market. Sizing never overrides the kill-switch.

---

# 22. EXIT — FULL EXIT ON MOMENTUM EXHAUSTION (NO PARTIAL TAKE-PROFIT)

## 22.1 Policy

- **No fixed take-profit and no partial take-profit.** Selling into strength in pieces gives away the tail (the 1000%+ moves).
- The position is held at 100% while momentum is alive. **All of it** exits when one exit trigger fires.

## 22.2 Exit trigger hierarchy

```text
1. RUG_EMERGENCY / DEV_LIQUIDATION (§18)          -> EXIT 100% immediately
2. Sellability deterioration / critical structure  -> EXIT 100% immediately
3. Hard catastrophic stop (validated, vs avg entry) -> EXIT 100% immediately
4. Momentum Exhaustion Exit (MEE, §22.3)           -> EXIT 100%
5. Scout time-stop: T1 not confirmed within T_scout -> EXIT 100%
6. Inactivity exit (§22.4)                          -> EXIT 100%
```

## 22.3 Momentum Exhaustion Exit (MEE)

These components are computed per event in O(1). Each is compared with the **post-entry momentum peak** and with the slow baseline.

| ID | Signal | Meaning |
|---|---|---|
| E1 | Fast real-buyer rate ÷ its post-entry peak, and ÷ slow | New traders drying up |
| E2 | Fast tx rate ÷ post-entry peak, and ÷ slow | Transactions drying up |
| E3 | FlowImbalance_fast < 0 and SignedFlow_fast falling | Money leaving |
| E4 | Sell makers ÷ buy makers rising; sell-rate acceleration | Sellers taking over |
| E5 | Smart-wallet net flow negative / smart sellers appearing | Informed exit |
| E6 | Non-creator top holders / snipers distributing | Distribution |
| E7 | Liquidity falling (non-creator) | Exit capacity shrinking |
| E8 | Failure to make a new high within T while E1–E3 decay; drawdown from post-entry peak | Price structure breaking |
| E9 | Median buy size falling | Retail exhaustion |
| E10 | Time since last real buy > k × recent P90 inter-arrival | Activity stalls |

Decision:

- **Model mode (after Stage 5):**

  \[
  P_{cont}=Continuation(X_t);\quad \text{EXIT if } P_{cont}<\theta_{exit}\ \text{persistently}
  \]

  "Persistently" means for ≥ k consecutive evaluations **and** ≥ T_persist seconds. This stops one noisy tick from shaking the system out.
- **Strong-reversal override (no persistence needed):** a validated combination of simultaneous E3 + E4 + E5/E6, e.g. money flipping negative while smart wallets sell.
- **Rule mode (before models exist, Stages 3–4):** a normalized weighted MEE score built from E1–E10. Weights start equal and are tuned offline.
- `θ_exit`, `k`, `T_persist` and the override combinations are **learned OOS**.

MUST NOT:

- exit on a single noisy tick (except the validated strong-reversal override);
- use a fixed % take-profit;
- sell part of the position as take-profit.

MUST:

- exit 100%;
- cancel any pending scale-in;
- log every component value at exit time for audit.

## 22.4 Inactivity exit

If `time_since_last_real_buy` exceeds a validated dynamic threshold (a multiple of the token's own recent P90 inter-arrival, with a floor), exit 100%.

## 22.5 Re-entry

- Allowed when momentum re-ignites (Radar 3 fires again) and the **full entry gate** passes again.
- A cooldown applies. Re-entries count against the per-coin daily cap.
- **No re-entry** on a mint that had DEV_LIQUIDATION, RUG_EMERGENCY or a sellability failure (blacklisted).

## 22.6 Execution of a 100% exit

- If the full size exceeds sell depth at the allowed slippage, the **single exit decision** may be executed as several back-to-back transactions across slots or pools. This is execution slicing, not partial take-profit. No new hold decision happens between slices.
- Emergency exits accept higher slippage and fees: preserving capital comes first.

---

# 23. EXECUTION

## 23.1 Net EV (normal entry)

\[
NetEV=GrossEV-Slippage-PoolFee-NetworkFee-PriorityFee-Tip-FailureCost
\]

A normal entry requires NetEV > 0. An emergency exit may pay more.

## 23.2 Pre-armed sell context (built at every fill)

Cached and ready: token mint, decimals, token program, venue, pool, route, pool and vault accounts, user token account, quote account, signer, address lookup tables, compute-budget policy, priority-fee policy, preferred and fallback send paths, and a recent blockhash (refreshed continuously).

On emergency:

```text
fresh minimum state -> construct -> sign -> send (all paths)
```

No route search unless no safe precomputed route exists.

## 23.3 Sending

- Helius Sender plus a fallback RPC. Use a Jito tip only when economically justified, except for emergencies.
- Priority fee is dynamic, based on recent landed fees for the same accounts.
- Skip preflight on the hot path. Idempotency guards prevent double-buy or double-sell on duplicate events.
- Confirmation tracking uses the stream, not polling.

## 23.4 Hot-path engineering

- No REST, no DB scan, no heap-heavy allocation and no blocking I/O on hot threads.
- Lock-minimized queues (e.g. crossbeam). P0 has its own queue and core.
- Model and feature state is lock-free and read-mostly.

---

# 24. LATENCY MEASUREMENT

Timestamps (ns): `T_event_slot`, `T_feed_rx`, `T_decode`, `T_state`, `T_decision`, `T_build`, `T_sign`, `T_send`, `T_land`.

\[
DetectionLatency=T_{detect}-T_{event},\quad DecisionLatency=T_{decision}-T_{detect},\quad EndToEnd=T_{land}-T_{event}
\]

\[
ExitLeadLag=T_{ourExitLand}-T_{DevSellLand}\quad(\text{negative = we landed first})
\]

Report P50/P75/P90/P95/P99. Never claim "millisecond protection" from CPU benchmarks alone.

---

# 25. DEXSCREENER BENCHMARK (OFFLINE ONLY)

A separate benchmark process (never the trading process) records, within DexScreener's rate limits and terms:

```text
first_seen_onchain, first_our_detection, our_buy, our_landing,
DexScreener_first_visible, DexScreener_trending_entry (if observable), GMGN_first_visible
```

\[
DexLead=T_{DexVisible}-T_{OurDetection}
\]

Report the distribution (median, P75, P90, P95, P99) over a large sample. Optional offline label: P(enter DexScreener trending).

---

# 26. DATA RECORDING & REPLAY

- Every raw feed message is recorded with receive timestamps. The format is compact binary plus a columnar store (e.g. Parquet/ClickHouse) for research.
- The deterministic replay engine feeds recordings through the **same Rust code** as live trading.
- The replay can simulate feed latency, decision latency, landing delay and pool state at the landing slot.
- Every live decision is logged with its full feature vector, model versions and outcome.

---

# 27. NO FUTURE LEAKAGE

At decision time `t`, every feature satisfies `FeatureTimestamp ≤ t`.

None of the following may be taken from the future: price, transactions, liquidity, holder state, wallet behaviour, wallet links, cluster membership, creator reputation, smart-wallet reputation, cohort statistics, or DexScreener/GMGN visibility.

Splits are chronological walk-forward with an embargo, and creator/cluster-aware (no creator in both train and test of the same fold, where testing generalization).

---

# 28. ADVANCED METHODS — OFFLINE ONLY

The following are research candidates: HMM, Hawkes, Bayesian change point, VPIN / AMM flow toxicity, Mahalanobis anomaly, Hurst/DFA, GARCH, graph embeddings, social/narrative, and cross-DEX lead/lag.

Process: hypothesis → offline implementation → unit test → leakage test → walk-forward OOS → ablation → robustness → KEEP/REJECT.

They must never slow the entry or emergency paths. Only O(1)-cost methods that pass validation may enter the hot path (CUSUM is the precedent).

---

# 29. UNITS & NUMERICAL SAFETY

```text
count, buyers/sec, tx/sec, quote units (lamports / raw), price = quote/base,
return_frac, return_pct, multiple (x), slippage_frac, slippage_bps, latency_ns, latency_ms
```

Every division defines its behaviour for a zero denominator, insufficient history, NaN and Inf. Missing data is **never** silently turned into zero. It always carries a validity flag.

---

# 30. CONFIG & SECRETS

- All endpoints and keys come from environment variables (`.env` locally, a secret manager in production). The names are in `.env.example`.
- `.env` and keypair files are in `.gitignore` and are never committed.
- Secrets are **never logged**. Log redaction is enforced, and a CI test greps logs for key patterns.
- The trading wallet keypair is referenced by path (`WALLET_KEYPAIR_PATH`) and is never stored in the repo or in `.env` content.
- Use a dedicated hot wallet that holds only the capital for the current stage.
- Configure Helius **RPC Access Control Rules** to allow only the server's IPs.
- Rotate the API key if it was ever shared (e.g. in screenshots).

---

# 31. STAGE-GATE DEVELOPMENT (MANDATORY)

## 31.1 Gate rules

1. Stages run **in order**. Stage N+1 starts only after Gate N is **PASS**.
2. Every gate produces `gates/STAGE_<N>_<NAME>.md` containing: commit hash, data window, hardware, every criterion with its measured value and PASS/FAIL, known issues and sign-off.
3. **One MUST criterion FAIL = stage FAIL.** Fix it and re-run the **whole** gate. Partial re-runs are not accepted.
4. **Regression.** CI runs the automated tests of every previously passed gate. A change that breaks an earlier gate cannot merge.
5. No skipping, no "temporary bypass", and no live capital before Stage 9.
6. Targets marked "initial" can be tightened anytime. Loosening one needs a written justification showing the target is infeasible for **any** causal method on the recorded data.

## 31.2 Overview

```text
STAGE 0  Foundations & feed           ─┐
STAGE 1  RADAR  (heaviest gate)        │  no ML, no money
STAGE 2  Safety, DEV & launch structure│
STAGE 3  Fast features & trader quality│
STAGE 4  Dataset & labels              │
STAGE 5  Models offline (OOS)          │
STAGE 6  Rust inference parity        ─┘
STAGE 7  Execution (local validator + micro mainnet tx)
STAGE 8  Shadow mode (live, no capital)
STAGE 9  Micro-live (tiny capital)
STAGE 10 Controlled scale-up
```

---

## STAGE 0 — FOUNDATIONS & FEED

**Deliverables:**

- Cargo workspace (§32).
- Config/env loader, structured logging with redaction, and a monotonic ns clock + slot clock.
- LaserStream client (devnet first, then mainnet), with WS fallback.
- Program-filtered subscriptions for all §4 venues.
- Reconnect with replay from the last slot, and gap detection + backfill.
- Raw recorder and a replay skeleton.

**Gate 0 (MUST):**

**HISTORICAL-FIRST MODE (active).** Stages 0–6 are built and validated on **historical data** (backfilled past slots), not on long live runs. Live-only criteria move to the **Live-Readiness Gate** (below), which MUST pass before Stage 7 mainnet transactions and Stage 8.

| ID | Criterion | Target |
|---|---|---|
| G0.H1 | Historical backfill of a chosen slot range for all §4 programs | 0 missing slots in range (every slot fetched or explicitly confirmed skipped by the cluster) |
| G0.H2 | Backfilled data stored in the same raw format as the live recorder | Same replay code reads both |
| G0.3 | Replay determinism | Same recording replayed twice → byte-identical derived output |
| G0.4 | Secret hygiene | CI log-grep finds 0 secrets; `.env` not tracked |
| G0.H3 | Credit/usage cost per backfilled hour measured | Reported (to plan how much history the Helius plan allows) |

**Live-Readiness Gate (deferred; MUST pass before Stage 7 mainnet tx / Stage 8):**

| ID | Criterion | Target |
|---|---|---|
| G0.1 | 24h continuous mainnet run | 0 unrecovered slot gaps (every gap detected + backfilled, count reported) |
| G0.2 | Forced disconnect ×50 | 100% auto-recovery; 0 duplicate events after dedup |
| G0.5 | Feed latency | Slot-time → receive histogram recorded per feed |
| G1G.* | 72h live soak (Stage 1G) | As defined in 1G |
| G1F.* | Performance re-measured on the live feed | As defined in 1F |

---

## STAGE 1 — RADAR (THE MOST IMPORTANT GATE)

The radar must be **excellent**, not merely working. Every later stage depends on it. All sub-gates are MUST.

**Deliverables:**

- Venue decoders (§4, §9), token/pool discovery and migration linking, and economic-trade aggregation.
- Token universe with tiers (§7) and all window families (§11.1).
- Rule-based (non-ML) HOT promotion.
- Independent reference decoder and Python reference recomputation.

**1A — Decode correctness (per venue)**

| ID | Criterion | Target |
|---|---|---|
| G1A.1 | BUY/SELL side accuracy vs reference decoder on ≥ 10,000 random real swaps per venue | ≥ 99.95% |
| G1A.2 | Amount match (raw integer units) | ≥ 99.9%; every mismatch root-caused |
| G1A.3 | Unknown-instruction rate on supported programs | < 0.1% of venue tx |
| G1A.4 | Spot check vs Helius parsed-tx (offline) on ≥ 500 tx per venue | All discrepancies explained |

**1B — Discovery completeness**

| ID | Criterion | Target |
|---|---|---|
| G1B.1 | New mint/pool recall vs independent block re-scan over ≥ 72h | ≥ 99.9% |
| G1B.2 | Bonding-curve → AMM migration linkage | ≥ 99.5% |
| G1B.3 | Duplicate TokenState after dedup | 0 |

**1C — Economic trade aggregation**

| ID | Criterion | Target |
|---|---|---|
| G1C.1 | Routed/multi-hop swaps (≥ 1,000 tx): one economic trade, correct trader | ≥ 99.5% |
| G1C.2 | Failed/retracted tx never in economic state | 100% |

**1D — State & window parity**

| ID | Criterion | Target |
|---|---|---|
| G1D.1 | Rust live vs Python reference on ≥ 24h recording, all tokens: counts & integer sums | Exact |
| G1D.2 | EWMA/float features | ≤ 1e-9 relative diff |
| G1D.3 | Window expiry correctness (5s…24h) | Exact on synthetic edge-case suite |

**1E — Universe vs DexScreener (offline)**

| ID | Criterion | Target |
|---|---|---|
| G1E.1 | Sample ≥ 1,000 Solana pairs on supported venues from DexScreener: present in our universe | ≥ 99% (missing ones explained; unsupported venue → backlog) |
| G1E.2 | New pairs: our detection earlier than DexScreener first-visible | ≥ 99%; lead-time P50/P90/P99 reported |
| G1E.3 | Txns/volume/makers 5m/1h/24h vs DexScreener | Median abs diff reported; systematic gaps explained |

**1F — Performance (target hardware, replay at 1×/3×/5× recorded peak)**

| ID | Criterion | Target (initial) |
|---|---|---|
| G1F.1 | Feed-receive → state updated at 3× peak | p50 ≤ 100 µs, p99 ≤ 1 ms |
| G1F.2 | P0 dispatch latency under 5× load | p99 ≤ 200 µs (unaffected by load) |
| G1F.3 | Queue backlog | No sustained growth at 3× |

**1G — Stability**

| ID | Criterion | Target |
|---|---|---|
| G1G.1 | 72h live soak | 0 crashes, 0 panics |
| G1G.2 | Memory | Bounded; tier eviction verified |
| G1G.3 | CPU headroom at peak | ≥ 30% |

**1H — Radar early-catch quality (rule-based, ≥ 4 weeks replay)**

| ID | Criterion | Target (initial) |
|---|---|---|
| G1H.1 | Newborn runners (later ≥ 10× within 24h) wrongly demoted from HOT and not re-promoted before +50% from demotion price | ≤ 2% |
| G1H.2 | Awakening recall: tokens age ≥ 1h that later did ≥ 5× from awakening start, promoted to HOT before +30% from pre-awakening baseline | ≥ 90%; median lead time reported |
| G1H.3 | HOT promotion quality vs naive baseline (raw tx-count threshold), walk-forward | Higher PR-AUC in every fold |
| G1H.4 | Concurrent HOT set size P99 | Within measured compute budget |

---

## STAGE 2 — SAFETY, DEV & LAUNCH STRUCTURE

**Deliverables:** creator identification, live watchlist, P0 kill-switch detection (no sending yet), transfer → dynamic wallet add, LP-remove detection, authority/Token-2022 checks, sellability simulation, and bundle/sniper/insider detection.

| ID | Criterion | Target |
|---|---|---|
| G2.1 | Creator identification accuracy (labeled sample per venue) | ≥ 99.9% |
| G2.2 | DEV direct-sell detection recall on ≥ 200 historical direct DEV sells | 100% |
| G2.3 | False creator attribution on ≥ 10,000 non-DEV sells | 0 |
| G2.4 | Watchlist add latency after DEV transfer decoded | p99 ≤ 1 ms |
| G2.5 | Creator-linked LP-remove detection on replay set | 100% |
| G2.6 | All §33.1 replay cases | 100% pass |
| G2.7 | Token-2022 extension / authority detection on fixtures | 100% |
| G2.8 | Bundle detection vs ≥ 300 manually labeled launches | Precision ≥ 0.9, recall ≥ 0.9 (initial) |

---

## STAGE 3 — FAST FEATURES & TRADER QUALITY

**Deliverables:** full §11 engine (EWMA, cohort z, CUSUM, arrival, price features), §12 trader quality (real makers, wallet age, clusters, smart wallets), rule-mode MEE score (§22.3) and the background indexers.

| ID | Criterion | Target |
|---|---|---|
| G3.1 | All §33.4 unit tests | 100% pass |
| G3.2 | Rust vs Python reference features | Exact / ≤ 1e-9 rel |
| G3.3 | Real-maker filter on ≥ 1,000 labeled wallets | Precision of "not real" ≥ 0.95; recall reported |
| G3.4 | Smart-wallet & cohort leakage audit | 0 violations (all inputs ≤ t) |
| G3.5 | Smart-wallet decile test (OOS): future early-runner rate monotone in score | Significant monotone trend |
| G3.6 | Feature update cost per event (HOT token) | p99 ≤ 20 µs (initial) |

---

## STAGE 4 — DATASET & LABELS

| ID | Criterion | Target |
|---|---|---|
| G4.1 | Recorded mainnet data | ≥ 8 weeks (initial), spanning different market regimes |
| G4.2 | Decision snapshots at causal points (same code as live) | Generated via replay engine |
| G4.3 | Labels per §19.5 using executable price + simulated latency | Implemented + documented |
| G4.4 | Automated leakage audit | 0 violations |
| G4.5 | Splits | Chronological walk-forward with embargo, creator-aware |
| G4.6 | Rug-label sensitivity | Results reported under ≥ 3 alternative definitions |

---

## STAGE 5 — MODELS OFFLINE (OOS)

Baselines: B0 = Stage 1–3 rules only, B1 = logistic regression, B2 = transparent score.

| ID | Criterion | Target (initial) |
|---|---|---|
| G5.1 | Rocket P(≥10×) PR-AUC vs best baseline | Better in ≥ 4 of 5 walk-forward folds |
| G5.2 | Calibration | Reliability curves + ECE reported; calibrated if needed |
| G5.3 | Economic backtest (full costs + latency + landing-slot pool state) | Positive net expectancy overall and in majority of folds |
| G5.4 | Tail dependence | Expectancy reported excluding top 1% trades; strategy not dependent on a single trade |
| G5.5 | Max drawdown | Within risk policy |
| G5.6 | MEE exit vs baselines (fixed trailing, time exit, partial-TP for comparison only) | MEE better capture ratio OOS |
| G5.7 | Scale-in vs single entry | Better OOS risk-adjusted result, else scale-in disabled |
| G5.8 | Dynamic sizing variants (§33.3) | Chosen by OOS robustness, not best backtest |
| G5.9 | Ablation per feature group | Each kept group shows OOS value |
| G5.10 | Creator-reputation decile report | Rug and runner rates by decile reported |

---

## STAGE 6 — RUST INFERENCE PARITY

| ID | Criterion | Target |
|---|---|---|
| G6.1 | Rust vs Python predictions on ≥ 100k rows | ≤ 1e-6 abs diff |
| G6.2 | Inference latency per model | p99 ≤ 50 µs (initial) |
| G6.3 | Model hot reload under load | No dropped events, atomic swap |
| G6.4 | Model version hash in every decision log | 100% |

---

## STAGE 7 — EXECUTION

**Environment:** a local validator with mainnet programs and accounts cloned (e.g. `solana-test-validator --clone`, or a mainnet-fork tool), then **minimum-size** mainnet transactions.

| ID | Criterion | Target |
|---|---|---|
| G7.1 | Buy / sell / sell-all builders per venue (local) | 100% pass |
| G7.2 | Kill-switch end-to-end with simulated DEV sell (local) | 100% trigger; internal detect→send p99 ≤ 1 ms |
| G7.3 | Pre-armed context rebuilt after every tranche | 100% |
| G7.4 | Idempotency (duplicate events) | 0 double-buy / double-sell |
| G7.5 | Mainnet micro tx land rate | ≥ 95% (initial); landing-slot distribution reported |
| G7.6 | Execution-sliced 100% exit | Correct total, no hold decision between slices |

---

## STAGE 8 — SHADOW MODE (LIVE DATA, NO CAPITAL)

| ID | Criterion | Target |
|---|---|---|
| G8.1 | Duration | ≥ 3 weeks continuous |
| G8.2 | Simulated fills (measured latency, landing-slot state) vs backtest | Inside OOS confidence band |
| G8.3 | P0 misses on DEV sells in shadow positions | 0 |
| G8.4 | False emergency exit rate | Measured and reported |
| G8.5 | Uptime | ≥ 99.5% |

---

## STAGE 9 — MICRO-LIVE (TINY CAPITAL)

| ID | Criterion | Target |
|---|---|---|
| G9.1 | Sample | ≥ 200 live trades or ≥ 4 weeks (initial) |
| G9.2 | Live vs shadow slippage & latency | Difference measured; within tolerance |
| G9.3 | Realized ExitLeadLag on DEV events | Distribution reported |
| G9.4 | Risk limits | Never breached by a bug |
| G9.5 | Performance | Consistent with shadow |

---

## STAGE 10 — CONTROLLED SCALE-UP

- Raise caps stepwise (e.g. ×2 per step) only after each step passes a fixed number of trades.
- Automatic rollback on drawdown breach or feature/prediction drift.
- Champion/challenger model retraining on a fixed cadence. A challenger is promoted only after it passes G5 + G6 on fresh data.
- Monthly re-validation report.

---

# 32. RUST WORKSPACE LAYOUT

```text
crates/
  config/            env + config, redaction
  feed/              laserstream/, preprocessed/, preconf/, ws/, failover, gap_backfill
  events/            canonical bus, dedup, priority queues, reconciliation
  decoder/           venues/{pumpfun,pumpswap,raydium_amm,raydium_cpmm,raydium_clmm,
                     launchlab,meteora_dlmm,meteora_damm,meteora_dbc,orca,moonshot}
                     reference_balance_delta/
  universe/          tiers, lifecycle, migration linking
  state/             token_state, ring windows, ewma, cusum, arrival
  features/          fast features, cohort baselines (lock-free read)
  trader_quality/    real_makers, wallet_age, clusters, smart_wallets, crowding
  launch_structure/  bundles, snipers, insiders, holder concentration
  safety/            authorities, token2022, sellability
  pool_math/         cpmm, clmm, dlmm, bonding curves, depth
  creator/           reputation cache, link_confidence, watchlist
  dev_monitor/       kill_switch, emergency state machine, emergency_queue
  models/            catboost/onnx inference, versioning, hot reload
  strategy/          entry, scale_in, mee_exit, reentry
  sizing/            kelly, multipliers, caps
  execution/         builders per venue, prearmed sell, fees, sender, idempotency
  risk/              portfolio limits, circuit breakers
  latency/           timestamps, histograms
  recorder/          raw recording
  replay/            deterministic replay, latency simulation
  bench/             load replay
python/ (offline)
  reference/         reference decoder + window recomputation
  indexers/          creator history, wallet funding, smart wallets
  datasets/ labels/ training/ evaluation/ gate_reports/
gates/               STAGE_<N>_<NAME>.md
```

Background work (indexers, reputation, cohort stats) never blocks P0 or entry threads.

---

# 33. REQUIRED TEST SUITES

## 33.1 DEV / emergency replay cases

DEV direct SELL (tiny), DEV direct SELL (large), DEV transfer → linked wallet → SELL, DEV transfer with no sell, DEV transfer + SELL in the same tx, unknown whale SELL, router intermediate account, creator-linked LP removal, ordinary LP rebalance, failed early tx, non-canonical early signal, reconciliation correction, DEV event during a scale-in, DEV event during an execution-sliced exit.

## 33.2 Creator reputation

Repeated 5x winners, repeated 10x/20x winners, 500%+ / 1000%+ winners, mixed runners and rugs, no observed rugs with incomplete history, many wallets, newly discovered linked wallet, one-token history, new creator / insufficient sample.

## 33.3 Sizing & scale-in

The sizing scenarios to test:

- high reputation + strong market;
- high reputation + weak market;
- low reputation + strong market;
- low reputation + weak market;
- unknown creator + strong market;
- high P(rug);
- high P(10×) but poor depth;
- large creator concentration;
- a T2 add blocked by DEV_WARNING;
- a T3 add blocked by depth;
- an add attempt below the previous fill (must be rejected).

Sizing variants to compare:

- fixed size;
- fractional Kelly only;
- Kelly + creator;
- Kelly + market;
- Kelly + creator + market + rug;
- the previous variant + depth constraint.

## 33.4 Unit tests (formulas)

```text
Flow: buy=5000, sell=1500 -> SignedFlow=3500, FlowImbalance=0.5384615...
Buyer acceleration: prev=0.3/s, cur=1.2/s -> 0.9
Dev share: dev=600M, circulating=1B -> 60%
LP removal: before=100000, after=40000 -> 60%
EWMA: constant rate r for >> tau -> R converges to r (tolerance 1e-9)
EWMA: no events for t -> R decays by exp(-t/tau) exactly
CUSUM: x below mu0+k forever -> S stays 0; step change -> alarm after expected n
Cohort z: x = median -> 0; MAD = 0 -> validity flag, not Inf
CPMM: known reserves/fee -> exact Δx, P_exec, impact
Window expiry: event at t, query at t+w+1ns -> excluded
Zero denominators / NaN / Inf -> explicit flag, never silent 0
```

## 33.5 Exit (MEE)

- Single noisy tick → no exit.
- Persistent decay → exit 100%.
- Strong-reversal combination → immediate exit.
- Inactivity → exit.
- Scout not confirmed by T_scout → exit.
- DEV event while MEE is pending → the P0 path wins.
- Re-entry blocked on a blacklisted mint.
- Re-entry allowed after a cooldown on a healthy mint.

---

# 34. ABSOLUTE NON-NEGOTIABLES

**MUST NOT:**

```text
use DexScreener/GMGN API or any REST in the trading/emergency hot path
poll wallets or pools on a critical path
use 5m/1h windows as the primary entry trigger
take partial profit / fixed % take-profit
average down / add below previous fill
add size while DEV_WARNING/ARMED or emergency state is not NORMAL
wait for ML, CVD, candles or confirmations before an explicit DEV liquidation exit
require a DEV transfer before detecting direct DEV selling
assume a single transfer proves wallet ownership; assume every whale is DEV
treat 0 observed rugs as 0 true rugs when history is incomplete
use any future data in features (reputation, links, cohorts, smart wallets, visibility)
let sizing override an emergency exit
use total liquidity as executable depth; use x*y=k for CLMM/DLMM
claim guaranteed front-running or guaranteed sub-ms end-to-end latency
start a stage before the previous gate PASSES; trade live capital before Stage 9
log, commit or hardcode secrets
```

**MUST:**

```text
observe all supported DEX programs directly from chain
update fast features on every event
filter fake activity (real makers) before decisions
monitor the creator directly during every open position; add linked wallets immediately
pre-arm the sell context for the TOTAL position after every fill
exit 100% on DEV liquidation, rug emergency, momentum exhaustion, time/inactivity/hard stop
record full latency timestamps and every decision's feature vector
reconcile early signals against processed state
validate everything OOS; produce a gate report for every stage
```

---

# 35. FINAL STRATEGY

```text
ALL SOLANA DEX EVENTS (on-chain, earliest feed)
        |
      RUST
        |
UNIVERSE: NEWBORN / ESTABLISHED / AWAKENING     ── DEV WATCHLIST (P0)
        |                                               |
FAST REAL ACTIVITY (seconds) + TRADER QUALITY           |
+ LAUNCH STRUCTURE + SAFETY + CREATOR REPUTATION        |
        |                                               |
RUST MODELS: ROCKET / RUG / CONTINUATION                |
        |                                               |
SCOUT BUY -> CONFIRM ADD -> BREAKOUT ADD (into strength) |
        |                                               |
HOLD 100% WHILE MOMENTUM LIVES                          |
        |                                               |
MOMENTUM EXHAUSTED -> EXIT 100%        DEV/RUG -> KILL-SWITCH EXIT 100%
        |
RECORD / RECONCILE / RE-ENTER ONLY THROUGH FULL GATE
```

> The system becomes more aggressive only when real current activity, trader quality, creator history, risk estimates and execution capacity **all** support it. It becomes defensive immediately when creator liquidation or momentum exhaustion becomes observable.

No performance claim is valid until reproduced through realistic replay, walk-forward OOS testing, shadow mode and micro-live measurement. No design can observe a non-existent pre-mempool "intention to rug" on Solana, or guarantee that our transaction lands before the creator's.

---

# 36. REFERENCES

- Solana transaction flow (no public mempool): https://solana.com/developers/migrate-to-solana/complete-guide
- Helius Preconfirmations: https://www.helius.dev/blog/solana-preconfirmations
- Helius Preprocessed Transactions: https://www.helius.dev/preprocessed-transactions
- Helius LaserStream / Sender / Wallet API / Historical data: https://www.helius.dev/docs
- Yellowstone gRPC: https://github.com/rpcpool/yellowstone-grpc
- Jito low-latency transaction send: https://docs.jito.wtf/lowlatencytxnsend/
- DexScreener API (benchmark only): https://docs.dexscreener.com/api/reference
- GMGN New Pair (benchmark only): https://docs.gmgn.ai/index/new-pair

Provider latency figures (e.g. preprocessed being "up to ~8 ms ahead of processed") are provider architecture claims, not guarantees of end-to-end landing latency.
