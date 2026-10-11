# HASIL KOREKSI DAN FORENSIK SISTEM — sniper_v4 (+ branch v5-upgrade)

> **For Claude Code.** This file is the single work order produced by a forensic review (2026-10-11).
> Place it in the repo root next to `CLAUDE.md` and `V5_UPGRADE_PLAN.md`. The user writes Indonesian:
> **reply in Indonesian**, keep code, comments and reports in English.
> Every number below was measured from files in the repo (source given). Nothing here was compiled or
> run in Rust during the review (no `cargo` on the review machine); treat all fixes as **untested** until you
> run them.

---

## RINGKASAN UNTUK USER (Bahasa Indonesia)

1. **Radar sudah melihat hampir semua moonshot** (96% runner ≥11x HOT sebelum +30%), **tapi eksekusinya
   bocor**: hanya 30% yang dibeli, 14% ditutup untung, hasil jual median 0,82–1,22x.
2. **Penyebab utama:** (a) 61–65% "koin baru" sebenarnya koin lama yang salah label → langsung HOT tanpa cek
   arus uang → 108–130 koin HOT per menit → slot penuh sampah; (b) slot "siapa cepat dia dapat"; (c) filter
   `median_buy` dan `buy/sell` memblokir koin lama yang kemasukan uang besar; (d) exit menjual di dump pertama;
   (e) entry telat (median +5–11% dari 60 dtk sebelumnya).
3. **Hanya pump.fun + PumpSwap** yang dibaca. Raydium, Meteora, Orca, dll. tidak terbaca.
4. **ML belum bisa membedakan runner vs rug**: label V4 terpotong 83–95%, umur koin salah, belum ada fitur
   "siapa yang beli" (keluarga dev, smart wallet live), V5 belum punya model terlatih.
5. **Belum ada**: sistem honeypot, sistem rug lengkap (funding graph, riwayat dev), pembacaan transfer token,
   holder asli dari chain, modul eksekusi, feed gRPC mainnet (Helius Free menolak).
6. **V5 (branch `v5-upgrade`)**: kodenya ada (7.192 baris) tapi **belum dites dengan data V5** — 0 window V5
   selesai, model belum dilatih. Satu-satunya hasil: perbandingan tanpa ML di 2 window lama (satu untung, satu
   rugi). V5 juga punya bug baru (ranking slot tidak pernah jalan, entry hanya saat koin lahir, jual 50% di 2x).
7. **Keputusan user yang wajib diikuti**: radar membaca koin baru **dan** lama; inti radar = **arus uang + keramaian
   trader**; semua venue; data dari RPC/gRPC on-chain (bukan DexScreener); debu dihitung (digabung per cluster);
   arus dibandingkan likuiditas; **beli sebelum harga naik**; **exit jual 100% berbasis momentum, tanpa jual setengah**.
8. **Prediksi moonshot ada di file terpisah: `02_PREDIKSI_MOONSHOT_SNIPER.md`.** Isinya semua komponen dari
   data → radar → saringan rug/honeypot → ML → eksekusi → exit, rumus quant, cara membaca, dan status tiap komponen
   (✅ sudah ada di V4, 🟡 ada di V5 tapi belum dites, ❌ belum ada), digabung dengan 15 rumus dari
   `MEME_COIN_QUANT_FORMULAS.md` plus koreksinya. File ini (forensik) berisi bug, masalah, dan urutan kerja.
9. **Bab 14 = cakupan review**: bagian sistem yang belum saya periksa detail (feed, decoder, events, V5 follow/model/
   eval) dan file spec `SOLANA_DEX_SNIPER_V4_FINAL.md` yang tidak ada di repo. Claude Code wajib memeriksanya di Fase 0.

---

## 0. Rules for whoever executes this file

1. **Stage-gate conflict.** `CLAUDE.md` forbids ML, backtests, sizing and execution before Gate 0 and Stage 1
   pass. Gate 0 is BLOCKED (`gates/STAGE_0_FOUNDATIONS.md`). V5 ran as exploratory research with the user's
   approval on 2026-10-10. **Before starting any phase below, ask the user to confirm it may run as exploratory
   research.** Do not assume.
2. **No live trading, no wallet keys, no real capital.** Paper / backtest / shadow only until the user decides.
3. **Rust = hot path** (feed, decoder, features, inference, strategy, execution). **Python = offline only.**
   No LLM / AI-agent components anywhere.
4. **One code path for features**: training snapshots and the simulator must use the same Rust `Engine`.
   No future data in any feature (§27). Keep the existing causality test (prefix vs full stream identical).
5. **Measure, never assume.** Every claimed improvement needs a before/after number on data. If something
   cannot be measured (plan limits, missing data), STOP and tell the user exactly what is missing.
6. **Anti-overfitting protocol** (Section 9) applies to every phase. Do not loosen acceptance criteria without
   the user's written OK.
7. **One phase at a time.** At the end of each phase: `cargo test`, `scripts/ci.sh`, report measured numbers
   (in Indonesian to the user), ask before the next phase.
