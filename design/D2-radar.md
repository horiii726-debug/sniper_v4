# D2-radar — design (V6, partial; NOT implemented, NOT compiled)

> Produced 2026-10-11 by a read-only design agent. Nothing here has been coded yet.

## Design

# WP D2 — Radar redesign (V4-1 / V5-3 / V5-2), implementation-ready design

Read-only design step. Nothing was compiled or run. Every line reference is to branch `v6-components` (= `v5-upgrade` b70c27c + 3 spec files).
Rust = hot path (radar, CUSUM, features, strategy hook). Python = offline only (one pytest, an optional CUSUM reference).
No signing, no keys, no execution crate, no new venue decoders. The only decoder change is a flag on an existing pump decoder output.

---------------------------------------------------------------------------------------------------------------------

## 0. What D2 delivers (one paragraph each)

(a) **Newborn only from on-chain creation.** Creations come from `CreateEvent` / `CreatePoolEvent` (`decoder::pump::decode_creations`). A window-wide scan writes `creations.bin.zst`, `Events.created` holds the times, and `Events.creations_scanned = true` marks the window.
When there is no creation data (the two V4 datasets), an explicit **PROXY** mode applies. Proxy = first trade after a 60 s warm-up, on the pump.fun curve, with progress < 5%. Every report, stat and feature labels it `proxy`.
The mode is chosen by `Params.newborn_mode`. Default `auto`: authoritative when `creations_scanned`, otherwise proxy. Newborns are **never auto-HOT**; they enter WATCH and must pass the strict rule.

(b) **Two paths.**
- New coin (birth = New and age ≤ `new_max_age_s`): strict V4 fast rule with the `median_buy` and `buy/sell` filters kept.
- Old coin (everything else): V4 awakening rules without the median block, plus a new **flow/liquidity + cluster-growth + CUSUM** rule. `old_max_buy_sell_ratio` stays configurable so it can be re-tested.

(c) **Dust is kept and tagged.** A trade is dust when `sol < real_min_sol`. Buys are aggregated per **wallet cluster**. `Trade.cluster` comes from an optional lookup `Context.clusters` (wallet id → cluster id, built from `wallet_scores.csv: cluster_id`). Without a lookup, the cluster is the wallet itself.

(d) **CUSUM.** New self-contained module `crates/radar/src/cusum.rs`:
- robust causal baseline: median/MAD, or EWMA;
- warm-up N, then a population baseline per age bucket for young coins;
- k = 0.5, h = 5, reset after an alarm;
- fixed 5 s time steps.

(e) **States Cold → WATCH → HOT (Promote).** In addition, rate-limited **Decide** rows are emitted while WATCH or HOT, so the model decides more than once (V5-2). A snapshot row with `is_signal = 1` now means "decision row" (V5 plan §3.3).

(f) **`radar::Trade` gains:** `cluster`, `mprice`, `liq_sol` (real quote reserve), `amm`, `by_creator`. Every caller goes through one new builder, `crates/app/src/radar_feed.rs` (`TradeBuilder`, `BirthTable`, `RadarFeed`). This also removes the 4 hand-written copies of radar input construction, which is the radar part of V4-10.

(g) **`Params` gains `#[serde(default)]` plus the new fields.** `config/radar_params.json` loads unchanged. A new `config/radar_params_v4_legacy.json` reproduces the V4 radar bit-for-bit, which a unit test checks against an embedded copy of the V4 code.

---------------------------------------------------------------------------------------------------------------------

## 1. Verified current state (call sites that construct `radar::Trade` or drive `Radar`)

| # | File:line | What it does today |
|---|---|---|
| 1 | `crates/radar/src/lib.rs:158` | `if t.sol < p.real_min_sol { return None; }` — dust dropped (file 01 §5.3) |
| 2 | `crates/radar/src/lib.rs:180-186` | newborn (first seen after `newborn_after`) → Promote on first real buy, no checks (V4-1) |
| 3 | `crates/radar/src/lib.rs:211,215` | `buy/sell > 4` and `median_buy > max_median_buy_sol` block **all** coins incl. old (file 01 §2.4) |
| 4 | `crates/app/src/ml_sim.rs:252` | `Radar::new(rp).with_newborn_after(ev.window.0 + 60)` (V5-3) |
| 5 | `crates/app/src/ml_sim.rs:309-313` | sweep each new second, signals discarded |
| 6 | `crates/app/src/ml_sim.rs:509-514` | builds `Trade{time,order,coin,trader,buy,sol,price}`; `signal = Promote`; snapshot forced only on Promote (V5-2) |
| 7 | `crates/app/src/ml_sim.rs:366` | regime `new_coins` uses the V4-1 proxy |
| 8 | `crates/app/src/ml_sim.rs:663-669, 987` | `newborn`, `age_s`, `created_known` from follow `created` or the V4-1 proxy |
| 9 | `crates/app/src/backtest.rs:358, 368-369, 460-467, 533-537` | V4 rule backtest: own radar + sweep + Trade construction |
| 10 | `crates/app/src/follow.rs:167-187` | `hot_coins`: own radar + Trade construction (selects coins to follow) |
| 11 | `crates/app/src/follow.rs:124` | `merge` overwrites `ev.created[ci] = c.created` |
| 12 | `crates/app/src/follow.rs:343` | `decode_creations(...).find(|c| c.mint == mint)` |
| 13 | `crates/app/src/radar_eval.rs:53-61, 180, 206` | raw-recording trades (balance_delta), `with_newborn_after(ds.window.0 + 60)`, `is_newborn` |
| 14 | `crates/app/src/strategy_v5.rs:487` | entry only `if step.signal` (Promote) (V5-2) |
| 15 | `crates/app/src/v5_eval.rs:394` | `Engine::new(&l.ev, rp)` without ctx (first-signal table) |
| 16 | `crates/app/src/main.rs:384,389,398,405,441,456,483,530,555,608`; `diagnose.rs:21` | 11 copies of `serde_json::from_value(...["params"])` for `radar::Params` |
| 17 | `crates/app/src/census.rs:204` | `newborn_in_window = first_time > window_start + 60` (report column only, no Radar) |
| 18 | `crates/radar/src/lib.rs:324-325, 351` | unit-test Trade literals |

Not radar callers: `pump_check.rs` and `census.rs` (except the documented column). `v5_eval.rs` only passes `RadarParams` through.

---------------------------------------------------------------------------------------------------------------------

## 2. `crates/radar` — new public API (`src/lib.rs`)

### 2.1 Module layout
```
crates/radar/src/lib.rs     Trade, Birth, CoinKind, RadarState, Signal, NewbornMode, BirthSource, Params, Metrics,
                            CoinView, Radar, NaiveRadar (unchanged), rule functions, tests
crates/radar/src/cusum.rs   NEW: CusumParams, BaselineKind, Channel, RobustBaseline, Cusum, PopBaseline, CoinCusum,
                            age_bucket, median_mad, tests
```
`lib.rs`: `pub mod cusum; pub use cusum::{CusumParams, BaselineKind, Channel};`

`crates/radar/Cargo.toml`: add
```toml
[dev-dependencies]
serde_json.workspace = true
```
This is needed for the params-loadability tests.

### 2.2 `Trade` (changed)
```rust
#[derive(Debug, Clone, Copy)]
pub struct Trade {
    pub time: i64,
    pub order: u64,
    pub coin: u32,
    pub trader: u32,
    /// Wallet cluster of the trader (funding / co-buying, offline lookup). == trader when unknown.
    pub cluster: u32,
    pub buy: bool,
    pub sol: f64,
    /// Execution price of this trade (sol / tokens), as before (lift rule, radar_eval labels).
    pub price: f64,
    /// Marginal pool price after the trade (quote/token reserves of the coin's active pool). NaN if unknown.
    pub mprice: f64,
    /// Real quote liquidity after the trade, SOL: curve = real_sol_reserves; PumpSwap = pool quote reserve. NaN if unknown.
    pub liq_sol: f64,
    /// Trade on an AMM pool (PumpSwap) rather than the bonding curve.
    pub amm: bool,
    /// Trader is the coin's creator (or in the creator's cluster when a cluster lookup is loaded).
    pub by_creator: bool,
}
impl Trade {
    /// Minimal trade (tests, raw-recording radar_eval): cluster = trader, mprice = price, liq unknown, curve, not creator.
    pub fn basic(time: i64, order: u64, coin: u32, trader: u32, buy: bool, sol: f64, price: f64) -> Trade
}
```
There is **no dust field.** Dust is derived inside the radar from `Params.real_min_sol`, so there is one definition. Callers never set it.

### 2.3 Enums
```rust
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum Birth {
    /// Created at `t`. proxy = false: on-chain CreateEvent / CreatePoolEvent (not a migration);
    /// proxy = true: inferred (NewbornMode::Proxy or LegacyFirstSeen).
    New { t: i64, proxy: bool },
    /// Not created inside the observed data (pre-existing) or unknown -> old-coin path.
    Old,
}
impl Birth {
    pub fn is_new(&self) -> bool;
    /// 0 old/unknown, 1 proxy newborn, 2 on-chain creation (feature rd_birth_src).
    pub fn code(&self) -> u8;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CoinKind { New, Old }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum RadarState { #[default] Cold, Watch, Hot }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    Promote,  // -> Hot (unchanged meaning)
    Demote,   // Hot -> Cold (sweep, unchanged)
    Watch,    // Cold -> Watch (NEW)
    Unwatch,  // Watch -> Cold (sweep, NEW)
    Decide,   // repeated decision row while Watch/Hot (NEW, V5-2)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewbornMode {
    #[default] Auto,   // authoritative when the window has a creation scan, else proxy
    Authoritative,     // force on-chain creations (diagnostics; see 5.2)
    Proxy,             // force proxy rule
    LegacyFirstSeen,   // V4-1 (first seen > window start + proxy_warmup_s), reproduction only
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BirthSource { Authoritative, Proxy, LegacyFirstSeen }
impl BirthSource { pub fn as_str(&self) -> &'static str }
```
Callers only compare signals with `==`: `== Some(Signal::Promote)` and `s == Signal::Demote`. There are no exhaustive matches, so the new variants do not break existing code.

### 2.4 `Params` (V4 fields unchanged; additions appended; struct-level serde default)
```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Params { /* V4 fields ... */ /* D2 fields below */ }
```
V4 fields keep their names and meaning: `win_s, real_min_sol, min_buyers, min_buy_sol, max_buy_sell_ratio, max_median_buy_sol, tau_fast_s, tau_slow_s, min_accel, demote_idle_s, slow_win_s, slow_min_buyers, slow_min_buy_sol, lift, lift_min_buyers`.

`min_buyers` / `min_buy_sol` / `min_accel` are the fast-rule thresholds shared by both paths. `max_median_buy_sol` / `max_buy_sell_ratio` now apply to the **new-coin path only**; the old path has its own `old_*` filters.

One Default change: `slow_min_buy_sol` goes from `f64::INFINITY` to `1e18`. serde_json writes ±inf as `null`, which cannot be read back. The slow rule stays disabled through the `slow_min_buyers == usize::MAX` sentinel, so behaviour is unchanged.