8. **Honest reporting.** Report failures and negative results as they are. If a fix makes things worse, say so.

---

## 1. Repository state at review time

| Item | Value |
|---|---|
| `main` | `e75a8c5` (V4, 2026-10-10) |
| `v5-upgrade` | `b70c27c` (V5, 2026-10-10 22:23 UTC), **not merged** into `main` |
| Datasets in repo | only `datasets/bf_1h` and `datasets/h1d` (V4 format, ~33–37 min each). The 28 V4 windows of the old VPS are gone; `reports/` refers to `h7d`, `h30d` which are not in the repo |
| Gates | only `gates/STAGE_0_FOUNDATIONS.md` (BLOCKED: no mainnet LaserStream on Helius Free) |
| Decoded venues | pump.fun curve + PumpSwap only (`crates/decoder/src/pump.rs`: `enum Venue { Curve, Amm }`), although `config/venues.toml` lists 11 venues |
| Execution module | none (no tx builder, signer, Jito, priority fee) |

---

## 2. Measured evidence (what is wrong, with numbers)

### 2.1 Moonshot funnel — V4 rule strategy (`reports/diagnose_exploratory.json`, 4 windows)

Runners ≥11x, all 4 windows summed (n = 148):

| Stage | Count | Note |
|---|---|---|
| HOT before +30% | 142 (96%) | radar sees them |
| Upside left after first HOT | median 2.5–6.3x per window; 42/148 still had ≥10x left | early enough |
| Jumped within latency (`instant_jump_runners`) | 129 (87%) | too fast for 2-slot latency |
| Entered during the run | **45 (30%)** | execution leak |
| Profitable | **20 (14%)** | |
| Median max multiple while held | 1.23–2.33x | exits too early |
| Median realized multiple | **0.82–1.22x** | moonshots sold like ordinary coins |

All tiers ≥2x: top block reason `max_positions` (147–226 blocks per window at tier 2x, ~70%).
Entry price vs 60 s before: median +5–11%, p90 2.16–2.46x (entries are late).
Losses by exit (`bf_1h`): 56 `HardStop` trades lost $113; net window −$50.

### 2.2 Radar newborn mislabel (measured on `datasets/*/trades.csv.zst`)

The radar treats any coin whose first trade is after `window_start + 60 s` as a newborn and promotes it to HOT on
its first real buy with **no flow check** (`crates/radar/src/lib.rs:180`, set by `with_newborn_after(ev.window.0 + 60)`
in `crates/app/src/backtest.rs:303` and `crates/app/src/ml_sim.rs:160`; **unchanged in V5**, `ml_sim.rs:252`).

| Window | "Newborn" per radar | Truly new (first trade on curve, progress < 5%) | Mislabeled old coins |
|---|---|---|---|
| `bf_1h` | 3,436 (108/min) | 1,216 (38/min) | 2,220 = **65%** |
| `h1d` | 4,682 (130/min) | 1,829 (51/min) | 2,853 = **61%** |

Caveat: a true new coin with a large dev buy at creation can exceed 5% progress → counted as old. Estimate, not exact.
Consequence: old coins skip the awakening rules, HOT floods, slots fill with noise, and the "92–94% of established
runners caught" radar metric is inflated.

### 2.3 Old coins are ~30% of 10x runners

With the corrected classification (2.2): runners ≥10x = 57 in `bf_1h` (40 new / **17 old**) and 43 in `h1d`
(31 new / **12 old**).

### 2.4 Entry filters: harmful for old coins, helpful for new coins

Test: every time a coin meets the base trigger (≥5 real buyers in 60 s, net inflow > 0, quote reserve ≥ 5 SOL,
at most once per 300 s per coin), enter ~3 trades later, measure max multiple until window end. Both windows pooled.

| Group | Filter result | n | ≥2x | ≥5x | ≥10x |
|---|---|---|---|---|---|
| **Old coins** | `median_buy ≤ 0.3` (pass) | 975 | 4.3% | 0.41% | 0.10% |
| | `median_buy > 0.3` (**blocked**) | 772 | **12.0%** | **1.42%** | **0.39%** |
| | `buy/sell ≤ 4` (pass) | 977 | 6.7% | 1.02% | 0.31% |
| | `buy/sell > 4` (blocked) | 770 | 9.1% | 0.65% | 0.13% |
| **New coins** | pass both | 208 | **26.9%** | **9.13%** | **3.85%** |
| | blocked by ≥1 | 348 | 15.8% | 3.74% | 1.72% |

Per window, new coins: pass ≥5x 10.5% vs 2.1% (`bf_1h`), 7.8% vs 4.8% (`h1d`) — consistent.
Per window, old coins: **inconsistent** (`bf_1h` pass 1.61% vs blocked 1.06%; `h1d` pass 0.00% vs blocked 0.96%).
Old-coin ≥5x sample is tiny (~15 coins). **Direction only, not proof.**
At the start of real ≥5x runs, rule filters block 70–78% of old-coin runners and 60–61% of new-coin runners.
Note: the radar's own fast rule also rejects `buy/sell > 4` and `median_buy > 1.0` (`crates/radar/src/lib.rs:211,215`,
`config/radar_params.json`), so the same issue exists before the strategy filters.

### 2.5 Label censoring (V4 snapshots, `datasets/*/ml_snapshots.csv.zst`)

V4 `labels()` (`ml_sim.rs:617`) uses 15–30 min forward horizons inside 33–37 min windows without checking coverage.

| Window | Snapshots | Peak label with < 30 min of data left |
|---|---|---|
| `bf_1h` | 56,129 | **95%** |
| `h1d` | 81,459 | **83%** |

V5 fixes the rule (10 h horizon + censoring), but needs follow-up data that does not exist yet.

### 2.6 Model / portfolio results (V4)

- ML portfolios (`reports/portfolio_*.json`): −50% to +18% per window, **`held_to_5x = 0` in every report**.
- Dump/rug models AUC 0.89–0.96 out of sample (this part works).
- `p_up2` AUC 0.80–0.88, PR-AUC 0.17–0.23 at base rate 5–6%; probabilities uncalibrated.

### 2.7 V5 comparison without ML (`reports/V5_COMPARE_v4windows.json` on branch)

| Window | V4 | V5 no ML | V5 trades | V5 PF | 10x runners held to ≥5x |
|---|---|---|---|---|---|
| `bf_1h` | $50.12 | $105.00 | 15 | 1.41 | 1 / 11 |
| `h1d` | $49.79 | $78.97 | 18 | 0.22 | 0 / 9 |

2 short windows, one up one down, 11 of 15 positions closed by `DataEnd`. **Proves nothing yet.**
Blocks: `exposure_cap` 1,381 and 2,013; `slots_full` 0 (see bug V5-1).

---

## 3. V5 status — done vs not done (from `reports/V5_STATUS.md` on branch + code review)