| New field | Type | Default | Legacy file | Meaning |
|---|---|---|---|---|
| newborn_mode | NewbornMode | `auto` | `legacy_first_seen` | 0(a) |
| proxy_warmup_s | i64 | 60 | 60 | proxy / legacy warm-up after window start |
| proxy_max_progress | f64 | 0.05 | – | proxy: curve progress after the first trade < this |
| newborn_auto_hot | bool | false | true | V4 Radar 1 (legacy only) |
| new_max_age_s | Option<i64> | Some(1800) | null | new-coin path while age ≤ this (V4 spec §8.1 "~30 min"); null = forever |
| exclude_creator | bool | true | false | creator trades not counted as real buyers / buy SOL / EWMA |
| count_clusters | bool | true | false | count clusters (true) or wallets (false) |
| old_max_median_buy_sol | Option<f64> | None | 1.0 | old path median block (None = no block, file 01 §5.1) |
| old_max_buy_sell_ratio | Option<f64> | Some(4.0) | 4.0 | old path buy/sell cap; None = off (re-test) |
| old_flow_rule | bool | true | false | flow/liquidity + growth/CUSUM promotion rule |
| old_min_clusters | usize | 5 | – | flow rule: unique real buying clusters in win_s |
| old_min_nfl_60 | f64 | 0.01 | – | flow rule: (B−S)/L over win_s |
| old_min_liq_sol | f64 | 5.0 | – | flow rule: real liquidity floor (V4-4 spirit) |
| old_min_growth | f64 | 1.5 | – | flow rule: cluster growth G (or a recent CUSUM alarm) |
| liq_floor_sol | f64 | 1.0 | – | denominator floor for every ÷L |
| watch | bool | true | false | WATCH state on/off |
| watch_min_clusters | usize | 3 | – | growth trigger |
| watch_min_growth | f64 | 2.0 | – | growth trigger |
| watch_max_rise | Option<f64> | Some(0.10) | – | old-coin WATCH only while price ≤ win-low × 1.10 ("flat") |
| watch_max_s | i64 | 1800 | – | Unwatch when no trigger for this long |
| split_min_buys | usize | 5 | – | split-buying trigger: buys of one cluster in 300 s |
| split_min_sol | f64 | 0.3 | – | … summing to ≥ max(this, split_min_liq_frac·L) |
| split_min_liq_frac | f64 | 0.005 | – | |
| split_max_avg_sol | f64 | 0.05 | – | … with average buy ≤ this |
| decide_every_s | i64 | 5 | 0 | Decide spacing per coin (0 = off → Promote-only decision rows) |
| decide_in_watch | bool | true | – | also emit Decide while WATCH |
| decide_max_age_s | i64 | 1800 | – | Decide window after entering a state / after a CUSUM re-trigger |
| cusum | CusumParams | §3.2 | `{"enabled": false}` | nested, also `#[serde(default)]` |

`impl Params { pub fn horizon_s(&self) -> i64 { self.slow_win_s.max(300).max(2 * self.win_s) } }`

Thresholds that mean "disabled" use `Option<f64>`, never ±inf. `null` round-trips through JSON.

### 2.5 `Radar` struct and methods
```rust
pub struct Radar {
    p: Params,
    coins: FxHashMap<u32, Coin>,
    pending_birth: FxHashMap<u32, Birth>,
    newborn_after: Option<i64>,      // deprecated shim only
    pop: cusum::PopBaseline,         // shared population baselines (causal)
    pub hot_now: usize, pub hot_max: usize,      // unchanged
    pub watch_now: usize, pub watch_max: usize,  // NEW
}
impl Radar {
    pub fn new(p: Params) -> Self;
    #[deprecated(note = "births come from app::radar_feed::BirthTable via set_birth")]
    pub fn with_newborn_after(self, t: i64) -> Self;  // coins without set_birth: New{first,proxy:true} if first > t
    /// Must be called before the coin's first trade; otherwise the coin is Birth::Old.
    pub fn set_birth(&mut self, coin: u32, b: Birth);
    pub fn sweep(&mut self, now: i64, out: &mut Vec<(u32, Signal)>);   // Demote + Unwatch
    pub fn on_trade(&mut self, t: &Trade) -> Option<Signal>;            // Promote > Watch > Decide
    pub fn is_newborn(&self, coin: u32) -> Option<bool>;               // kept: birth.is_new()
    pub fn birth(&self, coin: u32) -> Option<Birth>;
    pub fn state(&self, coin: u32) -> RadarState;                      // Cold if unknown
    pub fn n_alarms(&self, coin: u32) -> u32;                          // cheap (radar_eval)
    pub fn view(&self, coin: u32, now: i64) -> Option<CoinView>;      // features (one metrics code path)
    pub fn params(&self) -> &Params;
}
```

### 2.6 Per-coin state
```rust
struct Rec { time: i64, trader: u32, who: u32 /* cluster or trader */, buy: bool, dust: bool, creator: bool,
             sol: f64, price: f64 /* exec */, px: f64 /* mprice if finite else exec */ }
struct Coin {
    first_time: i64, birth: Birth,
    tr: VecDeque<Rec>,                    // ALL trades (dust tagged) within p.horizon_s()
    fast: Ewma, slow: Ewma, last_real_buy: i64,
    state: RadarState, state_since: i64, watch_trigger_t: i64, promoted_once: bool,
    last_decide: i64 /* i64::MIN/2 */, decide_until: i64 /* i64::MIN */, decisions_in_state: u32,
    liq: Option<f64>,                     // latest real liquidity, SOL (unfloored)
    cusum: cusum::CoinCusum, last_alarm: Option<(i64, Channel)>, n_alarms: u32,
}
fn kind(c: &Coin, now: i64, p: &Params) -> CoinKind  // New iff birth New{t} and new_max_age_s.map_or(true, |m| now - t <= m)
fn age_ref(c: &Coin) -> i64                           // birth t for New, first_time for Old
```

### 2.7 `Metrics` — one code path for rules and features
```rust
pub const SPLIT_WIN_S: i64 = 300;
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Metrics {
    pub clusters_win: u32,        // unique real, non-creator buying `who` in (now-win_s, now]
    pub clusters_prev_win: u32,   // same in (now-2win_s, now-win_s]
    pub buy_sol_win: f64,         // real, non-creator buys
    pub sell_sol_win: f64,        // real sells (creator included: real selling pressure)
    pub median_buy_win: f64,      // sorted per-trade real non-creator buy sizes [len/2] (V4 definition), 0 if none
    pub growth: f64,              // clusters_win / (clusters_prev_win + 1)
    pub rise_from_low_win: Option<f64>,  // px_last / min px over win - 1
    pub dust_n_win: u32, pub dust_sol_win: f64,        // dust BUYS (sol < real_min_sol)
    pub dust_top_cluster_share_win: Option<f64>,       // max_who dust_sol / dust_sol (None if no dust)
    pub split_max_sol_300: f64, pub split_max_n_300: u32,   // cluster with max total buy SOL in 300 s (incl. dust, excl. creator)
    pub liq_sol: Option<f64>,
    pub nfl_win: Option<f64>,     // (buy_sol_win - sell_sol_win) / max(liq, liq_floor_sol)
}
fn metrics(c: &Coin, now: i64, p: &Params) -> Metrics
```
This is a single pass over `c.tr`, with the same O(n) cost profile as V4's `slow_rule`. Window membership is `r.time > now - w`, the same as V4 expiry `<= t - w`.

Optional optimisation (not required): incremental per-`who` aggregates for the 300 s split window.

### 2.8 `on_trade` algorithm (exact order)
```rust
pub fn on_trade(&mut self, t: &Trade) -> Option<Signal> {
    let p = self.p;
    if !self.coins.contains_key(&t.coin) {                       // birth fixed at first trade (causal)
        let b = self.pending_birth.remove(&t.coin).unwrap_or(match self.newborn_after {
            Some(a) if t.time > a => Birth::New { t: t.time, proxy: true }, _ => Birth::Old });
        self.coins.insert(t.coin, Coin::new(t.time, b));
    }
    let c = self.coins.get_mut(&t.coin).unwrap();
    let dust = t.sol < p.real_min_sol;
    let creator = p.exclude_creator && t.by_creator;
    let who = if p.count_clusters { t.cluster } else { t.trader };
    if t.liq_sol.is_finite() && t.liq_sol >= 0.0 { c.liq = Some(t.liq_sol); }
    // 1) CUSUM: close every step that ended before this trade, then add the trade to the open step
    let mut alarm = None;
    if p.cusum.enabled {
        let birth_t = if let Birth::New { t: b, .. } = c.birth { Some(b) } else { None };
        alarm = c.cusum.advance(t.time, &p.cusum, &mut self.pop, birth_t, c.liq.map(|l| l.max(p.liq_floor_sol)));
        if !dust && !creator { c.cusum.add(who, t.buy, t.sol); }
        if let Some(ch) = alarm { c.last_alarm = Some((t.time, ch)); c.n_alarms += 1; }
    }
    // 2) keep EVERY trade, tagged (no dust drop)
    c.tr.push_back(Rec { time: t.time, trader: t.trader, who, buy: t.buy, dust, creator, sol: t.sol, price: t.price,
                         px: if t.mprice.is_finite() && t.mprice > 0.0 { t.mprice } else { t.price } });
    while c.tr.front().is_some_and(|r| r.time <= t.time - p.horizon_s()) { c.tr.pop_front(); }
    // 3) real-buy clock + EWMA (V4 semantics)
    if !dust && t.buy && !creator { c.fast.add(t.time as f64, 1.0, p.tau_fast_s); c.slow.add(t.time as f64, 1.0, p.tau_slow_s); c.last_real_buy = t.time; }
    let real = !dust;
    let k = kind(c, t.time, &p);
    let alarm_recent = c.last_alarm.is_some_and(|(a, _)| t.time - a <= p.cusum.hold_s);
    if alarm.is_some() && c.state != RadarState::Cold {          // re-trigger keeps decisions coming
        c.decide_until = c.decide_until.max(t.time + p.decide_max_age_s); c.watch_trigger_t = t.time;
    }
    // 4) metrics once, only when a rule can use them
    let need_m = c.state != RadarState::Hot && (alarm.is_some() || (t.buy && ((real && !creator) || (p.watch && c.state == RadarState::Cold && k == CoinKind::Old))));
    let m = need_m.then(|| metrics(c, t.time, &p));
    let mut out = None;
    // 5) Cold -> Watch
    if p.watch && c.state == RadarState::Cold {
        let trig = match k {
            CoinKind::New => true,                                   // a newborn is watched from its first trade
            CoinKind::Old => m.as_ref().is_some_and(|m| flat(m, &p) && (alarm.is_some() || watch_trigger(m, &p))),
        };
        if trig { transition(c, RadarState::Watch, t.time, &p, /*counters*/); out = Some(Signal::Watch); }
    } else if c.state == RadarState::Watch && m.as_ref().is_some_and(|m| watch_trigger(m, &p)) {
        c.watch_trigger_t = t.time;
    }
    // 6) Promotion: real, non-creator buy, not HOT
    if c.state != RadarState::Hot && real && t.buy && !creator {
        let m = m.as_ref().expect("computed for real non-creator buys");
        let promote = match k {
            CoinKind::New if !p.newborn_auto_hot => new_rule(c, m, t.time, &p),
            CoinKind::New if !c.promoted_once => true,                // legacy V4 Radar 1 only
            _ => old_rules(c, m, t.time, &p, alarm_recent),           // also legacy re-promotion of newborns
        };
        if promote { transition(c, RadarState::Hot, t.time, &p, ..); c.promoted_once = true; return Some(Signal::Promote); }
    }
    if out.is_some() { return out; }
    // 7) Repeated decision rows (V5-2)
    if p.decide_every_s > 0 && real
        && (c.state == RadarState::Hot || (c.state == RadarState::Watch && p.decide_in_watch))
        && t.time - c.last_decide >= p.decide_every_s && t.time <= c.decide_until {
        c.last_decide = t.time; c.decisions_in_state += 1; return Some(Signal::Decide);
    }
    None
}
```
`transition(c, to, now, p, &mut hot_now, &mut hot_max, &mut watch_now, &mut watch_max)` is a free fn, because the fields are borrowed disjointly. It does the following:
- updates the counters for leaving/entering the state;
- sets `state = to`, `state_since = now`, `decisions_in_state = 1` (the transition row is itself a decision row);
- sets `last_decide = now`, `decide_until = now + p.decide_max_age_s`, `watch_trigger_t = now`.