**Done (code + unit tests reported passing on the user's VPS, not re-run in this review):**
`sniper follow` (10 h follow-up), real creation time, V5 labels (10 h + censoring), 35 new features, wallet scores
(offline), `crates/model` (CatBoost JSON + isotonic, parity 10,000 rows), `strategy_v5` (EV entry, slot replacement,
TP1/TP2/trail, always-on exits), `v5_eval` (walk-forward, 24-point grid, sensitivity, Monte Carlo, acceptance).

**Not done:**
1. 0 V5 windows finished (window 1 was downloading). Need ≥20 train + 3 `final_holdout`.
2. `wallet_scores.py` and `sniper snapshots` never run on real V5 windows.
3. V5 models never trained (`v5_train.py` was still under review when committed).
4. Rust inference not wired into `strategy_v5` (score files only).
5. No V5 walk-forward, grid, sensitivity, Monte Carlo, holdout, acceptance results.
6. Feature-group ablation on strategy expectancy not run.
7. Helius credits per window not measured; follow-up cost not measured.
8. Stage-gate work (Gate 0 live, Stage 1 sub-gates) untouched.

---

## 4. Bug list (fix first — these invalidate results)

### V4 (`main`)
| ID | Bug | Location | Fix |
|---|---|---|---|
| V4-1 | Newborn = "first seen after window start + 60 s"; promoted with no checks | `radar/src/lib.rs:180`; `backtest.rs:303`; `ml_sim.rs:160` | Newborn only from on-chain create events (`CreateEvent` / pool create). Everything else = old coin → awakening rules |
| V4-2 | Labels right-censored | `ml_sim.rs:617` | Superseded by V5 labels; otherwise drop rows whose horizon exceeds data |
| V4-3 | `age_s`, `newborn`, `mult_since_first` from window start | `ml_sim.rs:465` etc. | From real creation (done in V5) |
| V4-4 | Liquidity check uses virtual reserves (curve starts at ~30 SOL virtual) → never blocks on the curve | `ml_sim.rs:368`; `backtest.rs:549` | Use real quote reserve (`real_quote`) on the curve |
| V4-5 | `price_vs_vwap60` is a plain mean of prices | `ml_sim.rs:458` | Volume-weighted (SOL-weighted) price |
| V4-6 | `STICKY_S` check always true (`last_event` set before the check) | `ml_sim.rs:325` | Gone in V5 (adaptive cadence) |
| V4-7 | Holder / top-10 features from observed trades only | `ml_sim.rs` features | Holders from chain (Section 6.3) |
| V4-8 | First-come slots (5) | `backtest.rs:525`; `ml_sim.rs:1042` | Rank by score (Section 7.1) |
| V4-9 | Exits kill winners (HardStop 30%, MEE, Scout, model exits) | `backtest.rs:500,506`; `ml_sim.rs` exits | Section 7.3 |
| V4-10 | **Two separate engines**: the rule backtest keeps its own per-coin state, pools and radar loop (`backtest.rs` `CoinState`); the ML path uses `ml_sim.rs::Engine`. Violates "one code path"; the two can drift | `backtest.rs`; `ml_sim.rs` | One shared engine for radar, features, rule and ML strategies |
| V4-11 | **Overtrading**: rule backtest makes 156 / 157 / 62 / 72 trades per 33–37 min window; network costs $18.4 / $18.6 / $7.7 / $7.9 of $100 (`reports/backtest_exploratory.json`, `reports/diagnose_exploratory.json`) | `backtest.rs` | Fewer, higher-quality entries (Sections 5, 7.1); report cost per trade |
| V4-12 | Quarter-Kelly sizing on **uncalibrated** `p_up2` in the V4 tune grid | `ml_sim.rs:1139` | Fixed size until calibration passes (Section 7.2) |

### V5 (`v5-upgrade`, `b70c27c`)
| ID | Bug | Location | Fix |
|---|---|---|---|
| V5-1 | **Exposure cap checked before slot ranking.** $100 ≈ 0.91 SOL × 0.6 ≈ 0.55 SOL ≈ 10 bets of 0.05 SOL < `max_positions` 18 → replacement never runs → first-come again | `strategy_v5.rs:544-551` | Collect candidates per decision tick (2–5 s), rank by EV, then apply exposure/slots; replacement must be reachable when exposure is the binding limit |
| V5-2 | **Entry decision only on radar `Promote`.** Newborns are promoted at their first real buy → model decides at birth with almost no information; later momentum is never a decision row unless demoted and re-promoted | `ml_sim.rs:510,514`; `strategy_v5.rs:487` | Repeated decision rows after HOT (rate-limited), plus WATCH/ARM states (Section 5.4) |
| V5-3 | Radar unchanged (V4-1 persists) → 1,864–2,684 signals per 33 min, ~1% runners | `ml_sim.rs:252` | V4-1 fix |
| V5-4 | No trained model → `NoScores` = uniform bins → equal EV for all signals → no selection | `strategy_v5.rs:146` | Train models (Section 8) |
| V5-5 | TP1 sells 50% at 2x, TP2 25% at 5x — **conflicts with the user's decision (no partial sells)** | `strategy_v5.rs:56` | Section 7.3 |
| V5-6 | Trailing on price (55% from peak), not crowd momentum | `strategy_v5.rs:466` | Section 7.3 |
| V5-7 | EV units mismatch: candidate EV = Σ P(bin)·R_k (net return); position EV = two-outcome bet (+100%/−50%) from `p_hold` | `strategy_v5.rs:305,307` | Same unit for both (expected net return of holding vs entering) |
| V5-8 | DevSell kill only when ONE sell ≥ 50% of the creator's current balance → repeated 49% sells or sells via other wallets pass | `strategy_v5.rs:441` | Cumulative sells of creator + linked wallets vs initial holdings (Section 6.2) |
| V5-9 | `y_bin` uses a −65% barrier: a coin that dumps −65% then runs counts as rug | `ml_sim.rs` `labels()` | Report how many ≥10x runners hit −65% first; keep or widen based on data |
| V5-10 | Entry model trains on all `is_signal` rows including follow-up period rows where regime features are −1 | `v5_train.py:136-138` | Train entry model on `is_signal == 1 & in_window == 1` (or make regime features valid after window end) |
| V5-11 | Wallet scores from scattered 1 h windows (2–30 days apart) → very thin history | `wallet_scores.py` | Continuous data (Section 6.4) |
| V5-12 | Bot flag has no minimum trip count (~75% of flagged bots have ≤ 2 closed trips) | `wallet_scores.py` | Minimum trips (e.g. ≥ 5) before flagging |
| V5-13 | Liquidity check still virtual (`untradable(e.coin, 0.5)`) | `ml_sim.rs:556`; `strategy_v5.rs:510` | V4-4 fix |
| V5-14 | VWAP bug persists | `ml_sim.rs:656` | V4-5 fix |
| V5-15 | Regime features (`attention_share_5m`, `new_coins_per_min`) count pump.fun only | V5 features | Cross-venue after Section 6.5 |
| V5-16 | SOL/USD from Binance via `curl` (failure → NaN) | `ml_sim.rs::sol_usd_series` | Cache + fallback; live: on-chain oracle or pool price |
| V5-17 | V5 snapshot format breaks V4 Python scripts | `V5_STATUS.md` item 15 | Keep V4 scripts working or remove them explicitly |

---

## 5. Radar redesign (user's core requirement)

**User decision:** the radar must read **new and old coins**, on **all venues**, and its first criterion is
**money flow and how crowded the traders are**, because that is what moves the price.

### 5.1 Two paths
- **New coin** (from create event): strict checks before HOT (no auto-HOT). Keep `median_buy` and `buy/sell`
  filters here (they help, Section 2.4) until data says otherwise.
- **Old coin** (everything else, any age, any venue): awakening path based on flow and crowd growth.
  **Remove `median_buy` blocking for old coins** (blocked group ran ~3x more often, Section 2.4). Re-test
  `buy/sell` on more windows before deciding (mixed result).

### 5.2 Core radar metrics (per coin, aggregated across all its pools)
| Metric | Definition |
|---|---|
| Net flow / liquidity | (buy SOL − sell SOL) over 15 s / 60 s / 300 s ÷ real quote reserve. Main "force" metric |
| Real trader growth | unique **clusters** (not wallets) buying in the last 60 s vs the previous 60 s |
| Buy arrival acceleration | fast/slow EWMA of real buys (exists) |
| Absorption | net flow / liquidity rising while price change is small (accumulation before the move) |
| Size distribution | p50/p90 buy size, whale buys, retail share (relative to liquidity) |
| Sell pressure | sell SOL, sellers, early-holder selling, profit overhang |
| Market cap | context only (room to grow, volume/mcap turnover). Price impact is driven by liquidity, not mcap |

Never use raw SOL thresholds as the main gate: 1 SOL in a 30 SOL pool ≠ 1 SOL in a 500 SOL pool.

### 5.3 Dust and split buying (user requirement: do not drop dust)
Currently the radar drops trades < 0.01 SOL entirely (`radar/src/lib.rs:158`); dust only exists as an ML feature.
- Keep every trade. Tag dust vs real.
- Aggregate buys **per wallet cluster** (funding graph + co-buying): one cluster = one trader whose size is the
  **sum** of all its buys, including dust. This catches split buying.
- Dust features: dust SOL, dust count, dust share, share of dust from one cluster.
- Interpretation: much dust + one cluster + no growth in real clusters = **fake volume (negative)**;
  dust summing to large SOL from one cluster = **hidden accumulation (watch: smart money or dev)**.

### 5.4 Buy before the rise (user requirement)
Do not buy after +10–30%. States:
- **WATCH**: leading signals while price is flat — absorption, smart-wallet entries (live wallet DB), split buying,
  real-cluster growth, sellers drying up, range compression, wallets freshly funded (e.g. from exchanges) buying the
  same coin, catalysts (curve near migration, new X/Telegram links, DexScreener paid profile/boost, community takeover).
- **ARM**: signals strengthen AND safety passes (Sections 6.1–6.2). Pre-build the transaction.
- **ENTRY**: first confirmed push, **only if price ≤ base × (1 + max_entry_ret)** (e.g. +10% over the 5-min low).
- **Time stop**: no move within N minutes → exit (loss is small because entry was near the base).
Trade-off: more false starts → small fixed size, many positions, strict rug/honeypot filters.
New-born coins are a pure speed race against other snipers; the main edge of this approach is **old coins being
accumulated**.

---

## 6. Missing components (important)

### 6.1 Honeypot system (absent in V4 and V5)
Standard pump.fun tokens usually have no mint/freeze authority, but other venues and **Token-2022** tokens can be
honeypots. Check per token, never assume:
- mint authority must be `null`; freeze authority must be `null`;
- Token-2022 extensions: reject `TransferHook`, `PermanentDelegate`, `DefaultAccountState = Frozen`,
  `NonTransferable`, `Pausable`; `TransferFee` → include in costs, reject above a limit;
- **sell simulation** (`simulateTransaction` of a small sell) must succeed and match pool math;
- on-chain proof: ≥ N distinct non-dev, non-dev-cluster wallets have sold successfully;
- effective tax: realized sell output vs pool math on every sell;
- pool owner program must be an official program from `config/venues.toml`.

### 6.2 Rug-pull system (V5 has only RugModel, single-sell DevSell, LpDrop, hazard, bundle/top10 features)
- **Funding graph**: wallets funded by the dev 1–2 hops (SOL transfers) → "dev family" supply share.
- **Dev history**: coins created before, share dead within 1 h, SOL extracted → blacklist serial ruggers.
- **Dev funding source**: fresh wallet funded by a known rugger vs exchange.
- **Bundle at create**: buys in the create slot from dev-family wallets.
- **Cumulative kill switch**: total sells by dev + dev family ≥ X% of initial holdings (not per transaction).
- Large holders exiting in sequence (top-10 sells within 60 s); LP drop for every AMM venue.
- Propagate blacklists of dev wallets and clusters to all future coins.

### 6.3 Token transfers and real holders (critical)
The system reads only swaps. A dev can move supply to 50 wallets by plain transfers and dump later; this is invisible
today. Read SPL / Token-2022 transfers for tracked mints, and holder balances from token accounts on chain, not from
observed trades.

### 6.4 Data
- **Helius Free** is the bottleneck: mainnet gRPC rejected; WS gives logs only; backfill ~1 h wall time per 1 h
  window (+ follow-up); credits per window unknown. A paid plan (LaserStream) is required for live and for
  complete, fast data.
- **Continuous** history (≥ 4 weeks, Gate 1H), not scattered 1 h windows → wallet scores, dev history, regime.
- **Live wallet database** (smart money, sniper, bot, rugger, cluster) updated continuously.
- Read on-chain via **gRPC/RPC only**. DexScreener is too slow and aggregated (candles, rate limits, indexed pairs
  only); use it only for metadata (links, paid profile/boost), never for the radar.
- Volume estimate: mainnet WS for 11 venues was ~2.8 MB/s raw (~21 GB/day). Decoder and storage must be efficient.

### 6.5 All venues (user requirement: not only pump.fun)
Decoders + pool math + cross-pool aggregation per coin + non-SOL quotes (e.g. USDC) converted to one unit:
| Venue | Priority |
|---|---|
| Raydium LaunchLab (letsbonk), Meteora DBC | P1 (launchpads) |
| Raydium CPMM, Meteora DAMM v2 | P1 (their migration targets) |
| Raydium AMM v4, CLMM, Meteora DLMM, Orca Whirlpool | P2 (CLMM/DLMM concentrated-liquidity math) |
| Moonshot / Moonit | P3 (re-verify program ID; SDK repo archived) |
Every program ID must be verified against official sources and recorded in config (CLAUDE.md rule).

### 6.6 Other missing pieces
- **Who is selling and at what profit**: cost basis per holder → profit overhang (partial in V5).
- **Exit liquidity check** before buying (can the position be sold without large slippage?).
- **Migration event** as its own catalyst (curve complete → AMM), not just a blocker.
- **Competition / MEV**: sandwich and front-run risk, slippage limits, dynamic priority fee, Jito bundles.
  Backtest uses fixed 15% buy failure and fixed tx cost; calibrate from shadow data.
- **Account-level risk**: daily loss limit, kill switch after consecutive losses, hourly exposure cap.
- **Off-chain attention (deterministic, no AI)**: X/Telegram/website present, copycat name/image, paid profile.
- **Market regime**: SOL trend, total memecoin volume, new coins per hour (cross-venue).
- **Monitoring + shadow mode**: log every live decision vs outcome; alerts on feed gaps and latency.
- **SDK reconnect**: fixed 5 s blind time per disconnect; implement immediate retry and measure.
- **Alpenglow cleared banks** (`EntryUpdateParent`) arrive only with an entry subscription, which is not subscribed
  (`gates/STAGE_0_FOUNDATIONS.md`, known issue 3). Needed before live: reconcile cleared/skipped banks.
- **Time resolution**: event time is the block's `blockTime` in whole seconds (`backtest.rs:71`); intra-second order
  comes from slot / tx index. Fine for 15–300 s windows, too coarse for sub-second latency work. Live: use receive
  timestamps (ns, `crates/latency`) and slot times.

---

## 7. Strategy changes

### 7.1 Slots and selection
- Rank candidates by expected value at each decision tick; apply exposure and slot limits **after** ranking.
- Replace the weakest open position when a candidate's EV beats it by `replace_margin`, in the **same unit** (V5-7).
- Cap entries per minute, but never as first-come within the minute.

### 7.2 Entry
- Price guard: no entry above `base × (1 + max_entry_ret)` (Section 5.4).
- Safety gates (6.1, 6.2) must pass.
- Size fixed (`bet_sol`) until calibration passes (reliability error < 0.05 per bin), as in V5 plan §7.6.

### 7.3 Exit — **user decision: sell 100%, no partial sells; exit on crowd momentum**
"Momentum Fade" (proposal, **untested**). Measured since entry, real trades:
| Signal | Definition |
|---|---|
| Quiet | buy arrival EWMA (τ 10 s) < `fade_rate_frac` × its peak since entry |
| Smaller sizes | mean real buy size over 30 s < `fade_size_frac` × its peak since entry |
| Sellers dominate | sell SOL over 30 s > buy SOL over 30 s |
| Dead | no real buy for `idle_s` |

Exit 100% when **Sellers dominate AND (Quiet OR Smaller sizes)** holds for ≥ `fade_persist_s`, or **Dead**.
Always on: cumulative dev/dev-family kill switch, rug model, LP drop, hazard, `max_hold_s`, optional wide disaster
stop for rugs. Disable V4 model exits (`DumpPredicted`, `StrongReversal`, `MomentumExhausted`, `Scout`) when this
mode is on. The persistence requirement exists because moonshots often dump first and then run.
V5's TP1/TP2 ladder may be kept **only as a comparison baseline** with the user's OK; the default is full exit.
Tune `fade_*`, `idle_s` on dev windows only; compare against the ladder on the same test windows.

---

## 8. Labels and ML

1. **Pre-pump label** (new): at snapshot t with price flat (|ret_60s| ≤ ε, price ≤ base × (1+δ)), y = 1 if price
   rises ≥ X within N minutes before falling ≤ −Y. Train the entry model **only on flat-state rows**, so it learns
   states before the move.
2. Keep V5 labels (`y_bin`, `y_tp1`, `y_hold`, `y_dead_10m`, `y_logmax`) with V5 censoring; resolve V5-9.
3. Decision rows: repeated after HOT/WATCH (V5-2), meta-labeling (radar = primary signal, model = take/skip).
4. Entry rows: `in_window == 1` (V5-10). Sample weight `1 / rows_per_coin` (V5 plan §3.4).
5. Features to add (Sections 5.2, 5.3, 6.2, 6.3): net flow / liquidity, real-cluster growth, absorption, dust
   cluster share, dev-family supply share, dev history, holders from chain, transfer-based supply moves,
   coin type (new/old), venue.
6. Linear algebra **as tools, not as the main model**: spectral clustering / SVD on the wallet × coin matrix
   (sybil and bundler clusters; V5 uses connected components only), PageRank-style risk propagation on the funding
   graph, OLS Kyle λ (exists), PCA for market regime, logistic regression baseline (exists). The main model stays
   tree-based (CatBoost); the market is non-linear, fat-tailed and regime-shifting.
7. Validation: walk-forward by time with purge ≥ H (V5 has it), calibration (isotonic), reliability tables,
   precision of the top 1% for ≥10x, Brier score.
8. Honest limit: part of a coin's fate depends on attention that arrives after entry; some of it is not predictable
   from on-chain data at entry time. Do not promise high accuracy.

---

## 9. Anti-overfitting and honesty protocol (mandatory)

1. Split by **time**: train windows < calibration windows < test windows; purge ≥ H between them.
2. `final_holdout` windows from `config/sample_windows.json` are **never** used for tuning; run them **once**, after
   everything is frozen.
3. Grids ≤ 24 combinations; select by the **median** dev-window result (not best, not worst on 2 windows).
   Fix the selection rule before looking at results.
4. A change is KEPT only if it improves out-of-sample **strategy expectancy** (after costs), not only AUC,
   and on the majority of test windows.
5. Minimum sample before any claim: report n for every rate; flag results with < 30 events as "direction only".
6. Report expectancy **with and without the top 1% trades**; one lucky moonshot must not carry the result.
7. Sensitivity: latency 1 / 2 / 4 slots, buy failure 0.15 / 0.30, tip ×2 — report all.
8. Never use window-end information in features; keep the causality test green.
9. If acceptance fails: report which criterion failed and stop. Do not loosen criteria without the user's written OK.

---

## 10. Work order (phases)

| Phase | Work | Done when (measured) |
|---|---|---|
| **0** | Ask the user to confirm exploratory mode (Rule 0.1) and for the missing spec `SOLANA_DEX_SNIPER_V4_FINAL.md`. Choose base branch: verify `v5-upgrade` builds and `cargo test` passes; re-run `sniper v5-compare` on `datasets/bf_1h`, `datasets/h1d` and reproduce Section 2.7 numbers. Review the parts listed in Section 14 that were not reviewed and add any new bugs to Section 4 | tests green; numbers reproduced or differences explained; review findings reported |
| **1** | Bug fixes: V5-1, V5-7, V5-8, V5-10, V5-12, V5-13/V4-4, V5-14/V4-5, V5-16, V5-17 | unit tests for each; re-run v5-compare; report before/after |
| **2** | Radar: true newborn from create events (V4-1/V5-3), two paths (5.1), core metrics (5.2; formulas in the prediction file, Bagian 5–9), CUSUM change-point (prediction file Bagian 7), dust + cluster aggregation (5.3; prediction file Bagian 11), repeated decision rows (V5-2), WATCH/ARM/ENTRY + price guard (5.4; prediction file Bagian 15, P5) | radar eval on all available windows: HOT count per minute, recall of ≥5x/≥10x runners **before** +10% and +30%, precision vs naive radar; old-coin and new-coin reported separately |
| **3** | Safety: honeypot (6.1), rug system (6.2), token transfers + holders from chain (6.3) | unit tests on known cases; report how many runners and rugs each gate blocks |
| **4** | Data: measure Helius credits/disk per window; with user OK fetch ≥20 train + 3 holdout windows (or continuous data on a paid plan); live wallet DB | cost report to the user before fetching more |
| **5** | Labels + models: pre-pump label (8.1), executable-price moonshot label (prediction file Bagian 13), V5 models, calibration, ablation of new groups | walk-forward metrics + reliability tables; groups kept only if strategy expectancy improves |
| **6** | Strategy: ranking (7.1), entry guard (7.2), Momentum Fade full exit (7.3) vs variants A–D (prediction file Bagian 16) | walk-forward on test windows, sensitivity runs |
| **7** | Acceptance (Section 11) on `final_holdout`, once | report `reports/V5_RESULTS.md` (or V6) |
| **8** | Only after acceptance: venues (6.5), paid gRPC + latency measurement, execution crate (tx builder, Jito, dynamic priority fee, simulate-before-send), shadow mode, monitoring, account risk (6.6) | shadow results vs backtest |

---

## 11. Acceptance criteria (test windows, latency 2 slots; from V5 plan §9)

1. Expectancy without the top 1% trades > 0.
2. Profit factor > 1.3.
3. ≥ 60% of test windows positive.
4. Moonshot capture ≥ 20% of ≥10x runners (held a position that reached ≥5x).
5. Still positive at latency 4 slots and buy failure 0.30.

Additional reporting (not a pass/fail): entry price vs base (median), share of entries before +10%,
old-coin vs new-coin results, blocks by reason.

---

## 12. Open decisions for the user

1. Exploratory mode before Gate 0 / Stage 1 (Rule 0.1).
2. Helius paid plan (required for live, all venues and complete data).
3. Keep V5's TP1/TP2 ladder as a comparison baseline, or remove it entirely (default here: full exit, Section 7.3).
4. How many windows / how much continuous history to fetch after the cost report.
5. Merge `v5-upgrade` into `main` after Phase 0–1, or keep working on the branch.
6. Provide `SOLANA_DEX_SNIPER_V4_FINAL.md` (missing from the repo; `CLAUDE.md` names it the single source of truth).

---

## 13. Moonshot prediction (separate file)

The full prediction specification — components from data → radar → safety → ML → execution → exit, quant formulas,
"how to read" notes, and status per component (✅V4 / 🟡V5 / ❌) — is in the separate file
**`02_PREDIKSI_MOONSHOT_SNIPER.md`** ("prediction file"). It merges the review's blueprint with the user's
`MEME_COIN_QUANT_FORMULAS.md` (15 formulas, U1–U15), including corrections to those formulas.
Use it together with this file: bugs and work order here, prediction spec there.

---

## 14. Review coverage (what this forensic review did and did not check)

**Read in detail:** `crates/radar`, `crates/app/src/ml_sim.rs` (V4), `crates/app/src/backtest.rs` (V4),
`python/ml/*.py` (V4), `config/*.json`, `gates/STAGE_0_FOUNDATIONS.md`, `CLAUDE.md`, `reports/*` (summaries);
V5: `strategy_v5.rs`, V5 `ml_sim.rs` (features list, labels, censoring, context), `v5_train.py` (structure),
`reports/V5_STATUS.md`, `reports/V5_COMPARE_v4windows.json`.

**Not reviewed in depth — review in Phase 0 before relying on them:**
- `crates/feed` (`grpc.rs`, `ws.rs`, `rpc.rs`, `backfill.rs`), `crates/events` (`pipeline.rs`, `gap.rs`,
  `normalize.rs`), `crates/recorder`, `crates/replay`, `crates/config`, `crates/latency`.
- `crates/decoder` (`pump.rs`, `idl.rs`, `balance_delta.rs`, `txview.rs`); accuracy is reported (side 100%,
  amounts 99.94%) but was not re-verified.
- `crates/app/src/{main.rs, census.rs, radar_eval.rs, pump_check.rs, diagnose.rs}`; `python/research/*`.
- V5: `crates/feed/src/follow.rs`, `crates/app/src/follow.rs`, `crates/model`, `crates/app/src/v5_eval.rs`,
  `python/ml/v5_common.py`, `python/ml/model_parity.py`, `python/research/wallet_scores.py` internals, all tests,
  `scripts/v5_*.sh`.
- **Spec file `SOLANA_DEX_SNIPER_V4_FINAL.md` is missing from the repo** (both branches), although `CLAUDE.md`
  calls it the single source of truth. Compliance with the spec could not be checked. Ask the user for it.
- No Rust code was compiled or run during this review.

---

## Appendix A — how the Section 2.2–2.4 numbers were measured (reproduce)

Data: `datasets/{bf_1h,h1d}/trades.csv.zst` (columns `slot, block_time, mint, venue, side, sol_lamports, token_raw,
trader_id, creator_id, quote_reserve_after, token_reserve_after, real_quote, real_token, fee_bps, curve_complete`).
Python with `pandas`, `numpy`, `zstandard`.

- **Price path** per mint: `quote_reserve_after / token_reserve_after`; curve events after the first PumpSwap event
  are dropped.
- **Truly new coin**: first trade after `t0 + 60 s`, on `pumpfun_curve`, and `1 − real_token / 793.1e12 < 0.05`.
- **Runner multiple** (2.3): max over t of (future max price / price at t) inside the window.
- **Filter test** (2.4): stream real trades (≥ 0.01 SOL) per coin; 60 s window; trigger when ≥ 5 unique real buyers,
  buy SOL > sell SOL, quote reserve ≥ 5 SOL, at most once per 300 s; entry price = price 3 trades later;
  outcome = max future price until window end / entry price; median buy and buy/sell SOL ratio over the same 60 s.

Limits: 2 windows of 33–37 min; moves after the window end are not seen (multiples are underestimated);
old-coin ≥5x samples are small.

## Appendix B — references
Methods listed in `V5_UPGRADE_PLAN.md` §10 (arXiv 2602.14860, 2602.13480, 2608.20271, 2504.07132, 2603.24625,
2607.02823, 2512.11850; López de Prado 2018; arXiv 1701.03960).