### 2.9 Rules
```rust
fn accel_ok(c: &Coin, now: i64, p: &Params) -> bool {          // V4: young (< tau_slow since birth/first seen) or accel
    ((now - age_ref(c)) as f64) < p.tau_slow_s || c.fast.at(now as f64, p.tau_fast_s) / (c.slow.at(now as f64, p.tau_slow_s) + 1e-9) >= p.min_accel
}
fn fast_rule(m: &Metrics, p: &Params, max_med: Option<f64>, max_bs: Option<f64>, accel: bool) -> bool {
    m.clusters_win as usize >= p.min_buyers && m.buy_sol_win >= p.min_buy_sol && m.buy_sol_win > m.sell_sol_win
      && max_bs.map_or(true, |r| !(m.sell_sol_win > 0.0 && m.buy_sol_win / m.sell_sol_win > r))
      && max_med.map_or(true, |x| m.median_buy_win <= x) && accel
}
/// NEW-coin path (strict, no auto-HOT): V4 fast rule WITH the median and buy/sell filters (file 01 §2.4: they help new coins).
fn new_rule(c, m, now, p) -> bool { fast_rule(m, p, Some(p.max_median_buy_sol), Some(p.max_buy_sell_ratio), accel_ok(c, now, p)) }
/// OLD-coin path: V4 awakening rules without the median block + flow/liquidity rule.
fn old_rules(c, m, now, p, alarm_recent) -> bool {
    slow_rule(c, now, p) || lift_rule(c, now, p)
      || fast_rule(m, p, p.old_max_median_buy_sol, p.old_max_buy_sell_ratio, accel_ok(c, now, p))
      || (p.old_flow_rule && flow_rule(m, p, alarm_recent))
}
fn flow_rule(m: &Metrics, p: &Params, alarm_recent: bool) -> bool {
    m.clusters_win as usize >= p.old_min_clusters && m.buy_sol_win > m.sell_sol_win
      && m.liq_sol.map_or(true, |l| l >= p.old_min_liq_sol)           // unknown L (raw radar_eval) -> not checked
      && m.nfl_win.map_or(true, |x| x >= p.old_min_nfl_60)
      && (m.growth >= p.old_min_growth || alarm_recent)
      && p.old_max_buy_sell_ratio.map_or(true, |r| !(m.sell_sol_win > 0.0 && m.buy_sol_win / m.sell_sol_win > r))
}
fn flat(m: &Metrics, p: &Params) -> bool { p.watch_max_rise.map_or(true, |x| m.rise_from_low_win.map_or(true, |r| r <= x)) }
fn watch_trigger(m: &Metrics, p: &Params) -> bool {                 // old coins, leading signals (file 02 §15)
    (m.clusters_win as usize >= p.watch_min_clusters && m.growth >= p.watch_min_growth) || split(m, p)
}
fn split(m: &Metrics, p: &Params) -> bool {                         // hidden accumulation / split buying (file 01 §5.3)
    m.split_max_n_300 as usize >= p.split_min_buys
      && m.split_max_sol_300 >= p.split_min_sol.max(m.liq_sol.map_or(0.0, |l| p.split_min_liq_frac * l))
      && m.split_max_sol_300 / m.split_max_n_300.max(1) as f64 <= p.split_max_avg_sol
}
```
`slow_rule(c, now, p)` and `lift_rule(c, now, p)` are re-implemented over `c.tr`, filtered to `!dust` and `r.time > now - slow_win_s`:
- buyers = `who` of non-creator real buys, deduped.
- `lift_rule` uses `r.price` (exec price), takes the **first** strict minimum as V4 does, and counts buyers/flow over the real records after that minimum.

The result is identical to V4 when `exclude_creator = false` and `count_clusters = false`.

### 2.10 `sweep`
```rust
for (id, c) in self.coins.iter_mut() {
    match c.state {
        RadarState::Hot if now - c.last_real_buy > p.demote_idle_s => { -> Cold; hot_now -= 1; out.push((*id, Signal::Demote)) }
        RadarState::Watch if now - c.last_real_buy > p.demote_idle_s || now - c.watch_trigger_t > p.watch_max_s
            => { -> Cold; watch_now -= 1; out.push((*id, Signal::Unwatch)) }
        _ => {}
    }
}
```

### 2.11 `CoinView` (read by the Engine for features)
```rust
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CoinView {
    pub state: RadarState, pub kind: CoinKind, pub birth: Birth, pub state_since: i64, pub decisions_in_state: u32,
    pub cusum_s: [Option<f64>; cusum::N_CH], pub cusum_pop: [Option<bool>; cusum::N_CH],
    pub last_alarm: Option<(i64, Channel)>, pub n_alarms: u32, pub metrics: Metrics,
}
```

### 2.12 Legacy reproduction (V4 radar exactly)
`config/radar_params_v4_legacy.json` (§9) plus the deprecated `with_newborn_after` shim, or `BirthSource::LegacyFirstSeen` from `RadarFeed`, reproduce every Promote/Demote of the V4 radar. With `watch = false`, `decide_every_s = 0` and `cusum.enabled = false`, the new code paths are inert:
- dust is recorded but filtered out of every legacy metric;
- creator and cluster handling are off;
- old filters = V4 filters.

Test `legacy_mode_matches_v4_reference_on_random_stream` (§10) proves this.

---------------------------------------------------------------------------------------------------------------------

## 3. `crates/radar/src/cusum.rs` (new, self-contained)

### 3.1 Math (file 02 Bagian 7; V4 spec §11.5)
- Sampling: fixed steps of `step_s` seconds aligned to absolute time (`step = time.div_euclid(step_s)`). Trades accumulate into the open step; a step closes when the coin's first trade of a later step arrives. Steps between the two are **idle steps**.
- Channels, computed per closed step from real (non-dust), non-creator trades of the coin:
  - `Clusters` (C1): n = unique buying `who`; x = Anscombe `2·sqrt(n + 3/8)`. This stabilises Poisson variance near 1, so the floor is 1.0. Idle: x = 2·sqrt(3/8).
  - `BuySol` (F4, log size): x = `ln(1 + B_step / bsol_unit)`. Idle: 0.
  - `NetFlowLiq` (F2): x = `(B_step − S_step) / L`, with L = latest real liquidity floored at `liq_floor_sol`. The channel is disabled (None) while L is unknown. Idle: 0.
- Robust causal baseline from the coin's own **past** steps; the current step is excluded:
  - `MedianMad` (default): over the last `base_n` samples, μ̂ = median, σ̂ = 1.4826·MAD.
  - `Ewma`: α = 1/ewma_tau_steps, μ ← μ + α(x−μ), var ← (1−α)(var + α(x−μ)²).
- σ = max(σ̂, floor_ch). Because of the floor, MAD = 0 never produces Inf/NaN (V4 spec §33.4).
- Warm-up: while own samples < `warmup_n`, use the **population baseline** of the coin's age bucket. Buckets are V4 §11.4 by age since birth: [0,10), [10,30), [30,60), [60,120), [120,300), [300,900), [900,3600), [3600,21600), ≥21600; bucket 9 = age unknown (Birth::Old). The population baseline is fed by the closed steps of coins processed earlier (causal) and refreshed at most every `pop_refresh_s` of event time (deterministic). If it has fewer than `pop_min` samples → no z, no update ("not ready").
- `z = (x − μ̂)/σ`; `S = max(0, S + z − k)`; alarm iff `S > h`, then `S = 0` (reset). Each channel has its own S. `advance` returns the first alarming channel in the order Clusters, BuySol, NetFlowLiq.
- Gap: at most `max_gap_steps` idle steps are processed (they go into the own baseline and the population baseline). A longer gap additionally resets every S to 0 and jumps to the current step.

### 3.2 Params
```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CusumParams {
    pub enabled: bool,          // true
    pub step_s: i64,            // 5
    pub k: f64,                 // 0.5
    pub h: f64,                 // 5.0  (spec 4-5; grid {4,5} on dev windows only)
    pub base_n: usize,          // 60 steps (5 min)
    pub warmup_n: usize,        // 12 steps (1 min)
    pub baseline: BaselineKind, // median_mad
    pub ewma_tau_steps: f64,    // 60.0
    pub pop_min: usize,         // 64
    pub pop_cap: usize,         // 2048 samples per (bucket, channel)
    pub pop_refresh_s: i64,     // 5
    pub max_gap_steps: usize,   // 60
    pub hold_s: i64,            // 60: an alarm is "recent" for the flow rule
    pub bsol_unit: f64,         // 0.1 SOL
    pub floor_uc: f64,          // 1.0
    pub floor_bsol: f64,        // 0.5
    pub floor_nfl: f64,         // 0.002
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineKind { #[default] MedianMad, Ewma }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Channel { Clusters = 0, BuySol = 1, NetFlowLiq = 2 }
pub const N_CH: usize = 3;
pub const N_BUCKETS: usize = 10;
```

### 3.3 API
```rust
pub fn median_mad(v: &mut [f64]) -> (f64, f64);              // (median, 1.4826*MAD); empty -> (NaN, NaN)
pub fn age_bucket(age_s: Option<i64>) -> usize;              // None -> 9
pub struct RobustBaseline { ring: VecDeque<f64>, cap: usize, ew_mean: f64, ew_var: f64, n: usize, scratch: Vec<f64> }
impl RobustBaseline {
    pub fn new(cap: usize) -> Self;
    pub fn len(&self) -> usize;
    pub fn push(&mut self, x: f64, kind: BaselineKind, tau: f64);
    pub fn estimate(&mut self, kind: BaselineKind) -> Option<(f64, f64)>;   // (mu, sigma_raw)
}
#[derive(Debug, Default, Clone, Copy)]
pub struct Cusum { pub s: f64 }
impl Cusum { pub fn update(&mut self, z: f64, k: f64, h: f64) -> bool }   // alarm resets s to 0
pub struct PopBaseline { rings: Vec<PopRing> /* N_BUCKETS * N_CH */ }
impl PopBaseline {
    pub fn new() -> Self;
    pub fn push(&mut self, bucket: usize, ch: Channel, x: f64, cap: usize);
    pub fn estimate(&mut self, bucket: usize, ch: Channel, now: i64, p: &CusumParams) -> Option<(f64, f64)>;
}
pub struct CoinCusum { step: i64, acc: StepAcc, base: [RobustBaseline; N_CH], cs: [Cusum; N_CH],
                       ready: [bool; N_CH], pop_used: [Option<bool>; N_CH] }
impl CoinCusum {
    pub fn new(p: &CusumParams) -> Self;
    pub fn advance(&mut self, t: i64, p: &CusumParams, pop: &mut PopBaseline, birth_t: Option<i64>, liq_floored: Option<f64>) -> Option<Channel>;
    pub fn add(&mut self, who: u32, buy: bool, sol: f64);
    pub fn s(&self, ch: Channel) -> Option<f64>;          // None until the channel produced a z once
    pub fn pop_used(&self, ch: Channel) -> Option<bool>;
}
```
`StepAcc { clusters: Vec<u32> (dedup at close), buy_sol: f64, sell_sol: f64 }`. The age bucket of a closed step is `age_bucket(birth_t.map(|b| step_start - b))`.

Note: the population ring also contains the coin's own earlier samples. This is negligible: at most 12 out of ≤ 2048, and only used during warm-up.

---------------------------------------------------------------------------------------------------------------------

## 4. `crates/decoder/src/pump.rs` (existing pump decoder; not a new venue)

1. `pub const CURVE_INITIAL_REAL_TOKEN: f64 = 793_100_000_000_000.0;` moves here. `ml_sim.rs:44` then uses `decoder::pump::CURVE_INITIAL_REAL_TOKEN`, and `radar_feed` uses it too.
2. `Creation` gains `pub migration: bool`. In `decode_creations` (pump.rs:200-233), a first pass over `tx.exec` collects the `mint` of every pump `CompletePumpAmmMigrationEvent` in the tx. A PumpSwap `CreatePoolEvent` whose `base_mint` is in that set gets `migration = true`; a curve `CreateEvent` gets `false`.

   Why: a curve migration emits pump `CompletePumpAmmMigrationEvent` **and** pump_amm `CreatePoolEvent` in the same tx (verified in `config/idl/pump.json` / `pump_amm.json`). Without the flag, a window-wide scan would mark every migrated old coin as a newborn.
3. `follow.rs:343`: `.find(|c| c.mint == mint && !c.migration)`.

---------------------------------------------------------------------------------------------------------------------

## 5. `crates/app/src/radar_feed.rs` (new; `mod radar_feed;` in main.rs)

### 5.1 TradeBuilder — the only `Ev → radar::Trade` construction
```rust
#[derive(Default, Clone, Copy)] struct FeedCoin { seen: bool, on_amm: bool, liq_sol: f64, mprice: f64 }
pub struct TradeBuilder { coins: Vec<FeedCoin>, clusters: Option<Arc<[u32]>> }
impl TradeBuilder {
    pub fn new(n_coins: usize, clusters: Option<Arc<[u32]>>) -> Self;
    pub fn cluster_of(&self, user: u32) -> u32;          // lookup or identity
    /// Returns (trade, first_event_of_coin).
    pub fn build(&mut self, e: &Ev) -> (Trade, bool);
}
```
`build`:
- Active-pool rule, the same as `Engine::apply` / `backtest` Pool: `if e.amm || !fc.on_amm { fc.liq_sol = if e.amm { e.quote_res as f64/1e9 } else { e.real_quote as f64/1e9 }; fc.mprice = if e.token_res > 0 { e.quote_res as f64 / e.token_res as f64 } else { NaN } }`, then `fc.on_amm |= e.amm`.
- `price = solf / e.tokens` (0 if tokens = 0), as before.
- `cluster = cluster_of(e.user)`.
- `by_creator = e.user == e.creator || (clusters.is_some() && cluster_of(e.user) == cluster_of(e.creator))`.
- `amm = e.amm`.

### 5.2 BirthTable
```rust
pub struct BirthTable { pub source: BirthSource, window_start: i64, created: Vec<Option<i64>>, warmup_s: i64, max_progress: f64 }
impl BirthTable {
    pub fn new(ev: &Events, p: &Params) -> Self;
    /// Pure: depends only on the coin's first event (causal), the window start and the creation table.
    pub fn birth(&self, coin: u32, first: &Ev) -> Birth;
}
```
Source resolution:
- `Auto` → `Authoritative` if `ev.creations_scanned`, else `Proxy`.
- `Authoritative` forced without a scan → `tracing::warn!` that only follow-up creations exist. Diagnostics only: follow creations exist only for HOT-selected coins, which is a selection leak.
- `Proxy` / `LegacyFirstSeen` map to themselves.

Birth rules:
- **Authoritative**: `New{t: ct, proxy: false}` iff `ev.created[c] = Some((ct, _))` with `ct >= window.0`, else `Old`. Only in-window creations (from the unselected window scan) count. Pre-window creation times that come from `follow` are ignored for births, because they exist only for followed (HOT) coins and would leak that selection. Boundary effect: a coin created < `new_max_age_s` before window start is treated as old.
- **Proxy** (forensic Appendix A, labelled PROXY): `New{t: first.time, proxy: true}` iff `first.time > window.0 + proxy_warmup_s && !first.amm && (1 − first.real_token as f64 / CURVE_INITIAL_REAL_TOKEN) < proxy_max_progress`, else `Old`. `ev.created` is **not** used in proxy mode (selection leak).
- **LegacyFirstSeen**: `New{t: first.time, proxy: true}` iff `first.time > window.0 + proxy_warmup_s`, else `Old`.

### 5.3 RadarFeed
```rust
pub struct RadarFeed { radar: Radar, tb: TradeBuilder, births: BirthTable, last_sweep: i64 }
impl RadarFeed {
    pub fn new(ev: &Events, p: Params) -> Self;                       // clusters None
    pub fn set_clusters(&mut self, c: Option<Arc<[u32]>>);            // before the first event
    pub fn radar(&self) -> &Radar;
    pub fn births(&self) -> &BirthTable;
    pub fn birth_source(&self) -> BirthSource;
    /// Sweep once per new second (Demote/Unwatch appended to `swept`), then build the trade, register the
    /// birth on the coin's first event and feed the radar.
    pub fn step(&mut self, e: &Ev, swept: &mut Vec<(u32, Signal)>) -> Option<Signal>;
}
/// `{"params": {...}}` file (radar-tune output); missing fields take Params::default() values (serde default).
pub fn load_params(path: &Path) -> anyhow::Result<radar::Params>;
```

---------------------------------------------------------------------------------------------------------------------

## 6. Creation data: `Events`, sidecar file, loaders, follow merge (`backtest.rs`, `follow.rs`)

### 6.1 Events
`pub creations_scanned: bool`: the whole window was scanned for creation events (`creations.bin.zst`, `complete = true`, same window). `Events::new` sets it to `false`; no other constructor exists. Doc of `created` becomes: "real creation (time, price): in-window creations from the window scan, or the mint's first signature from follow".

### 6.2 Sidecar format (`backtest.rs`)
```rust
pub const CREATIONS_FILE: &str = "creations.bin.zst";   // bincode + zstd (like follow.bin.zst)
const CREATIONS_VERSION: u32 = 1;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreationRec { pub mint: String, pub slot: u64, pub order: u64 /* (slot<<24)|(tx<<8) */, pub time: i64,
                         pub amm: bool, pub migration: bool, pub price: f64 }
#[derive(Debug, Serialize, Deserialize)]
pub struct CreationsFile { pub version: u32, pub window: (i64, i64), pub first_slot: u64, pub last_slot: u64,
                           pub complete: bool, pub items: Vec<CreationRec> }
pub fn read_creations(path: &Path) -> Result<CreationsFile>;
fn write_creations(path: &Path, f: &CreationsFile) -> Result<u64>;
fn attach_creations(ev: &mut Events, rec: &Path) -> Result<()>;
pub fn scan_creations(rec: &Path) -> Result<CreationsFile>;   // raw recording -> sidecar
```
- `load_window` raw branch (backtest.rs:110-170): in the tx loop, after `dec.decode(&tx)`, also run `for cr in dec.decode_creations(&tx)` → push `CreationRec{ time: if cr.time > 0 {cr.time} else {time}, order: (slot<<24)|((i as u64)<<8), amm: cr.venue==Venue::Amm, migration: cr.migration, price: cr.price, .. }`. Then write `creations.bin.zst` with `complete: true` next to `pump_events_v2.bin`.
- All three `load_window` branches end with `let mut ev = Events::new(..); attach_creations(&mut ev, rec)?; Ok(ev)`.
- `attach_creations`:
  - file missing → no-op;
  - `f.window != ev.window` → warn and no-op;
  - otherwise, for each `!migration` item whose mint is in `ev.mints` → `ev.created[c] = Some((time, Some(price)))`;
  - `ev.creations_scanned = f.complete`.
- `scan_creations(rec)`: the same raw loop as `load_window`, `decode_creations` only; the window is computed the same way (min/max blockTime). If the raw segments are missing, it fails with an error saying the window stays in proxy mode.

### 6.3 follow.rs
- `merge` (follow.rs:124): `ev.created[ci] = ev.created[ci].or(c.created);`, so window-scan data is never overwritten.
- `Opts` gains `pub min_state: radar::RadarState` (Hot default; Watch also follows WATCH coins and costs more Helius credits).
- `hot_coins(ev, rp, min_state)` uses `RadarFeed`:
```rust
pub fn hot_coins(ev: &Events, rp: RadarParams, min_state: RadarState) -> Vec<(u32, i64)> {
    let mut feed = RadarFeed::new(ev, rp);
    let (mut first, mut swept) = (FxHashMap::default(), Vec::new());
    for e in &ev.evs {
        swept.clear();
        let s = feed.step(e, &mut swept);
        let hit = match min_state { RadarState::Watch => matches!(s, Some(Signal::Promote | Signal::Watch)), _ => s == Some(Signal::Promote) };
        if hit { first.entry(e.coin).or_insert(e.time); }
    }
    /* sort as before */
}
```
- `follow_summary.json` gains `"birth_source"`, `"creations_scanned"` and `"min_state"`.
- Known drift: follow runs before `wallet_scores.py`, so `hot_coins` has no cluster lookup, while `Engine` may have one. Coins promoted only with clusters are not followed, and their rows are censored. This is documented and counted in `SnapStats.dropped_censored_signal`.

---------------------------------------------------------------------------------------------------------------------

## 7. `ml_sim.rs` Engine integration (the single feature path)

### 7.1 Fields
- `radar: Radar` (ml_sim.rs:218) becomes `feed: crate::radar_feed::RadarFeed`.
- Keep `sig: Vec<(u32, Signal)>`.
- Add `creations_scanned: bool` and `clusters_known: bool`.
- `Coin` adds `track_time: Option<i64>` (first Watch or Promote).
- `Engine::new` (ml_sim.rs:247-267): `feed: RadarFeed::new(ev, rp)`, `creations_scanned: ev.creations_scanned`.
- `with_ctx`: `self.feed.set_clusters(ctx.clusters.clone()); self.clusters_known = ctx.clusters.is_some();`.
- New accessors: `pub fn radar(&self) -> &radar::Radar`, `pub fn ever_hot(&self, coin: u32) -> bool` (= `hot_time.is_some()`).

### 7.2 `apply` changes
- Remove the sweep block (ml_sim.rs:309-313); `feed.step` sweeps.
- First-event block (ml_sim.rs:363-369): replace the V4-1 test with `if self.feed.births().birth(e.coin, e).is_new() && e.time <= self.window_end { self.new_coins.push_back(e.time) }`. The regime feature `new_coins_per_min` now counts radar births: same index, corrected definition.
- Lines 509-514 become:
```rust
self.sig.clear();
let rs = self.feed.step(e, &mut self.sig);
let signal = rs == Some(Signal::Promote);
let decision = matches!(rs, Some(Signal::Promote | Signal::Watch | Signal::Decide));
if signal { c.hot_time.get_or_insert(e.time); }
if decision { c.track_time.get_or_insert(e.time); }
let due = decision || (real && c.track_time.is_some_and(|h| e.time - c.last_snap >= snap_every(e.time - h)));
...
Step { snapshot, signal, decision, radar_state: self.feed.radar().state(e.coin), dev_sell, dev_sell_frac }
```

### 7.3 `Step`
```rust
pub struct Step {
    pub snapshot: Option<Vec<f32>>,
    pub signal: bool,                       // radar Promote (unchanged meaning)
    pub decision: bool,                     // NEW: decision row (Promote | Watch | Decide)
    pub radar_state: radar::RadarState,     // NEW: state after this event
    pub dev_sell: bool,
    pub dev_sell_frac: f64,
}
```

### 7.4 Leak-free creation in existing features (new causal issue introduced by WATCH rows)
Follow-up creation data exists only for coins the follow radar promoted. Before D2, snapshots existed only after HOT, so this was harmless. WATCH/Decide rows **before** HOT would leak "will become HOT" through `created_known`, `age_s`, `mult_since_first`, `early20_delay_med_s` and `newborn`. Fix:
```rust
fn created_for_features(&self, ci: usize, c: &Coin) -> Option<(i64, Option<f64>)> {
    let cr = self.created.get(ci).copied().flatten()?;
    ((self.creations_scanned && cr.0 >= self.window_start) || c.hot_time.is_some()).then_some(cr)
}
```
- `features()` lines 663-669: `let created = self.created_for_features(ci, c);`, then born / first_px as before.
- `newborn` (index 1) becomes `self.feed.radar().birth(ci as u32).is_some_and(|b| b.is_new())`. Same index, corrected V4-1/V4-3 definition.
- `features_v5` index 0 `created_known` uses the same `created_for_features`.
- Rows after HOT, i.e. all V5 rows today, keep their values.

### 7.5 Appended features (D2 block, 23 names, after any other WP's blocks; contiguous; prefix `rd_`)
`FEATURES` stays `pub const FEATURES: [&str; N]` with a **literal** N, because `python/ml/v5_common.py::rust_str_array` parses `(\d+)`. The fill function is `fn features_rd(&self, ci: usize, t: i64) -> Vec<f32>`, called from `features()` after `features_v5`, using `let v = self.feed.radar().view(ci as u32, t)`. The `debug_assert_eq!(v.len(), FEATURES.len())` stays.

| # | Name | Definition (causal, state after this event) | Unknown |
|---|---|---|---|
| 1 | rd_state | 0 cold, 1 watch, 2 hot | – |
| 2 | rd_kind_new | 1 if new-coin path at t (birth New and age ≤ new_max_age_s) | – |
| 3 | rd_birth_src | 0 old/unknown, 1 proxy newborn, 2 on-chain creation | – |
| 4 | rd_secs_in_state | t − state_since | −1 if cold |
| 5 | rd_decisions_in_state | decision rows in the current state episode (incl. this) | 0 if cold |
| 6 | rd_cusum_uc | S of the Clusters channel | −1 not ready / disabled |
| 7 | rd_cusum_bsol | S of the BuySol channel | −1 |
| 8 | rd_cusum_nfl | S of the NetFlowLiq channel | −1 (also if L unknown) |
| 9 | rd_cusum_alarm_age_s | t − last alarm time (any channel) | −1 none |
| 10 | rd_cusum_alarms | alarms since first seen | – |
| 11 | rd_cusum_pop | 1 if the last Clusters z used the population baseline, 0 own | −1 none |
| 12 | rd_clusters_60s | unique real non-creator buying clusters, win_s (C1) | – |
| 13 | rd_cluster_growth | G = U_now/(U_prev+1) (C2, cluster version) | – |
| 14 | rd_cluster_accel | (U_now − U_prev)/win_s (C11, U4 difference form) | – |
| 15 | rd_nfl_60s | (B−S)/max(L_real, liq_floor) over win_s (F2 with REAL reserve) | NaN if L unknown (never in Engine) |
| 16 | rd_liq_real_sol | L_real, SOL | NaN if unknown |
| 17 | rd_dust_n_60s | dust buy count, win_s | – |
| 18 | rd_dust_sol_60s | dust buy SOL, win_s | – |
| 19 | rd_dust_sol_share_60s | dust_sol / (dust_sol + real buy SOL) | 0 if no buys |
| 20 | rd_dust_top_cluster_share_60s | largest cluster's dust SOL / dust SOL | −1 no dust |
| 21 | rd_split_max_sol_liq_300s | max per-cluster buy SOL (incl. dust, excl. creator) over 300 s / max(L, floor) | −1 if L unknown |
| 22 | rd_split_max_n_300s | buy count of that cluster | – |
| 23 | rd_clusters_known | 1 if a cluster lookup is loaded for this window, else 0 (degradation flag) | – |

Names 12-15 overlap the "core metrics" WP (F2/C1/C2/C11). If that WP appends the same quantities, keep one copy (prefer `rd_*`, because the radar rules use exactly these values) and drop the duplicate there.

### 7.6 Context + cluster lookup
```rust
#[derive(Default, Clone)]
pub struct Context {
    pub wallets: Vec<Option<WalletInfo>>,
    pub sol_usd: Vec<(i64, f64)>,
    /// NEW: user id -> cluster id (representative wallet's user id; synthetic ids >= users.len() for representatives
    /// not in this window). None = no lookup (cluster = wallet).
    pub clusters: Option<std::sync::Arc<[u32]>>,
}
```
`load_context` (ml_sim.rs:1236-1262):
- Optional column `cluster_id` (already written by `python/research/wallet_scores.py`: the representative wallet address, "" for singletons).
- `let mut cl: Vec<u32> = (0..n_users).collect();`
- For each row with a non-empty `cluster_id`: `cl[wallet_id] = ids.get(rep).copied().unwrap_or_else(|| synthetic(rep))`.
- `ctx.clusters = any.then(|| Arc::from(cl))`.

Literal fixes: ml_sim.rs:1237 and the test at :1871 use `..Default::default()`.

### 7.7 `write_snapshots`
- `is_signal` column ← `step.decision`. This is V5 plan §3.3 semantics: Promote + re-triggers. With `decide_every_s = 0` and `watch = false` it is exactly the old Promote-only column.
- The header is unchanged (`coin,order,time,is_signal,in_window` + FEATURES + LABEL_COLS), so `v5_train.py` asserts still hold.
- `SnapStats` adds `promote_rows`, `watch_rows` (decision rows with `radar_state == Watch`), `newborn_coins`, `birth_source: &'static str`, `creations_scanned: bool`, `clusters_known: bool`. `signal_rows` now counts decision rows.

### 7.8 `ml_sim::simulate` (V4 ML sim)
Entry gate at ml_sim.rs:1625: add `if !eng.ever_hot(e.coin) { continue; }` before scoring, so WATCH-only snapshots do not create V4 entries. Before D2, snapshots existed only for ever-HOT coins. Exits are unchanged.

---------------------------------------------------------------------------------------------------------------------

## 8. Remaining call sites — exact changes

| File:line | Change |
|---|---|
| backtest.rs:14 | `use radar::{Params as RadarParams, Signal};` + `use crate::radar_feed::RadarFeed;` (Radar, Trade no longer used) |
| backtest.rs:358 | `let mut feed = RadarFeed::new(ev, radar_p);` (no clusters: V4 rule backtest) |
| backtest.rs:368-369 | drop `last_sweep`/`sig` for the radar (keep `last_sweep` for the per-second position checks); add `let mut swept = Vec::new();` |
| backtest.rs:462-467 | remove the `radar.sweep` block (equivalent: the `hot` set is only read in step 5, after the radar call) |
| backtest.rs:533-537 | `swept.clear(); let s = feed.step(e, &mut swept); for (cid, sg) in swept.drain(..) { if sg == Signal::Demote { hot.remove(&cid); } } if s == Some(Signal::Promote) { hot.insert(e.coin); diag[..].first_hot_time.get_or_insert(..); }` |
| backtest.rs:96-171 | creations sidecar (§6.2); `Events.creations_scanned` |
| ml_sim.rs | §7 |
| follow.rs:17,124,167-187,259,343 | §6.3 and §4 |
| radar_eval.rs | §8.1 |
| strategy_v5.rs:23-60, 487 | §8.2 (minimal V5-2 hook) |
| v5_eval.rs:394 | `crate::ml_sim::Engine::new(&l.ev, rp).with_ctx(d.ctx.clone())`, the same radar input as the strategy run |
| diagnose.rs:21 | `let rp = crate::radar_feed::load_params(radar_params)?;` |
| main.rs:384,389,398,405,441,456,483,530,555,608 | `radar_feed::load_params(&path)?` (11 copies → 1 helper) |
| main.rs Cmd | `ScanCreations { rec }`; `Follow --min-state hot|watch` (default hot); `RadarEval --source auto|events|raw` (default auto); `RadarTune --source`, `--legacy` (old 224 grid), `--base-params config/radar_params.json`, `--max-hot-per-min 30.0` |
| census.rs:204 | no change (does not use Radar; the column is documented as "first trade > window start + 60 s") |
| radar/src/lib.rs tests | `Trade::basic(...)` in `tr()` and the `mk` closure |
| scripts/v5_fetch.sh | after `export-events`: `[ -f $d/creations.bin.zst ] \|\| $BIN scan-creations --rec $d > /dev/null 2>&1 \|\| { echo "$name creation scan FAILED (raw kept)"; continue; }` **before** `rm -f $d/raw-*.zst` |
| scripts/v5_save_datasets.sh | add `[ -f "$d/creations.bin.zst" ] && put "$d/creations.bin.zst" "$out/creations.bin.zst"` |

### 8.1 radar_eval.rs
- `Dataset` gains `pub births: Vec<Birth>` and `pub birth_source: BirthSource`. `trades: Vec<Trade>` uses the new fields.
- `pub fn load(rec, p: &Params) -> Result<Dataset>` (raw):
  - trades via `Trade::basic`;
  - additionally `TxView::parse(t)` + `pump_check::load_decoder()?.decode_creations` in the same loop;
  - births: `New{ct,false}` if the coin has a non-migration creation with `ct >= window.0`, else `Old`; source Authoritative. Raw has the full window, so the scan is complete.
  - Raw + forced Proxy → error, because balance_delta carries no curve progress.
- NEW `pub fn from_events(ev: &Events, p: &Params) -> Dataset`: `TradeBuilder` (no clusters) + `BirthTable`. This makes radar eval possible on `datasets/bf_1h`, `datasets/h1d` (no raw there).
- NEW `pub fn load_auto(rec, p, source: &str) -> Result<Dataset>`: `events` if `pump_events_v2.bin` / `pump_events.bin` exists (`backtest::load`), else `raw`.
- `run_radar` returns `struct RadarRun { promos, hot: Vec<Vec<(u64,u64)>>, armed: Vec<Vec<(u64,u64)>> /* Watch∪Hot */, hot_max, watch_max, n_watch, alarms: Vec<Vec<i64>> }`:
  - `set_birth` at each coin's first trade (from `ds.births`);
  - `Watch`/`Promote` open intervals; `Demote`/`Unwatch` close them;
  - alarms recorded when `r.n_alarms(coin)` changes.
- `Label` adds `cross10_order` (first smoothed price ≥ 1.1·base after base). `NEWBORN_WARMUP_S` is replaced by `p.proxy_warmup_s`.
- `TierResult` adds `caught_hot_before_10, recall_hot_before_10, caught_armed_before_10, recall_armed_before_10, caught_armed_before_30`. Each rate carries its n; n < 30 → `"direction_only": true`.
- `EvalResult` adds:
  - `birth_source`, `hot_per_min` (= promotions / window minutes), `watch_per_min`, `newborn_coins` (births New; replaces the proxy count);
  - `cusum: CusumEval { runners_5x, alarmed_before_10, median_delay_s, nonrunner_coin_hours, nonrunner_alarms, alarms_per_coin_hour }`;
  - detection = first alarm in [base, cross10]; non-runner = run < 2x; coin-hours from first to last trade, min 60 s (file 02 §7.8: ARL + delay).
- NEW `pub fn tune_v6(sets, base: Params, max_hot_per_min: f64) -> (Params, Vec<(Params, Vec<EvalResult>)>, Vec<(String, Vec<EvalResult>)>)`:
  - Grid (16 ≤ 24): `cusum.h ∈ {4,5} × old_min_clusters ∈ {4,6} × old_min_nfl_60 ∈ {0.0,0.02} × old_max_buy_sell_ratio ∈ {Some(4.0), None}`. Other fields come from `base`.
  - Selection rule, fixed before results: maximise the **median over dev windows** of `mean(recall_hot_before_10, recall_hot_before_30)` at tier 5x, averaged over {new, old}. Subject to `hot_per_min ≤ max_hot_per_min` and `precision_2x ≥ naive_precision_2x` on every dev window. Ties → fewer promoted coins.
  - Ablation report for the chosen params (evaluated, not selected): `no_cusum`, `no_flow_rule`, `no_watch`, `old_median_block_1.0`, `legacy` (the legacy file).
- The old `tune` is kept for `--legacy`. The params file `objective` string is updated for v6.

### 8.2 strategy_v5.rs (minimal V5-2 hook; the strategy WP owns ranking / ARM / price guard)
- `StratV5` gets `#[serde(default)]` at struct level and two fields (Default impl extended):
  - `pub decision_rows: bool` = true (entries evaluated at every decision row while HOT; false = Promote rows only, V5 behaviour);
  - `pub reentry_cooldown_s: i64` = 300.
- `let mut last_exit: FxHashMap<u32, i64>`, set when a position closes in the Sell fill branch.
- Line 487 becomes:
```rust
let entry_row = if st.decision_rows { step.decision && step.radar_state == radar::RadarState::Hot } else { step.signal };
if !entry_row || e.time > ev.window.1 { continue; }
```
- After the `blacklisted` check: `if last_exit.get(&e.coin).is_some_and(|t| e.time - t < st.reentry_cooldown_s) { block("reentry_cooldown"); continue; }`.
- WATCH rows never enter here. They exist for the model (training rows) and for the strategy WP's ARM logic.

---------------------------------------------------------------------------------------------------------------------

## 9. Params files

`config/radar_params.json`: **unchanged.**
- It still loads: `#[serde(default)]`, all V4 fields present, new fields take `Params::default()` values, the nested `cusum` takes `CusumParams::default()`.
- Effect: `sniper backtest/snapshots/v5-*` run the **new** radar with the frozen V4 thresholds until `radar-tune` (v6) rewrites the file on dev windows.

NEW `config/radar_params_v4_legacy.json` (reproduction only):
```json
{ "note": "V4 radar reproduction: V4-1 newborn proxy, auto-HOT newborns, wallet counts, no dust/cluster/CUSUM/WATCH. Comparison only.",
  "params": { "demote_idle_s": 300, "lift": 1.15, "lift_min_buyers": 2, "max_buy_sell_ratio": 4.0, "max_median_buy_sol": 1.0,
    "min_accel": 2.0, "min_buy_sol": 1.0, "min_buyers": 4, "real_min_sol": 0.01, "slow_min_buy_sol": 2.0, "slow_min_buyers": 6,
    "slow_win_s": 300, "tau_fast_s": 10.0, "tau_slow_s": 120.0, "win_s": 60,
    "newborn_mode": "legacy_first_seen", "proxy_warmup_s": 60, "newborn_auto_hot": true, "new_max_age_s": null,
    "exclude_creator": false, "count_clusters": false, "old_max_median_buy_sol": 1.0, "old_max_buy_sell_ratio": 4.0,
    "old_flow_rule": false, "watch": false, "decide_every_s": 0, "cusum": { "enabled": false } } }
```

---------------------------------------------------------------------------------------------------------------------

## 10. Tests to write (all run on the VPS: `cargo test --release`, `pytest`)

### crates/radar/src/cusum.rs
1. `median_mad_known_values`: [1,2,3,4,100] → median 3, σ = 1.4826; empty → NaN.
2. `cusum_stays_zero_below_reference`: z = 0.4 forever, k = 0.5 → S == 0 every step (V4 §33.4).
3. `cusum_step_change_alarms_after_expected_n`: z = 2.0, k = 0.5, h = 5 → S = 1.5, 3, 4.5, then alarm on the 4th update; S == 0 after the reset.
4. `robust_baseline_ignores_single_outlier`: 59 samples ≈ 1.0 ± 0.1 plus one 1000 → median/MAD move < 0.05 (mean/std would move > 10).
5. `baseline_excludes_current_step`: estimate before `push(x)` is unchanged by x; z of a spike step uses the pre-spike baseline.
6. `warmup_uses_population_then_own`: pop ready (≥ pop_min samples in bucket 9); coin with < warmup_n own steps → `pop_used == Some(true)`; after warmup_n steps → `Some(false)`; pop not ready and no own → `s() == None` and no alarm.
7. `fixed_step_sampling_counts_idle_steps`: trades at t = 0 and t = 50, step 5 → own baseline len == 10 (1 active + 9 idle); gap of 10,000 s → len capped by max_gap_steps and every S == 0.
8. `sigma_floor_prevents_inf`: constant history (MAD 0) → z finite, uses the floor.
9. `age_bucket_edges`: 0→0, 9→0, 10→1, 29→1, 30→2, 59→2, 60→3, 300→5, 3600→7, 21600→8, None→9.
10. `pop_baseline_refresh_is_deterministic`: two identical push/estimate sequences → identical outputs; estimate cached until `pop_refresh_s` passes.
11. `ewma_baseline_matches_closed_form`: constant x → μ = x, var → 0 (floor applies).
12. `cluster_burst_alarms_on_clusters_channel`: 40 quiet steps (0-1 cluster) then 8 clusters per step → alarm Clusters within ≤ 2 steps; `last_alarm` channel = Clusters.

### crates/radar/src/lib.rs (existing 4 tests kept, updated to `Trade::basic`)
13. `newborn_is_not_auto_hot`: `set_birth(New{0,false})`; first trade → `Some(Watch)`; 3 real buyers → no Promote; Promote only when min_buyers clusters + buy SOL + filters pass.
14. `newborn_strict_filters_block`: new coin with median buy 2.0 > 1.0 → no Promote; with buy/sell 10 > 4 → no Promote.
15. `old_coin_not_blocked_by_median_buy`: 6 buyers × 2 SOL → old coin Promote; the same stream as a newborn → no Promote.
16. `old_coin_buy_sell_ratio_configurable`: buy/sell 6 → `Some(4.0)` blocks, `None` promotes.
17. `dust_kept_tagged_not_real`: 30 dust buys → `view().metrics.dust_n_win == 30`, `clusters_win == 0`, no Promote.
18. `split_buying_one_cluster_triggers_watch`: 10 wallets mapped to cluster 99, 30 buys of 0.009 SOL (split_min_sol 0.2) → `Some(Watch)`; the same buys from 10 distinct clusters → no Watch.
19. `cluster_aggregation_counts_one_trader`: 6 buyers, one cluster → no Promote (min_buyers 5); distinct clusters → Promote.
20. `creator_buys_excluded`: by_creator buys do not count toward clusters_win / buy_sol_win; `exclude_creator=false` → counted.
21. `watch_hot_decide_rate_limited`: real trades every 1 s → Decide only every ≥ decide_every_s; none after decide_until; an alarm extends decide_until.
22. `decide_disabled_gives_promote_only`: `decide_every_s=0`, `watch=false` → only Promote/Demote.
23. `unwatch_after_idle_and_watch_max`: sweep emits Unwatch on idle > demote_idle_s and on no trigger > watch_max_s.
24. `new_path_expires_after_new_max_age`: age > 1800 → kind Old → median block lifted.
25. `cusum_alarm_puts_flat_old_coin_on_watch`: old coin with 30 quiet steps then a cluster burst at flat price → Watch, `view().last_alarm.is_some()`; the same burst after +30% price (watch_max_rise 0.1) → no Watch.
26. `flow_rule_promotes_old_coin_on_nfl_and_growth`: L = 20 SOL, 5 clusters, NFL 0.03, G ≥ 1.5 → Promote; L = 2 SOL < old_min_liq → only legacy rules.
27. `legacy_mode_matches_v4_reference_on_random_stream`:
    - embed the V4 radar (current lib.rs:96-277) as `mod v4_ref`;
    - LCG stream of 30,000 trades over 60 coins, with dust (sol 0.001-2.0) and 200 traders, `newborn_after = 60`;
    - new Radar under the legacy JSON (`include_str!("../../../config/radar_params_v4_legacy.json")`) + `with_newborn_after(60)`;
    - identical `on_trade` outputs per trade, and identical sorted sweep outputs per second.
28. `radar_params_json_still_loads`: `include_str!("../../../config/radar_params.json")["params"]` → V4 fields equal the file values; new fields == `Params::default()` values.
29. `params_json_roundtrip`: `Params::default()` → JSON → Params equal (no ±inf / null breakage); same for the legacy file.

### crates/decoder/src/pump.rs
30. `creations_flag_pump_amm_migration`:
    - load the IDLs with `include_str!("../../../config/idl/pump.json")` / `pump_amm.json`;
    - build a synthetic `TxView` (pub fields) with emit_cpi exec items, `EVENT_IX_TAG` + discriminator (read from the IDL by name) + borsh fields;
    - tx A: CreateEvent → `migration == false`;
    - tx B: pump CompletePumpAmmMigrationEvent(mint M) + pump_amm CreatePoolEvent(base_mint M) → `migration == true`;
    - tx C: CreatePoolEvent alone → `false`.

### crates/app (bin crate module tests)
31. `radar_feed::tests::births_authoritative_from_window_scan`: scanned; created in-window → New{proxy:false}; pre-window follow creation → Old; none → Old.
32. `radar_feed::tests::births_proxy_rule`: not scanned: after warm-up on curve with progress 1% → New{proxy:true}; AMM first → Old; progress 10% → Old; within warm-up → Old; follow `created` present → still decided by proxy (no leak).
33. `radar_feed::tests::births_legacy_first_seen`.
34. `radar_feed::tests::trade_builder_active_pool_and_liquidity`: curve `liq = real_quote`, AMM `liq = quote_res`; a curve event after the AMM does not overwrite liq/mprice.
35. `radar_feed::tests::cluster_lookup_and_creator_family`: lookup maps trader → cluster; by_creator for a creator-cluster member.
36. `radar_feed::tests::load_params_defaults_missing_fields`: temp file with only V4 fields loads.
37. `backtest::tests::creations_sidecar_roundtrip_and_attach`:
    - temp dir: write `pump_events_v2.bin` (bincode `WindowCache`) + `creations.bin.zst`;
    - `load_window` → `created` set for non-migration items, `creations_scanned == true`;
    - wrong window → false; migration item ignored.
38. `follow::tests::merge_keeps_window_scan_creation`: window-scan created is kept when follow has None or another value; still filled when the window has None (the existing test keeps passing).
39. `ml_sim::tests::features_do_not_see_the_future` (kept): Context literal → `..Default::default()`; also pass `clusters: Some(..)` (every 4th wallet in a 2-wallet cluster), so cluster/dust/CUSUM/WATCH/Decide code is covered by the prefix test.
40. `ml_sim::tests::features_do_not_see_the_future_with_creations`: same stream with `creations_scanned = true` and coin 1 created in-window → new-coin path covered; prefix == full for all features.
41. `ml_sim::tests::decision_rows_repeat_after_hot`: old coin promoted once (`signal` once), then ≥ 3 `decision` rows ≥ decide_every_s apart; with `decide_every_s=0` → decision == signal.
42. `ml_sim::tests::watch_rows_do_not_leak_follow_creation`: not scanned, `ev.created[0] = Some(pre-window)`; WATCH row before HOT → `created_known == 0`, `age_s` from first trade; after HOT → `created_known == 1`.
43. `ml_sim::tests::feature_block_appended`: `FEATURES[0]=="age_s"`, `FEATURES[72]=="created_known"`, `FEATURES[106]=="top10_supply_share"`; rd_* names contiguous in the §7.5 order.
44. `strategy_v5::tests` `ladder_on_a_runner`, `ev_gate_blocks`: unchanged assertions. They pass because Events::new → proxy → coin first seen at t = 0 → old path; the Watch row is skipped; the Promote row enters. Add `decision_rows_reentry_cooldown`: a runner exited by Stop is not re-entered within reentry_cooldown_s.
45. `radar_eval::tests::from_events_births_and_cross10`: tiny Events → Dataset trades carry liq/mprice; births follow BirthTable; cross10 label on a hand-made path.

### Python (offline)
46. `python/ml/tests/test_radar_feature_block.py`:
    - `rust_str_array("FEATURES")[:107]` anchors as in test 43;
    - D2 names present in order;
    - `feature_columns(header)` returns them;
    - no `y_` names among features.
47. Optional, for the Stage 1D Python reference: `python/research/cusum_ref.py` (numpy, same algorithm) + `python/research/tests/test_cusum_ref.py` with the same hand vectors as tests 2, 3, 7 and 8.

---------------------------------------------------------------------------------------------------------------------

## 11. Data requirements, degradation, VPS commands

| Component | Needs | On `datasets/bf_1h`, `h1d` (V4 cache, no raw) | How the VPS produces it |
|---|---|---|---|
| Authoritative newborn | `creations.bin.zst` (window-wide CreateEvent/CreatePoolEvent scan) | **absent** → `birth_source = proxy`, labelled in radar-eval JSON, `snapshots_v5_stats.json`, `follow_summary.json`, feature `rd_birth_src = 1` | automatic in the raw decode (`load_window` raw branch, run by `export-events` in `scripts/v5_fetch.sh`); for decoded windows that still have raw: `sniper scan-creations --rec data/v5_X`; raw deleted → cannot be recovered (stays proxy) |
| Cluster aggregation | `wallet_scores.csv` with `cluster_id` | absent → cluster = wallet, `rd_clusters_known = 0` | `python3 python/research/wallet_scores.py data/v5_*` (already writes `cluster_id`; needs history before each window) |
| NFL / liquidity | `Ev.real_quote` (curve) / `quote_res` (AMM) | available | – |
| Radar eval | events or raw | events (new `from_events`) | `zstd -d datasets/bf_1h/pump_events.bin.zst -o data/bf_1h/pump_events.bin` then `sniper radar-eval --rec data/bf_1h --source events` |
| Raw-source radar eval | raw recording | not possible | raw kept |
| CUSUM pop baseline | ≥ 64 samples per bucket | builds within the first minutes | – |

Proxy validation number for the VPS (no tuning involved): `radar-eval --source events --params config/radar_params.json` on `data/bf_1h` and `data/h1d` should report `newborn_coins ≈ 1,216` and `≈ 1,829`. These are the forensic §2.2 "truly new" counts with the same rule. A difference means a proxy implementation bug or an edge case (equal-second ordering).

Phase-2 measurement (file 01 §10, phase 2):
1. `cargo test --release` and `scripts/ci.sh`.
2. `sniper radar-eval` on every available window with `config/radar_params_v4_legacy.json` (before) and `config/radar_params.json` (after). Report `hot_per_min`, `watch_per_min`, recall of ≥5x/≥10x runners HOT/armed before +10% and +30%, precision vs naive, old vs new separately, and the CUSUM delay/ARL, with n for every rate.
3. Only then `sniper radar-tune --rec <dev windows>` (v6 grid, 16 points, median rule). With only 2 V4 windows this is "direction only".

---------------------------------------------------------------------------------------------------------------------

## 12. Deviations from the docs (with reasons)

1. ARM and ENTRY are **not** radar states. ARM needs the Phase-3 safety gates; ENTRY needs the strategy price guard P5. The radar exposes WATCH/HOT plus decision rows, and the strategy WP maps HOT + safety → ARM → ENTRY.
2. The new-coin path is limited to `new_max_age_s` = 1800 s (V4 spec §8.1). File 01 only says "new coin (from create event)"; without a limit, a 10-hour-old coin would keep the strict newborn filters.
3. Authoritative births use only in-window creations (unselected). Follow-up creation times are selected by HOT status, so using them would leak. Coins created < 30 min before the window start are therefore "old" (boundary effect, reported).
4. `is_signal` now means "decision row" (V5 plan §3.3) instead of "Promote only". The legacy params reproduce the old column exactly.
5. Existing feature *definitions* change at unchanged indices: `newborn`(1), `age_s`(0), `mult_since_first`(35), `created_known`(72), `early20_delay_med_s`, `new_coins_per_min`. This is required once rows exist before HOT (leak). Old snapshot files are not comparable and must be regenerated.
6. CUSUM alarms are evaluated lazily at the coin's first trade after a step boundary, not by a timer. Detection delay is ≤ step_s + inter-trade gap. Decisions happen at trades anyway.
7. The population baseline includes the coin's own samples (not strictly "other coins"); this is negligible, see §3.3.
8. The median-buy filter stays per-trade (as measured in file 01 §2.4), not per cluster. Cluster-aggregated sizes are exposed as features instead.
9. AMM liquidity = decoded PumpSwap quote reserve, which includes `virtual_quote_reserves` when present; `Ev` does not split real and virtual for the AMM.
10. A PumpSwap `CreatePoolEvent` without a migration in the same tx counts as a newborn (pool launch, V4 §8.1). A second pool created for an existing old mint would be misclassified; this is rare and not measurable on current data.
11. The radar grid is reduced from 224 to 16 points with a median rule, per file 01 §9.3. The old grid stays behind `--legacy`.
12. There is no dust field in `Trade` (derived from `real_min_sol` in one place).


## Files touched

- `crates/radar/src/lib.rs` — Trade new fields + Trade::basic; Birth/CoinKind/RadarState/NewbornMode/BirthSource enums; Signal += Watch/Unwatch/Decide; Params #[serde(default)] + 28 new fields (slow_min_buy_sol default INFINITY->1e18); Radar: set_birth, view, state, birth, n_alarms, watch_now/max, deprecated with_newborn_after shim; per-coin deque of ALL trades (dust tagged), Metrics single code path, new/old path rules, WATCH triggers, Decide rows, sweep Unwatch; tests 13-29 incl. embedded V4 reference
- `crates/radar/src/cusum.rs` — NEW: CusumParams, BaselineKind, Channel, median_mad, age_bucket, RobustBaseline, Cusum, PopBaseline, CoinCusum (fixed 5 s steps, robust causal baseline, warm-up + population baseline, k/h, reset); tests 1-12
- `crates/radar/Cargo.toml` — [dev-dependencies] serde_json.workspace = true (params-loadability tests)
- `crates/decoder/src/pump.rs` — pub const CURVE_INITIAL_REAL_TOKEN; Creation.migration (CreatePoolEvent in a tx with CompletePumpAmmMigrationEvent for the same mint); test 30
- `crates/app/src/radar_feed.rs` — NEW: TradeBuilder (only Ev->radar::Trade path: liq real reserve, mprice, amm, cluster, by_creator), BirthTable (authoritative / proxy / legacy births, selection-leak-free), RadarFeed (sweep+birth+trade+radar), load_params(); tests 31-36
- `crates/app/src/backtest.rs` — Events.creations_scanned; CREATIONS_FILE sidecar (CreationRec/CreationsFile, read/write/attach); load_window decodes creations in the raw branch and attaches the sidecar in all branches; scan_creations(); simulate() uses RadarFeed (remove own Radar/sweep/Trade); test 37
- `crates/app/src/ml_sim.rs` — Engine uses RadarFeed; Step.decision + Step.radar_state; snapshots from first WATCH/HOT (track_time); leak-free created_for_features; newborn/new_coins from radar births; FEATURES += 23 rd_* names (literal N); features_rd(); Context.clusters + load_context cluster_id; write_snapshots is_signal=decision + SnapStats fields; simulate entry gated on ever_hot; tests 39-43
- `crates/app/src/follow.rs` — hot_coins(ev, rp, min_state) via RadarFeed; Opts.min_state; merge keeps window-scan created (.or); decode_creations filter !migration; summary birth_source/creations_scanned; test 38
- `crates/app/src/radar_eval.rs` — Dataset births/birth_source + new Trade fields; raw load also decodes creations; from_events(); load_auto(); run_radar with set_birth, armed (Watch/Hot) intervals, alarms; cross10 label; HOT/armed recall before +10%/+30%, hot/watch per min, CusumEval (delay, ARL); tune_v6 (16-point grid, median rule, ablation); old tune kept for --legacy; test 45
- `crates/app/src/strategy_v5.rs` — StratV5 #[serde(default)] + decision_rows (default true) + reentry_cooldown_s (300); entry at decision rows while HOT (line 487); last_exit map + reentry_cooldown block; new test decision_rows_reentry_cooldown
- `crates/app/src/v5_eval.rs` — line 394: Engine::new(..).with_ctx(d.ctx.clone()) so the first-signal table uses the same radar input as the strategy run
- `crates/app/src/diagnose.rs` — line 21: radar_feed::load_params
- `crates/app/src/main.rs` — mod radar_feed; 10 params-load sites -> radar_feed::load_params; new ScanCreations cmd; Follow --min-state; RadarEval --source; RadarTune --source/--legacy/--base-params/--max-hot-per-min
- `config/radar_params_v4_legacy.json` — NEW: explicit V4 reproduction params (legacy_first_seen, auto-HOT newborns, no watch/decide/cusum/flow, wallet counts)
- `scripts/v5_fetch.sh` — run scan-creations if creations.bin.zst is missing, BEFORE rm -f raw-*.zst
- `scripts/v5_save_datasets.sh` — version creations.bin.zst with the window
- `python/ml/tests/test_radar_feature_block.py` — NEW pytest: first 107 FEATURES anchors unchanged, rd_* block appended in order, feature_columns picks them up
- `python/research/cusum_ref.py` — OPTIONAL: offline numpy reference of the robust CUSUM (Stage 1D parity)
- `python/research/tests/test_cusum_ref.py` — OPTIONAL: same hand vectors as Rust cusum tests 2,3,7,8

## Interfaces

RADAR CRATE (crates/radar):
- `pub struct Trade { time: i64, order: u64, coin: u32, trader: u32, cluster: u32, buy: bool, sol: f64, price: f64, mprice: f64, liq_sol: f64, amm: bool, by_creator: bool }`
- `Trade::basic(time, order, coin, trader, buy, sol, price) -> Trade`
- `pub enum Birth { New { t: i64, proxy: bool }, Old }` with `is_new()` and `code()` (0 old / 1 proxy / 2 on-chain)
- `pub enum CoinKind { New, Old }`; `pub enum RadarState { Cold, Watch, Hot }`
- `pub enum Signal { Promote, Demote, Watch, Unwatch, Decide }`
- `pub enum NewbornMode { Auto, Authoritative, Proxy, LegacyFirstSeen }` (serde snake_case); `pub enum BirthSource { Authoritative, Proxy, LegacyFirstSeen }`
- `Radar` methods:
  - `new(Params)`, `set_birth(coin, Birth)`, `sweep(now, &mut Vec<(u32, Signal)>)`, `on_trade(&Trade) -> Option<Signal>` (priority Promote > Watch > Decide)
  - `is_newborn`, `birth`, `state`, `n_alarms`, `view(coin, now) -> Option<CoinView>`, `params()`
  - pub fields `hot_now`, `hot_max`, `watch_now`, `watch_max`
  - `with_newborn_after` is kept but deprecated
- `pub struct Metrics { clusters_win, clusters_prev_win, buy_sol_win, sell_sol_win, median_buy_win, growth, rise_from_low_win: Option<f64>, dust_n_win, dust_sol_win, dust_top_cluster_share_win: Option<f64>, split_max_sol_300, split_max_n_300, liq_sol: Option<f64>, nfl_win: Option<f64> }`
- `pub struct CoinView { state, kind, birth, state_since, decisions_in_state, cusum_s: [Option<f64>; 3], cusum_pop: [Option<bool>; 3], last_alarm: Option<(i64, Channel)>, n_alarms, metrics }`

PARAMS:
- `#[serde(default)]` on `Params`. The V4 fields are unchanged.
- New fields with defaults: newborn_mode=auto, proxy_warmup_s=60, proxy_max_progress=0.05, newborn_auto_hot=false, new_max_age_s=Some(1800), exclude_creator=true, count_clusters=true, old_max_median_buy_sol=None, old_max_buy_sell_ratio=Some(4.0), old_flow_rule=true, old_min_clusters=5, old_min_nfl_60=0.01, old_min_liq_sol=5.0, old_min_growth=1.5, liq_floor_sol=1.0, watch=true, watch_min_clusters=3, watch_min_growth=2.0, watch_max_rise=Some(0.10), watch_max_s=1800, split_min_buys=5, split_min_sol=0.3, split_min_liq_frac=0.005, split_max_avg_sol=0.05, decide_every_s=5, decide_in_watch=true, decide_max_age_s=1800, cusum=CusumParams.
- `CusumParams` (`#[serde(default)]`): enabled=true, step_s=5, k=0.5, h=5.0, base_n=60, warmup_n=12, baseline=median_mad, ewma_tau_steps=60, pop_min=64, pop_cap=2048, pop_refresh_s=5, max_gap_steps=60, hold_s=60, bsol_unit=0.1, floor_uc=1.0, floor_bsol=0.5, floor_nfl=0.002.
- Default change: `slow_min_buy_sol` goes from INFINITY to 1e18 (JSON round-trip).

CUSUM MODULE (crates/radar/src/cusum.rs):
- `median_mad(&mut [f64]) -> (f64, f64)`; `age_bucket(Option<i64>) -> usize` (10 buckets)
- `RobustBaseline`; `Cusum::update(z, k, h) -> bool`
- `PopBaseline::{push, estimate}`
- `CoinCusum::{new, advance(t, &CusumParams, &mut PopBaseline, birth_t: Option<i64>, liq_floored: Option<f64>) -> Option<Channel>, add(who, buy, sol), s(ch), pop_used(ch)}`
- `Channel { Clusters, BuySol, NetFlowLiq }`

DECODER:
- `decoder::pump::Creation { venue, mint, time, price, migration: bool }`
- `decoder::pump::CURVE_INITIAL_REAL_TOKEN: f64 = 793.1e12`

APP RADAR FEED (crates/app/src/radar_feed.rs):
- `TradeBuilder::{new(n_coins, Option<Arc<[u32]>>), cluster_of(u32) -> u32, build(&Ev) -> (Trade, bool)}`
- `BirthTable::{new(&Events, &Params), birth(coin, first: &Ev) -> Birth}` with `pub source: BirthSource`
- `RadarFeed::{new(&Events, Params), set_clusters(Option<Arc<[u32]>>), radar() -> &Radar, births() -> &BirthTable, birth_source(), step(&Ev, &mut Vec<(u32, Signal)>) -> Option<Signal>}`
- `load_params(&Path) -> anyhow::Result<radar::Params>`

EVENTS AND CREATION DATA (backtest.rs):
- `Events.creations_scanned: bool`
- `const CREATIONS_FILE = "creations.bin.zst"` (bincode+zstd): `CreationsFile { version: u32 = 1, window: (i64, i64), first_slot: u64, last_slot: u64, complete: bool, items: Vec<CreationRec { mint: String, slot: u64, order: u64, time: i64, amm: bool, migration: bool, price: f64 }> }`
- `read_creations(&Path)`, `scan_creations(&Path) -> Result<CreationsFile>`
- `load_window` writes and attaches the sidecar
- `follow::merge` keeps window-scan `created`

ENGINE (ml_sim.rs):
- `Step { snapshot, signal (Promote), decision (Promote|Watch|Decide), radar_state: RadarState, dev_sell, dev_sell_frac }`
- `Engine::{radar() -> &Radar, ever_hot(coin) -> bool}`
- `Context { wallets, sol_usd, clusters: Option<Arc<[u32]>> }`; `load_context` reads the optional `cluster_id` column of `wallet_scores.csv`

SNAPSHOT CSV:
- The header is unchanged.
- `is_signal` = decision row (Promote + Watch + Decide).
- `snapshots_v5_stats.json` gains promote_rows, watch_rows, newborn_coins, birth_source, creations_scanned, clusters_known.

FEATURES APPENDED (one contiguous block, literal array length, in this order):
rd_state, rd_kind_new, rd_birth_src, rd_secs_in_state, rd_decisions_in_state, rd_cusum_uc, rd_cusum_bsol, rd_cusum_nfl, rd_cusum_alarm_age_s, rd_cusum_alarms, rd_cusum_pop, rd_clusters_60s, rd_cluster_growth, rd_cluster_accel, rd_nfl_60s, rd_liq_real_sol, rd_dust_n_60s, rd_dust_sol_60s, rd_dust_sol_share_60s, rd_dust_top_cluster_share_60s, rd_split_max_sol_liq_300s, rd_split_max_n_300s, rd_clusters_known.
Existing indices are unchanged. Definitions corrected at the same index: newborn(1), age_s(0), mult_since_first(35), created_known(72), early20_delay_med_s, new_coins_per_min.

STRATEGY HOOK:
- `StratV5` gets `#[serde(default)]`, `decision_rows: bool = true`, `reentry_cooldown_s: i64 = 300`.
- Entry happens at `step.decision && step.radar_state == Hot` (or `step.signal` when decision_rows is false).

CLI:
- `sniper scan-creations --rec DIR`
- `sniper follow --min-state hot|watch` (default hot)
- `sniper radar-eval --source auto|events|raw`
- `sniper radar-tune --source ... --legacy --base-params config/radar_params.json --max-hot-per-min 30`

RADAR-EVAL JSON additions:
- birth_source, hot_per_min, watch_per_min, newborn_coins (births)
- per tier: caught/recall_hot_before_10, caught/recall_armed_before_10, caught_armed_before_30, direction_only
- cusum: {runners_5x, alarmed_before_10, median_delay_s, nonrunner_coin_hours, nonrunner_alarms, alarms_per_coin_hour}

CONFIG FILES:
- `config/radar_params.json` is unchanged and loadable.
- New `config/radar_params_v4_legacy.json`.

## Depends on

- Strategy WP (V5-1 ranking per decision tick, P5 price guard, ARM = HOT + safety, Momentum Fade): consumes Step.decision / Step.radar_state; if it rewrites strategy_v5 entry loop it must keep StratV5.decision_rows + reentry_cooldown_s semantics (or supersede them explicitly)
- Core-metrics WP (F2/F3/C1/C2/C11 features): coordinate so rd_clusters_60s / rd_cluster_growth / rd_cluster_accel / rd_nfl_60s are not appended twice; orchestrator fixes the final FEATURES block order (all blocks appended after index 106, literal array length)
- Labels/model WP: is_signal rows now include WATCH and Decide rows (V5 plan 3.3); entry model may additionally filter rd_state == 2; V5-10 in_window filter remains theirs; v5_train.py needs no change for D2
- Safety WP (dev family, rug, honeypot): can reuse Context.clusters, Trade.by_creator and radar_feed::TradeBuilder::cluster_of for creator-family aggregation
- Bug-fix WP V4-4/V5-13 (real liquidity): shares the liquidity definition used here (curve real_sol_reserves, AMM pool quote reserve) via TradeBuilder
- Data WP / VPS pipeline: produce creations.bin.zst during the raw decode (load_window / scan-creations) before raw deletion; run wallet_scores.py to provide cluster_id

## Verified bugs

| id | confirmed | location | note |
|---|---|---|---|
| V4-1 | True | crates/radar/src/lib.rs:155,180-186; crates/app/src/ml_sim.rs:252; backtest.rs:358; follow.rs:168; radar_eval.rs:180 | Newborn = first seen after window.0 + 60 (incl. dust first trade) and promoted HOT on the first real buy with no checks. On this branch the backtest line is 358 (file 01 says 303 for main). |
| V5-3 | True | crates/app/src/ml_sim.rs:252 | Engine radar is still Radar::new(rp).with_newborn_after(ev.window.0 + 60); follow::hot_coins (follow.rs:168) uses the same setup, so follow selection inherits V4-1. |
| V5-2 | True | crates/app/src/ml_sim.rs:510,514; strategy_v5.rs:487 | The forced snapshot / is_signal happens only on Promote; strategy_v5 enters only if step.signal, so there is one decision per HOT episode. |
| DUST-DROP | True | crates/radar/src/lib.rs:158 | Trades with sol < real_min_sol return None before any window update: dust is not kept or aggregated (file 01 5.3). |
| OLD-FILTERS | True | crates/radar/src/lib.rs:211,215 | buy/sell > max_buy_sell_ratio and median buy > max_median_buy_sol block every non-newborn coin (old-coin awakening) in the radar itself. |
| D2-N1 follow-creation selection leak (latent) | True | crates/app/src/ml_sim.rs:663-669, 987 (created_known) | ev.created exists only for followed (= HOT) coins. Harmless today because snapshot rows exist only after HOT, but any pre-HOT (WATCH) row or any birth decision using it would leak future HOT status. The design uses it only after HOT or from an unselected window scan. |
| D2-N2 merge overwrites creation | True | crates/app/src/follow.rs:124 | ev.created[ci] = c.created would clobber window-scan creation data with None when the follow history scan was not exhausted; changed to .or(). |
| D2-N3 migration counted as creation | True | crates/decoder/src/pump.rs:222-224 | decode_creations returns PumpSwap CreatePoolEvent, which the curve migration also emits (with pump CompletePumpAmmMigrationEvent in the same tx, per config/idl). A window-wide scan would mark every migrated old coin as a newborn. Harmless in follow today (mint's first signature); fixed with Creation.migration. |
| D2-N4 Params infinity default not JSON round-trippable | True | crates/radar/src/lib.rs:66 | slow_min_buy_sol = f64::INFINITY serializes as null in serde_json and fails to deserialize; config/radar_params.json has finite values, so only Default-based dumps are affected. |
| D2-N5 radar_eval needs raw recordings | True | crates/app/src/radar_eval.rs:28-70 | Radar eval decodes raw getBlock JSON (balance_delta) only, so it cannot run on datasets/bf_1h or h1d (no raw). The phase-2 radar eval on available data needs from_events. |
| D2-N6 regime new_coins uses V4-1 proxy | True | crates/app/src/ml_sim.rs:366 | new_coins_per_min counts coins first seen after window.0 + 60 (61-65% mislabeled old coins per file 01 2.2). |
| D2-N7 compare uses Engine without ctx | True | crates/app/src/v5_eval.rs:394 | The first-signal table builds Engine without load_context while the strategy run uses it. Latent today; would diverge once a cluster lookup feeds the radar. |

## Open questions

- HOT budget for radar-tune v6: --max-hot-per-min default 30 is a placeholder (old radar produced 108-130 newborn HOT/min); the user should fix the cap before results are seen (anti-overfitting rule)
- Should the entry model train on all decision rows (WATCH + HOT) or only HOT rows (rd_state == 2)? Design keeps is_signal = all decision rows; strategy only enters on HOT rows
- Follow-up cost: following WATCH coins (--min-state watch) gives uncensored labels for pre-HOT rows but costs more Helius credits; default stays hot (WATCH-only coins' rows are censored unless they die in-window)
- StratV5.decision_rows default true (V5-2 fix on) vs false (reproduce V5 numbers): design defaults to true and keeps false for A/B; confirm
- new_max_age_s = 1800 s for the new-coin path (V4 spec 8.1) is a choice, not measured; alternative: until migration + N min
- Authoritative mode treats coins created shortly before the window start as old (selection-free); acceptable boundary effect or exclude the first 30 min of each window from new/old comparisons?
- A PumpSwap CreatePoolEvent without migration counts as a newborn (pool launch); a new pool for an existing old mint would be misclassified (rare, not measurable on current data)
- Existing V5 windows whose raw JSON was already deleted cannot get creations.bin.zst without re-backfill; they stay in proxy mode (labelled). Re-backfill or accept?
- CUSUM default h = 5.0 (spec 4-5): with ~2,000 active coins and 5 s steps even an in-control ARL of ~465 steps implies many WATCH alarms per minute; the eval reports ARL so h can be chosen on dev windows
