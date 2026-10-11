//! EXPLORATORY ML pipeline (user-requested before the Stage 1–5 gates): causal feature snapshots,
//! forward labels, and a simulator that trades on model scores.
//!
//! One code path computes features for both the training snapshots and the simulator, on the same
//! cadence: an event of at least REAL_MIN_SOL on a coin the radar has flagged HOT, at most once per
//! `snap_every(age since HOT)` per coin (2 s for the first 10 min, 10 s until 1 h, then 30 s), plus a forced
//! snapshot on every radar Promote (the decision rows, `is_signal`). Snapshot key = event order.
//! Labels (V5 §3) use the marginal pool price path (quote/token reserves) from the first event >= t + 1 s
//! (latency) forward over H = 10 h, on window + follow-up data; they are never used as features.

use crate::backtest::{Costs, Events};
use decoder::pump::{buy_tokens_out, sell_quote_out};
use radar::{Params as RadarParams, Radar, Signal, Trade};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::Write;

const REAL_MIN_SOL: f64 = 0.01;
/// Snapshot spacing by time since the coin first became HOT.
fn snap_every(since_hot: i64) -> i64 {
    if since_hot < 600 {
        2
    } else if since_hot < 3600 {
        10
    } else {
        30
    }
}
/// V5 §3: label horizon (= max_hold_s) and label latency.
pub const H_S: i64 = 36_000;
const LABEL_LAT_S: i64 = 1;
/// Smart wallet: shrunk mean log return per closed trip above this (non-bot). Sniper: >= this many newborn snipes.
const SMART_THETA: f32 = 0.1;
const SNIPER_MIN_COINS: u32 = 3;
const TAU_FAST: f64 = 10.0;
const TAU_SLOW: f64 = 120.0;
/// Hawkes kernel decay per slot: fast ~ 10 slots, slow ~ 100 slots.
const HK_FAST: f64 = 0.1;
const HK_SLOW: f64 = 0.01;
/// pump.fun total token supply (raw, 6 decimals): 1B tokens.
const TOTAL_SUPPLY_RAW: f64 = 1_000_000_000_000_000.0;
/// pump.fun initial real token reserves (raw, 6 decimals): 793.1M tokens.
const CURVE_INITIAL_REAL_TOKEN: f64 = 793_100_000_000_000.0;

pub const FEATURES: [&str; 107] = [
    "age_s", "newborn", "on_amm", "quote_res_sol", "curve_progress", "fee_bps",
    "n_trades_15s", "n_trades_60s", "n_trades_300s", "buyers_15s", "buyers_60s", "sellers_60s",
    "buy_sol_15s", "buy_sol_60s", "sell_sol_15s", "sell_sol_60s", "net_flow_15s", "net_flow_60s", "flow_imb_60s",
    "median_buy_60s", "max_buy_60s", "dust_share_60s", "repeat_buyer_share_60s", "new_wallet_share_60s",
    "top_buyer_share_60s", "ewma_fast", "ewma_slow", "accel", "secs_since_real_buy",
    "ret_5s", "ret_15s", "ret_60s", "ret_300s", "price_vs_high300", "price_vs_low300", "mult_since_first",
    "price_vs_vwap60", "creator_sold", "creator_sell_sol", "creator_buy_sol", "secs_since_hot", "sellers_15s",
    "buy_sell_count_ratio_60s",
    // Candidate groups from the literature review (kept only if they pass the OOS ablation):
    // F1 curve fill speed (arXiv 2602.14860)
    "f1_sol_in_curve", "f1_sol_per_trade", "f1_trades_since_first",
    // F2 bundles + early-buyer concentration (arXiv 2602.13480)
    "f2_bundle_buy_share", "f2_bundle_txs_60s", "f2_early20_supply_share", "f2_top10_holder_share",
    // F4 wash trades + multi-sell transactions (arXiv 2609.10246)
    "f4_wash_share_60s", "f4_multi_sell_txs_60s",
    // F6 holder count dynamics (arXiv 2603.24625)
    "f6_holders", "f6_holders_vs_max",
    // F7 same-slot buy bursts (La Morgia et al.)
    "f7_max_slot_buyers_60s", "f7_slot_buy_std_60s",
    // F8 trade arrival (V4 §11.6) + Hawkes self-exciting intensity (V4 §28 research candidate), slot units
    "f8_dt_median20", "f8_dt_p90_20", "f8_dt_cv20", "f8_last_dt", "f8_cluster_share20",
    "f8_hawkes_fast", "f8_hawkes_slow", "f8_hawkes_ratio", "f8_hawkes_buy_sell_ratio",
    // DEV exposure: creator's share of tokens held by observed wallets, net SOL in, ever sold
    "dev_hold_share", "dev_net_sol", "dev_ever_sold",
    // Microstructure for exits: sell pressure in the last 5 s
    "sellers_5s", "sell_sol_5s", "buy_sol_5s", "max_sell_15s",
    // ---- V5 §4 (appended; indices above are unchanged) ----
    "created_known", "log_quote_res",
    // smart wallets (offline wallet_scores.csv, built only from trips closed before the window start)
    "smart_buy_q_60s", "smart_share_60s", "n_smart_buyers_300s", "flipper_share_60s", "sniper_share_held", "cluster_buy_share_60s",
    // crowd growth
    "buyer_growth", "holder_vel_60s", "hawkes_branching",
    // size distribution
    "hhi_buy_60s", "retail_share_60s", "whale_buys_60s", "median_sell_60s", "p90_buy_60s", "p90_sell_60s", "buy_q_60s", "sell_q_60s",
    // order flow (SOL amounts as ratios to the quote reserve)
    "ofi_60s", "cvd_300s", "kyle_lambda_60s", "vol_60s",
    // overhang
    "early_retention", "profit_overhang",
    // curve
    "curve_speed", "eta_migration_s",
    // regime (market-wide; -1 after the window, where only followed coins have data)
    "attention_share_5m", "new_coins_per_min", "market_vol_10m", "sol_ret_1h",
    // trader speed
    "fast_wallet_share_60s", "wallet_rate_med_60s", "early20_delay_med_s",
    // concentration vs TOTAL supply (f2_top10_holder_share is vs observed holdings, ~1 at the first signal)
    "top10_supply_share",
];

#[derive(Default, Clone, Copy)]
struct Pool {
    quote_res: u128,
    token_res: u128,
    real_quote: u64,
    real_token: u64,
    fee_bps: u64,
    amm: bool,
    complete: bool,
    known: bool,
}

impl Pool {
    /// SOL that can actually be withdrawn by selling (V4-4 / V5-13): the real quote reserve on the bonding curve
    /// (the virtual ~30 SOL is not withdrawable), the pool quote reserve on the AMM.
    fn exit_liquidity_sol(&self) -> f64 {
        (if self.amm { self.quote_res as f64 } else { self.real_quote as f64 }) / 1e9
    }

    fn price(&self) -> f64 {
        if self.token_res == 0 {
            0.0
        } else {
            self.quote_res as f64 / self.token_res as f64
        }
    }
}

#[derive(Default)]
struct Coin {
    first_time: i64,
    first_price: f64,
    /// (time, user, buy, sol) of every event within 300 s.
    tr: VecDeque<(i64, u32, bool, f64)>,
    /// (time, marginal price) within 300 s.
    px: VecDeque<(i64, f64)>,
    fast: (f64, f64),
    slow: (f64, f64),
    last_real_buy: i64,
    hot_time: Option<i64>,
    last_event: i64,
    last_snap: i64,
    creator_sell_sol: f64,
    creator_buy_sol: f64,
    dev_sell_time: Option<i64>,
    on_amm: bool,
    n_trades: u64,
    // F2/F6: token balances per wallet (observed trades only), early buyers, bundle accounting.
    bal: FxHashMap<u32, i64>,
    holders: i64,
    holders_max: i64,
    early: Vec<u32>,
    buy_sol_total: f64,
    bundle_buy_sol: f64,
    // Current transaction grouping: (tx key, buyers in it, buy SOL in it, sellers in it, users who bought).
    cur_tx: u64,
    cur_tx_buyers: Vec<u32>,
    cur_tx_buy_sol: f64,
    cur_tx_sellers: Vec<u32>,
    /// (time, kind) markers: 1 = bundle tx (>= 2 distinct buyers), 2 = wash (same user buy+sell in tx), 3 = multi-sell tx.
    tx_marks: VecDeque<(i64, u8)>,
    /// (time, slot, distinct buyers in that slot) for the last 60 s.
    slot_buys: VecDeque<(i64, u64, u32)>,
    cur_slot: u64,
    cur_slot_buyers: Vec<u32>,
    // F8: last 21 trade slots; Hawkes intensities (all / buys / sells) with exponential kernels.
    trade_slots: VecDeque<u64>,
    hk_fast: (f64, u64),
    hk_slow: (f64, u64),
    hk_buy: (f64, u64),
    hk_sell: (f64, u64),
    creator: Option<u32>,
    // V5 state
    holders_hist: VecDeque<(i64, i64)>,
    /// Slots of the last 100 real buys (Hawkes branching ratio).
    buy_slots: VecDeque<u64>,
    /// (time, curve progress) within 60 s.
    prog_hist: VecDeque<(i64, f64)>,
    /// (time, d ln P, signed SOL) per price-path event within 60 s.
    flow: VecDeque<(i64, f64, f64)>,
    last_price: f64,
    /// Tokens bought by each of the first 20 buyers (all their buys).
    early_bought: FxHashMap<u32, i64>,
    /// First buy time of each of the first 20 buyers.
    early_first: Vec<i64>,
    /// Cost basis (lamports) of each wallet's current balance (average-cost method).
    cost: FxHashMap<u32, f64>,
}

fn ewma(e: &mut (f64, f64), t: f64, tau: f64) {
    e.0 = e.0 * (-(t - e.1) / tau).exp() + 1.0 / tau;
    e.1 = t;
}

fn ewma_at(e: &(f64, f64), t: f64, tau: f64) -> f64 {
    e.0 * (-(t - e.1) / tau).exp()
}

/// Offline wallet information for one window (`wallet_scores.csv`).
#[derive(Debug, Clone, Copy, Default)]
pub struct WalletInfo {
    pub score: f32,
    pub n_closed: u32,
    pub is_bot: bool,
    pub is_flipper: bool,
    pub sniper_coins: u32,
    pub cluster_size: u32,
}

/// Optional per-window context: wallet scores (by user id) and the SOL/USD 1-minute closes (close time, price;
/// optional local file, never fetched).
#[derive(Default, Clone)]
pub struct Context {
    pub wallets: Vec<Option<WalletInfo>>,
    pub sol_usd: Vec<(i64, f64)>,
}

/// Live, causal per-wallet activity across all coins (trader speed).
#[derive(Default, Clone, Copy)]
struct WalletLive {
    first_t: i64,
    last_t: i64,
    n: u32,
    gap_ewma: f64,
}

/// Shared causal state: pools, per-coin windows, radar, wallet first-seen.
pub struct Engine {
    pools: Vec<Pool>,
    coins: Vec<Coin>,
    radar: Radar,
    first_seen: FxHashMap<u32, i64>,
    window_start: i64,
    window_end: i64,
    last_sweep: i64,
    sig: Vec<(u32, Signal)>,
    created: Vec<Option<(i64, Option<f64>)>>,
    ctx: Context,
    live: FxHashMap<u32, WalletLive>,
    /// All-coin (time, SOL) within 300 s / 600 s and running sums.
    mkt5: VecDeque<(i64, f64)>,
    mkt5_sum: f64,
    mkt10: VecDeque<(i64, f64)>,
    mkt10_sum: f64,
    /// First-seen times of newborn-proxy coins within 60 s.
    new_coins: VecDeque<i64>,
}

/// What the engine tells the caller after applying one event.
pub struct Step {
    pub snapshot: Option<Vec<f32>>,
    /// The radar promoted the coin on this event (decision row, V5 §3.3).
    pub signal: bool,
    pub dev_sell: bool,
    /// Fraction of the creator's token balance sold in this event (0 if not a creator sell).
    pub dev_sell_frac: f64,
}

impl Engine {
    pub fn new(ev: &Events, rp: RadarParams) -> Self {
        let n = ev.mints.len();
        Self {
            pools: vec![Pool::default(); n],
            coins: (0..n).map(|_| Coin::default()).collect(),
            radar: Radar::new(rp).with_newborn_after(ev.window.0 + 60),
            first_seen: FxHashMap::default(),
            window_start: ev.window.0,
            window_end: ev.window.1,
            last_sweep: i64::MIN,
            sig: Vec::new(),
            created: ev.created.clone(),
            ctx: Context::default(),
            live: FxHashMap::default(),
            mkt5: VecDeque::new(),
            mkt5_sum: 0.0,
            mkt10: VecDeque::new(),
            mkt10_sum: 0.0,
            new_coins: VecDeque::new(),
        }
    }

    pub fn with_ctx(mut self, ctx: Context) -> Self {
        self.ctx = ctx;
        self
    }

    fn wallet(&self, u: u32) -> Option<&WalletInfo> {
        self.ctx.wallets.get(u as usize).and_then(|w| w.as_ref())
    }

    fn is_smart(&self, u: u32) -> bool {
        self.wallet(u).is_some_and(|w| !w.is_bot && w.n_closed > 0 && w.score > SMART_THETA)
    }

    /// Close the coin's current transaction group (bundle / wash / multi-sell markers).
    fn close_tx(c: &mut Coin, t: i64) {
        if c.cur_tx == 0 {
            return;
        }
        let mut b = c.cur_tx_buyers.clone();
        b.sort_unstable();
        b.dedup();
        let mut s = c.cur_tx_sellers.clone();
        s.sort_unstable();
        s.dedup();
        if b.len() >= 2 {
            c.bundle_buy_sol += c.cur_tx_buy_sol;
            c.tx_marks.push_back((t, 1));
        }
        if b.iter().any(|u| s.binary_search(u).is_ok()) {
            c.tx_marks.push_back((t, 2));
        }
        if s.len() >= 2 {
            c.tx_marks.push_back((t, 3));
        }
        c.cur_tx_buyers.clear();
        c.cur_tx_sellers.clear();
        c.cur_tx_buy_sol = 0.0;
    }

    pub fn apply(&mut self, e: &crate::backtest::Ev) -> Step {
        if e.time > self.last_sweep {
            self.last_sweep = e.time;
            self.radar.sweep(e.time, &mut self.sig);
            self.sig.clear();
        }
        let ci = e.coin as usize;
        let p = &mut self.pools[ci];
        if e.amm || !p.amm {
            *p = Pool {
                quote_res: e.quote_res,
                token_res: e.token_res,
                real_quote: e.real_quote,
                real_token: e.real_token,
                fee_bps: e.fee_bps,
                amm: e.amm,
                complete: e.complete,
                known: true,
            };
        }
        let pool = *p;
        let solf = e.sol as f64 / 1e9;
        // Market-wide volume (all coins) and newborn arrivals.
        self.mkt5.push_back((e.time, solf));
        self.mkt5_sum += solf;
        self.mkt10.push_back((e.time, solf));
        self.mkt10_sum += solf;
        while let Some(&(t0, v)) = self.mkt5.front() {
            if t0 > e.time - 300 {
                break;
            }
            self.mkt5_sum -= v;
            self.mkt5.pop_front();
        }
        while let Some(&(t0, v)) = self.mkt10.front() {
            if t0 > e.time - 600 {
                break;
            }
            self.mkt10_sum -= v;
            self.mkt10.pop_front();
        }
        while self.new_coins.front().is_some_and(|x| *x <= e.time - 60) {
            self.new_coins.pop_front();
        }
        // Trader speed: per-wallet activity across all coins.
        {
            let w = self.live.entry(e.user).or_insert(WalletLive { first_t: e.time, last_t: e.time, n: 0, gap_ewma: 0.0 });
            if w.n > 0 {
                let gap = (e.time - w.last_t) as f64;
                w.gap_ewma = if w.n == 1 { gap } else { 0.7 * w.gap_ewma + 0.3 * gap };
            }
            w.n += 1;
            w.last_t = e.time;
        }
        let c = &mut self.coins[ci];
        if c.first_time == 0 {
            c.first_time = e.time;
            c.first_price = pool.price();
            if e.time >= self.window_start + 60 && e.time <= self.window_end {
                self.new_coins.push_back(e.time);
            }
        }
        c.on_amm |= e.amm;
        c.last_event = e.time;
        c.n_trades += 1;
        c.creator.get_or_insert(e.creator);
        // Transaction / slot grouping (events of one transaction are consecutive in chain order).
        let tx = (e.order >> 8) + 1;
        if tx != c.cur_tx {
            Self::close_tx(c, e.time);
            c.cur_tx = tx;
        }
        if e.buy {
            c.cur_tx_buyers.push(e.user);
            c.cur_tx_buy_sol += solf;
            c.buy_sol_total += solf;
        } else {
            c.cur_tx_sellers.push(e.user);
        }
        if e.slot != c.cur_slot {
            if c.cur_slot != 0 && !c.cur_slot_buyers.is_empty() {
                let mut b = std::mem::take(&mut c.cur_slot_buyers);
                b.sort_unstable();
                b.dedup();
                c.slot_buys.push_back((e.time, c.cur_slot, b.len() as u32));
            }
            c.cur_slot = e.slot;
            c.cur_slot_buyers.clear();
        }
        if e.buy && solf >= REAL_MIN_SOL {
            c.cur_slot_buyers.push(e.user);
        }
        while c.tx_marks.front().is_some_and(|x| x.0 <= e.time - 60) {
            c.tx_marks.pop_front();
        }
        while c.slot_buys.front().is_some_and(|x| x.0 <= e.time - 60) {
            c.slot_buys.pop_front();
        }
        // F8 arrival + Hawkes (lambda <- lambda * exp(-beta * dslot) + 1).
        c.trade_slots.push_back(e.slot);
        if c.trade_slots.len() > 21 {
            c.trade_slots.pop_front();
        }
        let hk = |h: &mut (f64, u64), slot: u64, beta: f64| {
            h.0 = h.0 * (-(beta * slot.saturating_sub(h.1) as f64)).exp() + 1.0;
            h.1 = slot;
        };
        hk(&mut c.hk_fast, e.slot, HK_FAST);
        hk(&mut c.hk_slow, e.slot, HK_SLOW);
        if e.buy {
            hk(&mut c.hk_buy, e.slot, HK_FAST);
        } else {
            hk(&mut c.hk_sell, e.slot, HK_FAST);
        }
        // Balances / holders.
        let delta = if e.buy { e.tokens as i64 } else { -(e.tokens as i64) };
        let b = c.bal.entry(e.user).or_insert(0);
        let before = *b;
        // Cost basis (average cost): buys add the SOL paid; sells remove the sold share of the basis.
        {
            let cb = c.cost.entry(e.user).or_insert(0.0);
            if e.buy {
                *cb += e.sol as f64;
            } else if before > 0 {
                let after = (before - e.tokens as i64).max(0);
                *cb *= after as f64 / before as f64;
            }
        }
        let dev_sell_frac = if !e.buy && e.user == e.creator && e.tokens > 0 {
            if before > 0 { (e.tokens as f64 / before as f64).min(1.0) } else { 1.0 }
        } else {
            0.0
        };
        *b += delta;
        if before <= 0 && *b > 0 {
            c.holders += 1;
        } else if before > 0 && *b <= 0 {
            c.holders -= 1;
        }
        c.holders_max = c.holders_max.max(c.holders);
        c.holders_hist.push_back((e.time, c.holders));
        while c.holders_hist.len() > 1 && c.holders_hist[1].0 <= e.time - 60 {
            c.holders_hist.pop_front();
        }
        if e.buy && c.early.len() < 20 && !c.early.contains(&e.user) {
            c.early.push(e.user);
            c.early_first.push(e.time);
        }
        if e.buy {
            if let Some(x) = c.early_bought.get_mut(&e.user) {
                *x += e.tokens as i64;
            } else if c.early.contains(&e.user) {
                c.early_bought.insert(e.user, e.tokens as i64);
            }
        }
        self.first_seen.entry(e.user).or_insert(e.time);
        c.tr.push_back((e.time, e.user, e.buy, solf));
        while c.tr.front().is_some_and(|x| x.0 <= e.time - 300) {
            c.tr.pop_front();
        }
        if !e.amm && c.on_amm {
            // late curve event after migration: ignore for the price path
        } else {
            let pr = pool.price();
            c.px.push_back((e.time, pr));
            if c.last_price > 0.0 && pr > 0.0 {
                c.flow.push_back((e.time, (pr / c.last_price).ln(), if e.buy { solf } else { -solf }));
            }
            c.last_price = pr;
            let progress = if pool.amm { 1.0 } else { 1.0 - pool.real_token as f64 / CURVE_INITIAL_REAL_TOKEN };
            c.prog_hist.push_back((e.time, progress));
        }
        while c.px.front().is_some_and(|x| x.0 <= e.time - 300) {
            c.px.pop_front();
        }
        while c.flow.front().is_some_and(|x| x.0 <= e.time - 60) {
            c.flow.pop_front();
        }
        while c.prog_hist.len() > 1 && c.prog_hist[1].0 <= e.time - 60 {
            c.prog_hist.pop_front();
        }
        let real = solf >= REAL_MIN_SOL;
        if real && e.buy {
            c.buy_slots.push_back(e.slot);
            if c.buy_slots.len() > 100 {
                c.buy_slots.pop_front();
            }
            ewma(&mut c.fast, e.time as f64, TAU_FAST);
            ewma(&mut c.slow, e.time as f64, TAU_SLOW);
            c.last_real_buy = e.time;
        }
        let dev_sell = !e.buy && e.user == e.creator && e.tokens > 0;
        if e.user == e.creator {
            if e.buy {
                c.creator_buy_sol += solf;
            } else {
                c.creator_sell_sol += solf;
                c.dev_sell_time = Some(e.time);
            }
        }
        let price = if e.tokens > 0 { solf / e.tokens as f64 } else { 0.0 };
        let tr = Trade { time: e.time, order: e.order, coin: e.coin, trader: e.user, buy: e.buy, sol: solf, price };
        let signal = self.radar.on_trade(&tr) == Some(Signal::Promote);
        if signal {
            c.hot_time.get_or_insert(e.time);
        }
        let due = signal || (real && c.hot_time.is_some_and(|h| e.time - c.last_snap >= snap_every(e.time - h)));
        let snapshot = if due {
            c.last_snap = e.time;
            Some(self.features(ci, e.time))
        } else {
            None
        };
        Step { snapshot, signal, dev_sell, dev_sell_frac }
    }

    pub fn pool_value(&self, coin: u32, tokens: u64, default_fee: u64) -> f64 {
        let p = &self.pools[coin as usize];
        if !p.known || tokens == 0 {
            return 0.0;
        }
        let mut q = sell_quote_out(p.quote_res, p.token_res, tokens, if p.fee_bps > 0 { p.fee_bps } else { default_fee });
        if !p.amm {
            q = q.min(p.real_quote as u128);
        }
        q as f64 / 1e9
    }

    pub fn fill_buy(&self, coin: u32, lamports: u64, default_fee: u64) -> u128 {
        let p = &self.pools[coin as usize];
        if !p.known || (p.complete && !p.amm) {
            return 0;
        }
        let t = buy_tokens_out(p.quote_res, p.token_res, lamports, if p.fee_bps > 0 { p.fee_bps } else { default_fee });
        if p.amm {
            t
        } else {
            t.min(p.real_token as u128)
        }
    }

    /// None if tradable, otherwise the reason. `min_exit_sol` is compared with the real exit liquidity.
    pub fn untradable(&self, coin: u32, min_exit_sol: f64) -> Option<&'static str> {
        let p = &self.pools[coin as usize];
        if !p.known {
            Some("pool_unknown")
        } else if p.complete && !p.amm {
            Some("migrating_curve_complete")
        } else if p.exit_liquidity_sol() < min_exit_sol {
            Some("low_liquidity")
        } else {
            None
        }
    }

    /// Marginal price (quote / token reserves) of the coin's current pool.
    pub fn price(&self, coin: u32) -> f64 {
        self.pools[coin as usize].price()
    }

    pub fn is_amm(&self, coin: u32) -> bool {
        self.pools[coin as usize].amm
    }

    /// Real exit liquidity of the coin's current pool, SOL (curve: real quote reserve).
    pub fn exit_liquidity_sol(&self, coin: u32) -> f64 {
        self.pools[coin as usize].exit_liquidity_sol()
    }

    pub fn quote_res_sol(&self, coin: u32) -> f64 {
        self.pools[coin as usize].quote_res as f64 / 1e9
    }

    pub fn dev_sold_within(&self, coin: u32, now: i64, secs: i64) -> bool {
        self.coins[coin as usize].dev_sell_time.is_some_and(|t| now - t < secs)
    }

    pub fn secs_since_real_buy(&self, coin: u32, now: i64) -> i64 {
        now - self.coins[coin as usize].last_real_buy
    }

    fn features(&self, ci: usize, t: i64) -> Vec<f32> {
        let c = &self.coins[ci];
        let p = &self.pools[ci];
        let price = p.price();
        let win = |s: i64| c.tr.iter().filter(move |x| x.0 > t - s);
        let mut buyers15 = FxHashSet::default();
        let mut sellers15 = FxHashSet::default();
        let (mut b15, mut s15) = (0.0, 0.0);
        let mut n15 = 0;
        for &(_, u, b, s) in win(15) {
            n15 += 1;
            if s >= REAL_MIN_SOL {
                if b {
                    buyers15.insert(u);
                    b15 += s;
                } else {
                    sellers15.insert(u);
                    s15 += s;
                }
            }
        }
        let mut buyers60: FxHashMap<u32, (u32, f64)> = FxHashMap::default();
        let mut sellers60 = FxHashSet::default();
        let (mut b60, mut s60, mut n60, mut nb60, mut ns60, mut dust) = (0.0, 0.0, 0, 0, 0, 0);
        let mut sizes = Vec::new();
        for &(_, u, b, s) in win(60) {
            n60 += 1;
            if b {
                nb60 += 1;
                if s < REAL_MIN_SOL {
                    dust += 1;
                    continue;
                }
                let e = buyers60.entry(u).or_insert((0, 0.0));
                e.0 += 1;
                e.1 += s;
                b60 += s;
                sizes.push(s);
            } else {
                ns60 += 1;
                if s >= REAL_MIN_SOL {
                    sellers60.insert(u);
                    s60 += s;
                }
            }
        }
        sizes.sort_by(|a, b| a.total_cmp(b));
        let med = if sizes.is_empty() { 0.0 } else { sizes[sizes.len() / 2] };
        let maxb = sizes.last().copied().unwrap_or(0.0);
        let nb = buyers60.len().max(1) as f64;
        let repeat = buyers60.values().filter(|x| x.0 > 1).count() as f64 / nb;
        let newly = buyers60.keys().filter(|u| self.first_seen.get(u).is_some_and(|f| *f > t - 60)).count() as f64 / nb;
        let top = buyers60.values().map(|x| x.1).fold(0.0, f64::max) / (b60 + 1e-9);
        let px_at = |s: i64| c.px.iter().filter(|x| x.0 <= t - s).next_back().or(c.px.front()).map(|x| x.1).unwrap_or(price);
        let r = |s: i64| {
            let a = px_at(s);
            if a > 0.0 {
                price / a - 1.0
            } else {
                0.0
            }
        };
        let hi = c.px.iter().map(|x| x.1).fold(0.0, f64::max);
        let lo = c.px.iter().map(|x| x.1).fold(f64::INFINITY, f64::min);
        let (mut vw_q, mut vw_t) = (0.0, 0.0);
        for &(tt, _, _, s) in win(60) {
            let _ = tt;
            vw_q += s;
        }
        // VWAP over 60 s from the price path weighted by event count (no per-trade token amounts kept here).
        let pts: Vec<f64> = c.px.iter().filter(|x| x.0 > t - 60).map(|x| x.1).collect();
        if !pts.is_empty() {
            vw_t = pts.iter().sum::<f64>() / pts.len() as f64;
        }
        let _ = vw_q;
        let fast = ewma_at(&c.fast, t as f64, TAU_FAST);
        let slow = ewma_at(&c.slow, t as f64, TAU_SLOW);
        let progress = if p.amm { 1.0 } else { 1.0 - p.real_token as f64 / CURVE_INITIAL_REAL_TOKEN };
        // V5 fix: age / newborn / multiple from the real creation (follow-up), else from first observation.
        let created = self.created.get(ci).copied().flatten();
        let born = created.map(|x| x.0).unwrap_or(c.first_time);
        let first_px = created.and_then(|x| x.1).filter(|x| *x > 0.0).unwrap_or(c.first_price);
        let newborn = match created {
            Some((ct, _)) => ct >= self.window_start,
            None => c.first_time > self.window_start + 60,
        };
        let mut v = vec![
            (t - born) as f32,
            newborn as u8 as f32,
            p.amm as u8 as f32,
            (p.quote_res as f64 / 1e9) as f32,
            progress as f32,
            p.fee_bps as f32,
            n15 as f32,
            n60 as f32,
            c.tr.len() as f32,
            buyers15.len() as f32,
            buyers60.len() as f32,
            sellers60.len() as f32,
            b15 as f32,
            b60 as f32,
            s15 as f32,
            s60 as f32,
            (b15 - s15) as f32,
            (b60 - s60) as f32,
            ((b60 - s60) / (b60 + s60 + 1e-9)) as f32,
            med as f32,
            maxb as f32,
            if nb60 > 0 { dust as f32 / nb60 as f32 } else { 0.0 },
            repeat as f32,
            newly as f32,
            top as f32,
            fast as f32,
            slow as f32,
            (fast / (slow + 1e-9)) as f32,
            (t - c.last_real_buy) as f32,
            r(5) as f32,
            r(15) as f32,
            r(60) as f32,
            r(300) as f32,
            if hi > 0.0 { (price / hi) as f32 } else { 1.0 },
            if lo.is_finite() && lo > 0.0 { (price / lo) as f32 } else { 1.0 },
            if first_px > 0.0 { (price / first_px) as f32 } else { 1.0 },
            if vw_t > 0.0 { (price / vw_t) as f32 } else { 1.0 },
            c.dev_sell_time.is_some() as u8 as f32,
            c.creator_sell_sol as f32,
            c.creator_buy_sol as f32,
            c.hot_time.map(|h| (t - h) as f32).unwrap_or(-1.0),
            sellers15.len() as f32,
            (nb60 as f32) / (ns60 as f32 + 1.0),
            // F1
            if p.amm { -1.0 } else { (p.real_quote as f64 / 1e9) as f32 },
            if p.amm { -1.0 } else { (p.real_quote as f64 / 1e9 / c.n_trades.max(1) as f64) as f32 },
            c.n_trades as f32,
            // F2
            (c.bundle_buy_sol / (c.buy_sol_total + 1e-9)) as f32,
            c.tx_marks.iter().filter(|x| x.1 == 1).count() as f32,
            {
                let held: i64 = c.bal.values().filter(|v| **v > 0).sum();
                let early: i64 = c.early.iter().filter_map(|u| c.bal.get(u)).filter(|v| **v > 0).sum();
                if held > 0 { early as f32 / held as f32 } else { 0.0 }
            },
            {
                let mut v: Vec<i64> = c.bal.values().copied().filter(|v| *v > 0).collect();
                let held: i64 = v.iter().sum();
                v.sort_unstable_by(|a, b| b.cmp(a));
                if held > 0 { v.iter().take(10).sum::<i64>() as f32 / held as f32 } else { 0.0 }
            },
            // F4
            c.tx_marks.iter().filter(|x| x.1 == 2).count() as f32 / (n60 as f32 + 1.0),
            c.tx_marks.iter().filter(|x| x.1 == 3).count() as f32,
            // F6
            c.holders as f32,
            if c.holders_max > 0 { c.holders as f32 / c.holders_max as f32 } else { 1.0 },
            // F7
            c.slot_buys.iter().map(|x| x.2).max().unwrap_or(0) as f32,
            {
                let v: Vec<f64> = c.slot_buys.iter().map(|x| x.2 as f64).collect();
                if v.len() < 2 { 0.0 } else {
                    let m = v.iter().sum::<f64>() / v.len() as f64;
                    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() - 1) as f64).sqrt() as f32
                }
            },
            // F8
            {
                let mut d: Vec<f64> = c.trade_slots.iter().zip(c.trade_slots.iter().skip(1)).map(|(a, b)| (b - a) as f64).collect();
                d.sort_by(|a, b| a.total_cmp(b));
                if d.is_empty() { -1.0 } else { d[d.len() / 2] as f32 }
            },
            {
                let mut d: Vec<f64> = c.trade_slots.iter().zip(c.trade_slots.iter().skip(1)).map(|(a, b)| (b - a) as f64).collect();
                d.sort_by(|a, b| a.total_cmp(b));
                if d.is_empty() { -1.0 } else { d[((d.len() - 1) as f64 * 0.9) as usize] as f32 }
            },
            {
                let d: Vec<f64> = c.trade_slots.iter().zip(c.trade_slots.iter().skip(1)).map(|(a, b)| (b - a) as f64).collect();
                if d.len() < 2 { -1.0 } else {
                    let m = d.iter().sum::<f64>() / d.len() as f64;
                    let sd = (d.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (d.len() - 1) as f64).sqrt();
                    if m > 0.0 { (sd / m) as f32 } else { 0.0 }
                }
            },
            {
                let n = c.trade_slots.len();
                if n >= 2 { (c.trade_slots[n - 1] - c.trade_slots[n - 2]) as f32 } else { -1.0 }
            },
            {
                let d: Vec<u64> = c.trade_slots.iter().zip(c.trade_slots.iter().skip(1)).map(|(a, b)| b - a).collect();
                if d.is_empty() { 0.0 } else { d.iter().filter(|x| **x <= 1).count() as f32 / d.len() as f32 }
            },
            c.hk_fast.0 as f32,
            c.hk_slow.0 as f32,
            (c.hk_fast.0 / (c.hk_slow.0 + 1e-9)) as f32,
            {
                // decay both to the current slot before comparing
                let now = c.cur_slot;
                let b = c.hk_buy.0 * (-(HK_FAST * now.saturating_sub(c.hk_buy.1) as f64)).exp();
                let s = c.hk_sell.0 * (-(HK_FAST * now.saturating_sub(c.hk_sell.1) as f64)).exp();
                (b / (s + 0.1)) as f32
            },
            {
                let held: i64 = c.bal.values().filter(|v| **v > 0).sum();
                let dev = c.creator.and_then(|cr| c.bal.get(&cr)).copied().unwrap_or(0).max(0);
                if held > 0 { dev as f32 / held as f32 } else { 0.0 }
            },
            (c.creator_buy_sol - c.creator_sell_sol) as f32,
            c.dev_sell_time.is_some() as u8 as f32,
            {
                let mut s5 = FxHashSet::default();
                for &(_, u, b, sz) in win(5) { if !b && sz >= REAL_MIN_SOL { s5.insert(u); } }
                s5.len() as f32
            },
            win(5).filter(|x| !x.2).map(|x| x.3).sum::<f64>() as f32,
            win(5).filter(|x| x.2).map(|x| x.3).sum::<f64>() as f32,
            win(15).filter(|x| !x.2).map(|x| x.3).fold(0.0, f64::max) as f32,
        ];
        v.extend(self.features_v5(ci, t, born));
        debug_assert_eq!(v.len(), FEATURES.len());
        v
    }

    /// V5 §4 features (appended after the V4 block). All causal: state up to and including the current event.
    fn features_v5(&self, ci: usize, t: i64, born: i64) -> Vec<f32> {
        let c = &self.coins[ci];
        let p = &self.pools[ci];
        let price = p.price();
        let q = (p.quote_res as f64 / 1e9).max(1e-9);
        let real = |s: f64| s >= REAL_MIN_SOL;
        let pct = |v: &mut Vec<f64>, f: f64| -> f64 {
            if v.is_empty() {
                return 0.0;
            }
            v.sort_by(|a, b| a.total_cmp(b));
            v[((v.len() - 1) as f64 * f) as usize]
        };
        // Windows over the 300 s trade deque.
        let (mut b60, mut s60, mut smart60, mut flip60, mut clus60, mut fast60, mut retail60) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        let (mut buys, mut sells) = (Vec::new(), Vec::new());
        let mut whales = 0u32;
        let mut buyers_now: FxHashSet<u32> = FxHashSet::default();
        let mut buyers_prev: FxHashSet<u32> = FxHashSet::default();
        let mut smart300: FxHashSet<u32> = FxHashSet::default();
        let mut rates: Vec<f64> = Vec::new();
        let mut cvd = 0.0;
        for &(tt, u, b, sz) in &c.tr {
            cvd += if b { sz } else { -sz };
            if b && real(sz) && self.is_smart(u) {
                smart300.insert(u);
            }
            if b && real(sz) && tt > t - 120 && tt <= t - 60 {
                buyers_prev.insert(u);
            }
            if tt <= t - 60 {
                continue;
            }
            if b {
                if !real(sz) {
                    continue;
                }
                b60 += sz;
                buys.push(sz);
                if buyers_now.insert(u) {
                    if let Some(w) = self.live.get(&u) {
                        rates.push(w.n as f64 / ((t - w.first_t) as f64 / 60.0 + 1.0));
                    }
                }
                if self.is_smart(u) {
                    smart60 += sz;
                }
                if let Some(w) = self.wallet(u) {
                    if w.is_flipper {
                        flip60 += sz;
                    }
                    if w.cluster_size >= 2 {
                        clus60 += sz;
                    }
                }
                if self.live.get(&u).is_some_and(|w| w.n >= 3 && w.gap_ewma < 5.0) {
                    fast60 += sz;
                }
                if (0.05..=2.0).contains(&sz) {
                    retail60 += sz;
                }
                if sz > 5.0 {
                    whales += 1;
                }
            } else if real(sz) {
                s60 += sz;
                sells.push(sz);
            }
        }
        let share = |x: f64| if b60 > 0.0 { (x / b60) as f32 } else { 0.0 };
        let hhi = if b60 > 0.0 { buys.iter().map(|x| (x / b60).powi(2)).sum::<f64>() } else { 0.0 };
        // Holder velocity and curve speed over ~60 s (oldest kept point is the last one at or before t - 60).
        let holder_vel = c.holders_hist.front().map(|h| (c.holders - h.1) as f64 / 60.0).unwrap_or(0.0);
        let (curve_speed, eta) = if p.amm {
            (-1.0, -1.0)
        } else {
            let now_prog = c.prog_hist.back().map(|x| x.1).unwrap_or(0.0);
            match c.prog_hist.front() {
                Some(&(t0, p0)) if t - t0 >= 1 => {
                    let sp = (now_prog - p0) / (t - t0) as f64;
                    (sp, if sp > 0.0 { ((1.0 - now_prog) / sp).min(1e6) } else { -1.0 })
                }
                _ => (0.0, -1.0),
            }
        };
        // Hawkes branching ratio, moment estimator: for a stationary Hawkes process the count Fano factor over
        // long bins is 1 / (1 - n)^2, so n = 1 - 1 / sqrt(Fano). Bins of 10 slots over the last 100 real buys.
        let branching = if c.buy_slots.len() >= 20 {
            let first = *c.buy_slots.front().expect("len");
            let nb = ((c.buy_slots.back().expect("len") - first) / 10 + 1) as usize;
            if nb < 3 {
                0.99
            } else {
                let mut cnt = vec![0f64; nb];
                for s in &c.buy_slots {
                    cnt[((s - first) / 10) as usize] += 1.0;
                }
                let m = cnt.iter().sum::<f64>() / nb as f64;
                let var = cnt.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (nb - 1) as f64;
                let fano = var / m;
                if fano <= 1.0 { 0.0 } else { (1.0 - 1.0 / fano.sqrt()).clamp(0.0, 0.99) }
            }
        } else {
            -1.0
        };
        // Kyle lambda: OLS slope of d ln P on signed SOL over the last 60 s.
        let kyle = {
            let n = c.flow.len() as f64;
            if n < 5.0 {
                0.0
            } else {
                let mx = c.flow.iter().map(|x| x.2).sum::<f64>() / n;
                let my = c.flow.iter().map(|x| x.1).sum::<f64>() / n;
                let sxx: f64 = c.flow.iter().map(|x| (x.2 - mx).powi(2)).sum();
                let sxy: f64 = c.flow.iter().map(|x| (x.2 - mx) * (x.1 - my)).sum();
                if sxx > 0.0 { sxy / sxx } else { 0.0 }
            }
        };
        // Volatility: std of 1 s log returns of the last price per second over 60 s.
        let vol = {
            let mut last = c.px.iter().filter(|x| x.0 <= t - 60).next_back().or(c.px.front()).map(|x| x.1).unwrap_or(price);
            let mut it = c.px.iter().filter(|x| x.0 > t - 60).peekable();
            let mut rets = Vec::with_capacity(60);
            for sec in (t - 59)..=t {
                let mut cur = last;
                while let Some(x) = it.peek() {
                    if x.0 > sec {
                        break;
                    }
                    cur = x.1;
                    it.next();
                }
                if last > 0.0 && cur > 0.0 {
                    rets.push((cur / last).ln());
                }
                last = cur;
            }
            if rets.len() < 2 {
                0.0
            } else {
                let m = rets.iter().sum::<f64>() / rets.len() as f64;
                (rets.iter().map(|r| (r - m).powi(2)).sum::<f64>() / (rets.len() - 1) as f64).sqrt()
            }
        };
        // Overhang: early buyers' retention; unrealized profit share of holders (average cost).
        let (mut early_held, mut early_bought) = (0i64, 0i64);
        for (u, bought) in &c.early_bought {
            early_bought += bought;
            early_held += c.bal.get(u).copied().unwrap_or(0).clamp(0, *bought);
        }
        let (mut prof, mut mcap, mut held_all, mut held_snipers) = (0.0, 0.0, 0i64, 0i64);
        let mut top: Vec<i64> = Vec::new();
        for (u, bal) in &c.bal {
            if *bal <= 0 {
                continue;
            }
            let bf = *bal as f64;
            let avg = c.cost.get(u).copied().unwrap_or(0.0) / bf;
            prof += bf * (price - avg).max(0.0);
            mcap += bf * price;
            held_all += bal;
            top.push(*bal);
            if self.wallet(*u).is_some_and(|w| w.sniper_coins >= SNIPER_MIN_COINS) {
                held_snipers += bal;
            }
        }
        // Regime: market-wide, only meaningful inside the backfill window (all coins observed there).
        let in_win = t <= self.window_end;
        let coin_vol5: f64 = c.tr.iter().map(|x| x.3).sum();
        let sol_ret_1h = {
            let at = |ts: i64| -> Option<f64> {
                let i = self.ctx.sol_usd.partition_point(|x| x.0 <= ts);
                (i > 0).then(|| self.ctx.sol_usd[i - 1].1)
            };
            match (at(t), at(t - 3600)) {
                (Some(a), Some(b)) if b > 0.0 => (a / b - 1.0) as f32,
                _ => f32::NAN,
            }
        };
        let mut early_delay: Vec<f64> = c.early_first.iter().map(|x| (x - born) as f64).collect();
        vec![
            self.created.get(ci).copied().flatten().is_some() as u8 as f32,
            q.ln() as f32,
            (smart60 / q) as f32,
            share(smart60),
            smart300.len() as f32,
            share(flip60),
            if held_all > 0 { held_snipers as f32 / held_all as f32 } else { 0.0 },
            share(clus60),
            buyers_now.len() as f32 / (buyers_prev.len() as f32 + 1.0),
            holder_vel as f32,
            branching as f32,
            hhi as f32,
            share(retail60),
            whales as f32,
            pct(&mut sells.clone(), 0.5) as f32,
            pct(&mut buys, 0.9) as f32,
            pct(&mut sells, 0.9) as f32,
            (b60 / q) as f32,
            (s60 / q) as f32,
            ((b60 - s60) / q) as f32,
            (cvd / q) as f32,
            kyle as f32,
            vol as f32,
            if early_bought > 0 { early_held as f32 / early_bought as f32 } else { -1.0 },
            if mcap > 0.0 { (prof / mcap) as f32 } else { 0.0 },
            curve_speed as f32,
            eta as f32,
            if in_win { if self.mkt5_sum > 0.0 { (coin_vol5 / self.mkt5_sum) as f32 } else { 0.0 } } else { -1.0 },
            if in_win { self.new_coins.len() as f32 } else { -1.0 },
            if in_win { self.mkt10_sum as f32 } else { -1.0 },
            sol_ret_1h,
            share(fast60),
            pct(&mut rates, 0.5) as f32,
            pct(&mut early_delay, 0.5) as f32,
            {
                top.sort_unstable_by(|a, b| b.cmp(a));
                (top.iter().take(10).sum::<i64>() as f64 / TOTAL_SUPPLY_RAW) as f32
            },
        ]
    }
}

/// Marginal price path per coin: (time, price); curve until the coin first trades on the AMM.
pub fn paths(ev: &Events) -> Vec<Vec<(i64, f64)>> {
    let mut out: Vec<Vec<(i64, f64)>> = vec![Vec::new(); ev.mints.len()];
    let mut on_amm = vec![false; ev.mints.len()];
    for e in &ev.evs {
        let c = e.coin as usize;
        if e.amm {
            on_amm[c] = true;
        } else if on_amm[c] {
            continue;
        }
        if e.token_res > 0 {
            out[c].push((e.time, e.quote_res as f64 / e.token_res as f64));
        }
    }
    out
}

/// V5 §3.1 labels from time t (p0 = first path price at time >= t + LABEL_LAT_S), over [t, min(t + H, cover_end)].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Labels {
    /// Triple barrier: +100% before -35% within H.
    pub tp1: u8,
    /// Peak-multiple class before the -65% barrier: 0 <0.35 | 1 0.35-1 | 2 1-2 | 3 2-5 | 4 5-10 | 5 10-50 | 6 >=50.
    pub bin: u8,
    /// Peak multiple before the -65% barrier (the value binned in `bin`).
    pub peak_barrier: f32,
    /// +100% before -50% within H (hold model).
    pub hold: u8,
    /// The coin dies (V5 §3.2 rule) within the next 10 min.
    pub dead_10m: u8,
    /// ln(max price within H / p0), no barrier.
    pub logmax: f32,
    /// Last price within H / p0.
    pub end_h: f32,
    /// Ends <= 0.2 p0 without ever reaching 2x within H.
    pub rugdead: u8,
    /// Seconds from t to the first 2x (-1 if none).
    pub t_2x: f32,
}

const BIN_EDGES: [f64; 6] = [0.35, 1.0, 2.0, 5.0, 10.0, 50.0];

pub fn bin_of(m: f64) -> u8 {
    BIN_EDGES.iter().take_while(|e| m >= **e).count() as u8
}

pub fn labels(path: &[(i64, f64)], t: i64, deaths: &[i64], cover_end: i64) -> Option<Labels> {
    let i0 = path.partition_point(|x| x.0 < t + LABEL_LAT_S);
    let p0 = path.get(i0)?.1;
    if p0 <= 0.0 {
        return None;
    }
    let end = (t + H_S).min(cover_end);
    let (mut tp1, mut hold): (Option<u8>, Option<u8>) = (None, None);
    let (mut barrier, mut peak_b, mut mx, mut last, mut t2) = (false, f64::NAN, 1.0f64, 1.0f64, -1.0f32);
    for &(tt, p) in &path[i0 + 1..] {
        if tt > end {
            break;
        }
        let m = p / p0;
        mx = mx.max(m);
        last = m;
        if m >= 2.0 && t2 < 0.0 {
            t2 = (tt - t) as f32;
        }
        if tp1.is_none() {
            if m >= 2.0 {
                tp1 = Some(1);
            } else if m <= 0.65 {
                tp1 = Some(0);
            }
        }
        if hold.is_none() {
            if m >= 2.0 {
                hold = Some(1);
            } else if m <= 0.5 {
                hold = Some(0);
            }
        }
        if !barrier {
            peak_b = if peak_b.is_nan() { m } else { peak_b.max(m) };
            barrier = m <= 0.35;
        }
    }
    // No trade after p0 within the horizon: nothing happened, the multiple stays 1.
    let peak_b = if peak_b.is_nan() { 1.0 } else { peak_b };
    Some(Labels {
        tp1: tp1.unwrap_or(0),
        bin: bin_of(peak_b),
        peak_barrier: peak_b as f32,
        hold: hold.unwrap_or(0),
        dead_10m: deaths.iter().any(|&d| d > t && d <= t + 600) as u8,
        logmax: mx.ln() as f32,
        end_h: last as f32,
        rugdead: (last <= 0.2 && mx < 2.0) as u8,
        t_2x: t2,
    })
}

/// V5 §3.2 censoring: the label window [t, t + H] is covered by data, or the coin died inside the data first.
pub fn label_covered(t: i64, deaths: &[i64], cover_end: i64) -> bool {
    cover_end >= t + H_S || deaths.iter().any(|&d| d > t && d <= cover_end)
}

/// Death times per coin (V5 §3.2): a real-trade gap of >= DEAD_IDLE_S during which, at last_real + DEAD_IDLE_S,
/// the marginal price is below DEAD_PRICE_FRAC x the running peak. Death time = last_real + DEAD_IDLE_S.
/// Same rule as the `follow` stop (`follow::Life`).
pub fn death_times(ev: &Events) -> Vec<Vec<i64>> {
    use crate::follow::{Life, DEAD_IDLE_S};
    let n = ev.mints.len();
    let mut out: Vec<Vec<i64>> = vec![Vec::new(); n];
    let mut life: Vec<Option<Life>> = vec![None; n];
    let mut checked = vec![true; n];
    for e in &ev.evs {
        let c = e.coin as usize;
        if let Some(l) = &life[c] {
            if !checked[c] && e.time - l.last_real >= DEAD_IDLE_S {
                checked[c] = true;
                if let Some(d) = l.dead_at(e.time) {
                    out[c].push(d);
                }
            }
        }
        let l = life[c].get_or_insert(Life { last_real: e.time, ..Default::default() });
        let before = l.last_real;
        l.on_event(e.time, e.amm, e.sol, e.quote_res, e.token_res);
        if l.last_real != before || e.sol >= (REAL_MIN_SOL * 1e9) as u64 {
            checked[c] = false;
        }
    }
    for c in 0..n {
        if let Some(l) = &life[c] {
            if !checked[c] && ev.cover_end[c] - l.last_real >= DEAD_IDLE_S {
                if let Some(d) = l.dead_at(ev.cover_end[c]) {
                    out[c].push(d);
                }
            }
        }
    }
    out
}

/// Counts reported by `write_snapshots`.
#[derive(Debug, Default, Serialize)]
pub struct SnapStats {
    pub rows: usize,
    pub signal_rows: usize,
    pub signal_rows_in_window: usize,
    pub dropped_censored: usize,
    pub dropped_censored_signal: usize,
    pub dropped_no_p0: usize,
    pub coins: usize,
    pub coins_followed: usize,
}

pub const LABEL_COLS: [&str; 9] = ["y_tp1", "y_bin", "y_peak_barrier", "y_hold", "y_dead_10m", "y_logmax", "y_end_h", "y_rugdead", "y_t2x"];

/// Training snapshots with V5 labels for one recording (window + follow-up), zstd-compressed CSV.
pub fn write_snapshots(ev: &Events, rp: RadarParams, ctx: Context, out: &std::path::Path) -> anyhow::Result<SnapStats> {
    let mut eng = Engine::new(ev, rp).with_ctx(ctx);
    let pth = paths(ev);
    let deaths = death_times(ev);
    let file = std::fs::File::create(out)?;
    let mut f = std::io::BufWriter::with_capacity(1 << 22, zstd::stream::write::Encoder::new(file, 3)?.auto_finish());
    write!(f, "coin,order,time,is_signal,in_window")?;
    for n in FEATURES {
        write!(f, ",{n}")?;
    }
    for n in LABEL_COLS {
        write!(f, ",{n}")?;
    }
    writeln!(f)?;
    let mut st = SnapStats { coins_followed: ev.followed.iter().filter(|x| **x).count(), ..Default::default() };
    let mut seen = vec![false; ev.mints.len()];
    for e in &ev.evs {
        let step = eng.apply(e);
        let Some(x) = step.snapshot else { continue };
        let ci = e.coin as usize;
        if !label_covered(e.time, &deaths[ci], ev.cover_end[ci]) {
            st.dropped_censored += 1;
            st.dropped_censored_signal += step.signal as usize;
            continue;
        }
        let Some(y) = labels(&pth[ci], e.time, &deaths[ci], ev.cover_end[ci]) else {
            st.dropped_no_p0 += 1;
            continue;
        };
        let in_win = e.time <= ev.window.1;
        write!(f, "{},{},{},{},{}", e.coin, e.order, e.time - ev.window.0, step.signal as u8, in_win as u8)?;
        for v in &x {
            write!(f, ",{v}")?;
        }
        writeln!(f, ",{},{},{},{},{},{},{},{},{}", y.tp1, y.bin, y.peak_barrier, y.hold, y.dead_10m, y.logmax, y.end_h, y.rugdead, y.t_2x)?;
        st.rows += 1;
        st.signal_rows += step.signal as usize;
        st.signal_rows_in_window += (step.signal && in_win) as usize;
        if !seen[ci] {
            seen[ci] = true;
            st.coins += 1;
        }
    }
    f.flush()?;
    Ok(st)
}

/// Per-window context: `wallet_scores.csv` (offline, causal) and the optional local SOL/USD 1-minute closes.
pub fn load_context(rec: &std::path::Path, ev: &Events) -> anyhow::Result<Context> {
    let mut ctx = Context { wallets: vec![None; ev.users.len()], sol_usd: Vec::new() };
    let wp = rec.join("wallet_scores.csv");
    if wp.exists() {
        let ids: FxHashMap<&str, u32> = ev.users.iter().enumerate().map(|(i, u)| (u.as_str(), i as u32)).collect();
        let text = std::fs::read_to_string(&wp)?;
        let mut lines = text.lines();
        let hdr: Vec<&str> = lines.next().unwrap_or("").split(',').collect();
        let col = |n: &str| hdr.iter().position(|h| *h == n).ok_or_else(|| anyhow::anyhow!("wallet_scores.csv: no column {n}"));
        let (iw, isc, inc, ib, ifl, isn, icl) = (col("wallet")?, col("score")?, col("n_closed")?, col("is_bot")?, col("is_flipper")?, col("sniper_coins")?, col("cluster_size")?);
        for l in lines {
            let v: Vec<&str> = l.split(',').collect();
            let Some(&id) = v.get(iw).and_then(|w| ids.get(w)) else { continue };
            let num = |i: usize| v.get(i).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
            ctx.wallets[id as usize] = Some(WalletInfo {
                score: num(isc) as f32,
                n_closed: num(inc) as u32,
                is_bot: num(ib) != 0.0 || v.get(ib) == Some(&"True"),
                is_flipper: num(ifl) != 0.0 || v.get(ifl) == Some(&"True"),
                sniper_coins: num(isn) as u32,
                cluster_size: num(icl).max(1.0) as u32,
            });
        }
    }
    ctx.sol_usd = crate::units::read_sol_usd_series(rec)?;
    Ok(ctx)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MlStrategy {
    pub entry_p2: f32,
    pub entry_p5: f32,
    pub rug_veto: f32,
    pub exit_cont: f32,
    pub exit_k: u32,
    pub exit_persist_s: i64,
    pub hard_stop: f64,
    pub size_frac: f64,
    pub max_positions: usize,
    pub max_impact: f64,
    pub min_quote_res_sol: f64,
    pub inactivity_s: i64,
    pub scout_s: i64,
    pub reentry_cooldown_s: i64,
    /// Profit protection (not a price stop): once the position reached `giveback_arm` x cost, exit when its
    /// value falls `giveback` below the post-entry peak value. 0 disables.
    pub giveback_arm: f64,
    pub giveback: f64,
    /// Microstructure exit: P(drop >= 20% within 30 s) at or above this exits (1.0 disables).
    pub dump_exit: f32,
    /// Entry veto: P(creator sells within 10 min AND the coin dies) at or above this (1.0 disables).
    pub devsell_veto: f32,
    /// Block entries this many seconds after a creator sell (0 = no block; devs of runners also sell).
    pub dev_sold_block_s: i64,
    /// The predicted-dump exit only protects profit: active once value >= this multiple of cost.
    pub dump_exit_min_mult: f64,
    /// DEV kill-switch: exit when a creator sell is at least this fraction of the creator's balance
    /// (0.0 = any sell, the V4 spec rule).
    pub dev_kill_min_frac: f64,
    /// Hold through momentum / dump / give-back exits while the model still sees P(>= 2x from now) at or
    /// above this (1.0 disables). Rug and DEV kill-switch exits are never suppressed.
    pub hold_if_p2_above: f32,
    /// Fractional Kelly sizing on the current equity (compounding). 0 = fixed `size_frac`.
    /// f = kelly_fraction * (p / kelly_loss - (1 - p) / kelly_win), p = P(peak >= 2x); f <= 0 -> no trade.
    pub kelly_fraction: f64,
    pub kelly_win: f64,
    pub kelly_loss: f64,
    pub max_bet_frac: f64,
}

impl Default for MlStrategy {
    fn default() -> Self {
        Self {
            entry_p2: 0.4,
            entry_p5: 0.0,
            rug_veto: 0.5,
            exit_cont: 0.3,
            exit_k: 3,
            exit_persist_s: 10,
            // No price stop-loss (user decision 2026-10-10); exits are momentum / reversal / DEV / rug.
            hard_stop: 1.0,
            size_frac: 0.15,
            max_positions: 5,
            max_impact: 0.02,
            min_quote_res_sol: 5.0,
            inactivity_s: 120,
            scout_s: 300,
            reentry_cooldown_s: 300,
            giveback_arm: 1.5,
            giveback: 0.35,
            dump_exit: 1.0,
            devsell_veto: 1.0,
            dev_sold_block_s: 0,
            dump_exit_min_mult: 1.2,
            dev_kill_min_frac: 0.0,
            hold_if_p2_above: 1.0,
            kelly_fraction: 0.0,
            kelly_win: 1.0,
            kelly_loss: 0.5,
            max_bet_frac: 0.10,
        }
    }
}

/// Model scores keyed by snapshot event order: [p_up2, p_up5, p_cont, p_rug, p_dump, p_devsell].
pub type Scores = FxHashMap<u64, [f32; 6]>;

pub fn read_scores(path: &std::path::Path) -> anyhow::Result<Scores> {
    let text = std::fs::read_to_string(path)?;
    let mut m = Scores::default();
    for line in text.lines().skip(1) {
        let v: Vec<&str> = line.split(',').collect();
        if v.len() < 6 {
            continue;
        }
        let opt = |i: usize| v.get(i).and_then(|x| x.parse::<f32>().ok()).unwrap_or(0.0);
        m.insert(v[1].parse()?, [v[2].parse()?, v[3].parse()?, v[4].parse()?, v[5].parse()?, opt(6), opt(7)]);
    }
    Ok(m)
}

#[derive(Debug, Serialize, Clone)]
pub struct MlTrade {
    pub mint: String,
    pub entry_t: i64,
    pub exit_t: i64,
    pub cost_sol: f64,
    pub proceeds_sol: f64,
    pub pnl_sol: f64,
    pub ret: f64,
    pub reason: &'static str,
    pub max_mult_held: f64,
    pub p2_at_entry: f32,
}

/// Per-coin decision trace at snapshots (diagnostics).
#[derive(Debug, Serialize, Clone, Default)]
pub struct MlDiag {
    pub snapshots: u32,
    pub max_p2: f32,
    pub snaps_over_entry: u32,
    pub blocked: std::collections::BTreeMap<&'static str, u32>,
    pub entered: u32,
}

#[derive(Debug, Serialize, Clone)]
pub struct MlReport {
    pub window_s: i64,
    pub sol_usd: f64,
    pub start_sol: f64,
    pub final_sol: f64,
    pub pnl_sol: f64,
    pub final_usd: f64,
    pub return_pct: f64,
    pub trades: usize,
    pub coins_traded: usize,
    pub profitable_coins: usize,
    pub win_rate: f64,
    pub max_drawdown_pct: f64,
    pub failed_buys: usize,
    pub costs_sol: f64,
    pub costs_usd: f64,
    pub exits: std::collections::BTreeMap<&'static str, usize>,
    pub held_to_5x: usize,
    pub held_to_10x: usize,
    pub best_ret: f64,
    /// Gross profit / gross loss over closed trades.
    pub profit_factor: f64,
    pub pnl_usd: f64,
    pub expectancy_usd: f64,
    pub expectancy_excl_top1pct_usd: f64,
    pub trades_list: Vec<MlTrade>,
    #[serde(skip)]
    pub diag: Vec<MlDiag>,
}

struct Pos {
    tokens: u64,
    cost: f64,
    entry_t: i64,
    peak: f64,
    max_mult: f64,
    low_count: u32,
    low_since: Option<i64>,
    p2: f32,
    /// Latest P(>= 2x) seen for this coin while held.
    last_p2: f32,
}

fn hash01(a: u64, b: u64) -> f64 {
    let mut x = a.wrapping_mul(0x9E3779B97F4A7C15) ^ b.wrapping_mul(0xC2B2AE3D27D4EB4F);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58476D1CE4E5B9);
    x ^= x >> 29;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

enum Order {
    Buy(u32, u64, f32),
    Sell(u32, &'static str),
}

pub fn simulate(ev: &Events, rp: RadarParams, sc: &Scores, st: &MlStrategy, c: &Costs, sol_usd: f64) -> MlReport {
    let mut eng = Engine::new(ev, rp);
    let start = crate::units::start_sol();
    let mut cash = start;
    let txc = c.tx_cost_lamports as f64 / 1e9;
    let mut pos: FxHashMap<u32, Pos> = FxHashMap::default();
    let mut pending: Vec<(u64, Order)> = Vec::new();
    let mut busy: FxHashSet<u32> = FxHashSet::default();
    let mut blacklist: FxHashSet<u32> = FxHashSet::default();
    let mut last_exit: FxHashMap<u32, i64> = FxHashMap::default();
    let mut trades: Vec<MlTrade> = Vec::new();
    let (mut failed, mut costs) = (0usize, 0.0);
    let (mut peak_eq, mut max_dd) = (start, 0.0f64);
    let mut last_sec = i64::MIN;
    let mut diag: Vec<MlDiag> = vec![MlDiag::default(); ev.mints.len()];
    let evs = &ev.evs;
    let mut i = 0;
    loop {
        let (cur_slot, cur_t) = if i < evs.len() { (evs[i].slot, evs[i].time) } else { (u64::MAX, ev.window.1 + 1) };
        let mut k = 0;
        while k < pending.len() {
            if pending[k].0 >= cur_slot {
                k += 1;
                continue;
            }
            let (_, o) = pending.swap_remove(k);
            cash -= txc;
            costs += txc;
            match o {
                Order::Buy(coin, lam, p2) => {
                    busy.remove(&coin);
                    let toks = eng.fill_buy(coin, lam, c.default_fee_bps);
                    if hash01(coin as u64, cur_slot) < c.buy_fail_prob || toks == 0 {
                        failed += 1;
                        continue;
                    }
                    let cost = lam as f64 / 1e9;
                    cash -= cost;
                    pos.insert(coin, Pos { tokens: toks as u64, cost, entry_t: cur_t, peak: cost, max_mult: 1.0, low_count: 0, low_since: None, p2, last_p2: p2 });
                }
                Order::Sell(coin, reason) => {
                    busy.remove(&coin);
                    let Some(p) = pos.remove(&coin) else { continue };
                    let proceeds = eng.pool_value(coin, p.tokens, c.default_fee_bps);
                    cash += proceeds;
                    last_exit.insert(coin, cur_t);
                    if reason == "DevSell" || reason == "RugModel" {
                        blacklist.insert(coin);
                    }
                    let pnl = proceeds - p.cost - 2.0 * txc;
                    trades.push(MlTrade {
                        mint: ev.mints[coin as usize].clone(),
                        entry_t: p.entry_t - ev.window.0,
                        exit_t: cur_t - ev.window.0,
                        cost_sol: p.cost,
                        proceeds_sol: proceeds,
                        pnl_sol: pnl,
                        ret: pnl / (p.cost + 2.0 * txc),
                        reason,
                        max_mult_held: p.max_mult,
                        p2_at_entry: p.p2,
                    });
                }
            }
        }
        if i >= evs.len() {
            let open: Vec<u32> = pos.keys().copied().filter(|c| !busy.contains(c)).collect();
            if open.is_empty() && pending.is_empty() {
                break;
            }
            for coin in open {
                busy.insert(coin);
                pending.push((0, Order::Sell(coin, "EndOfWindow")));
            }
            continue;
        }
        let e = &evs[i];
        i += 1;
        let step = eng.apply(e);
        // Per-second housekeeping: inactivity, scout stop, equity.
        if e.time > last_sec {
            last_sec = e.time;
            let ids: Vec<u32> = pos.keys().copied().filter(|c| !busy.contains(c)).collect();
            for coin in ids {
                let p = &pos[&coin];
                let reason = if eng.secs_since_real_buy(coin, e.time) > st.inactivity_s {
                    Some("Inactivity")
                } else if e.time - p.entry_t > st.scout_s && p.max_mult < 1.1 {
                    Some("Scout")
                } else {
                    None
                };
                if let Some(r) = reason {
                    busy.insert(coin);
                    pending.push((e.slot + c.latency_slots, Order::Sell(coin, r)));
                }
            }
            let eq = cash + pos.iter().map(|(cid, p)| eng.pool_value(*cid, p.tokens, c.default_fee_bps)).sum::<f64>();
            peak_eq = peak_eq.max(eq);
            max_dd = max_dd.max(1.0 - eq / peak_eq);
        }
        // Position on this coin: P0 dev sell, hard stop on every event; model exit at snapshots.
        if pos.contains_key(&e.coin) && !busy.contains(&e.coin) {
            let val = eng.pool_value(e.coin, pos[&e.coin].tokens, c.default_fee_bps);
            let p = pos.get_mut(&e.coin).expect("present");
            p.peak = p.peak.max(val);
            p.max_mult = p.max_mult.max(val / p.cost);
            let mut reason = if step.dev_sell && step.dev_sell_frac >= st.dev_kill_min_frac {
                Some("DevSell")
            } else if st.hard_stop < 1.0 && val <= (1.0 - st.hard_stop) * p.cost {
                Some("HardStop")
            } else if st.giveback > 0.0 && p.max_mult >= st.giveback_arm && val <= (1.0 - st.giveback) * p.peak && p.last_p2 < st.hold_if_p2_above {
                Some("GiveBackFromPeak")
            } else {
                None
            };
            if reason.is_none() {
                if let (Some(x), Some(s)) = (&step.snapshot, sc.get(&e.order)) {
                    let (p_cont, p_rug) = (s[2], s[3]);
                    p.last_p2 = s[0];
                    let hold = s[0] >= st.hold_if_p2_above;
                    // Strong reversal override (§22.3): money leaving, sellers outnumber buyers, price well off the high.
                    let strong = x[18] < -0.5 && x[41] > x[9] && x[33] < 0.6;
                    if p_rug >= st.rug_veto {
                        reason = Some("RugModel");
                    } else if hold {
                        p.low_count = 0;
                        p.low_since = None;
                    } else if s[4] >= st.dump_exit && p.max_mult >= st.dump_exit_min_mult && val >= st.dump_exit_min_mult * p.cost {
                        reason = Some("DumpPredicted");
                    } else if strong {
                        reason = Some("StrongReversal");
                    } else if p_cont < st.exit_cont {
                        p.low_count += 1;
                        let since = *p.low_since.get_or_insert(e.time);
                        if p.low_count >= st.exit_k && e.time - since >= st.exit_persist_s {
                            reason = Some("MomentumExhausted");
                        }
                    } else {
                        p.low_count = 0;
                        p.low_since = None;
                    }
                }
            }
            if let Some(r) = reason {
                busy.insert(e.coin);
                let lat = if r == "DevSell" { 1 } else { c.latency_slots };
                pending.push((e.slot + lat, Order::Sell(e.coin, r)));
            }
            continue;
        }
        // Entry at snapshots.
        let (Some(_x), Some(s)) = (&step.snapshot, sc.get(&e.order)) else { continue };
        let dg = &mut diag[e.coin as usize];
        dg.snapshots += 1;
        dg.max_p2 = dg.max_p2.max(s[0]);
        if s[0] < st.entry_p2 || s[1] < st.entry_p5 {
            continue;
        }
        dg.snaps_over_entry += 1;
        let blocked: Option<&'static str> = if s[3] >= st.rug_veto {
            Some("rug_veto")
        } else if s[5] >= st.devsell_veto {
            Some("devsell_veto")
        } else if pos.len() + busy.iter().filter(|b| !pos.contains_key(b)).count() >= st.max_positions {
            Some("slots_full")
        } else if busy.contains(&e.coin) {
            Some("order_pending")
        } else if blacklist.contains(&e.coin) {
            Some("blacklisted")
        } else if last_exit.get(&e.coin).is_some_and(|t| e.time - t < st.reentry_cooldown_s) {
            Some("reentry_cooldown")
        } else if st.dev_sold_block_s > 0 && eng.dev_sold_within(e.coin, e.time, st.dev_sold_block_s) {
            Some("dev_sold_recently")
        } else if let Some(r) = eng.untradable(e.coin, st.min_quote_res_sol) {
            Some(r)
        } else {
            None
        };
        if let Some(b) = blocked {
            *dg.blocked.entry(b).or_default() += 1;
            continue;
        }
        let eq = cash + pos.iter().map(|(cid, p)| eng.pool_value(*cid, p.tokens, c.default_fee_bps)).sum::<f64>();
        let frac = if st.kelly_fraction > 0.0 {
            let p = s[0] as f64;
            let f = st.kelly_fraction * (p / st.kelly_loss - (1.0 - p) / st.kelly_win);
            if f <= 0.0 {
                *dg.blocked.entry("kelly_negative_edge").or_default() += 1;
                continue;
            }
            f.min(st.max_bet_frac)
        } else {
            st.size_frac
        };
        let size = (frac * eq).min(st.max_impact * eng.quote_res_sol(e.coin) / 2.0);
        if size < 0.005 || cash < size + 3.0 * txc {
            *dg.blocked.entry(if size < 0.005 { "size_too_small_impact_cap" } else { "no_cash" }).or_default() += 1;
            continue;
        }
        dg.entered += 1;
        busy.insert(e.coin);
        pending.push((e.slot + c.latency_slots, Order::Buy(e.coin, (size * 1e9) as u64, s[0])));
    }
    let n = trades.len();
    let mut by: FxHashMap<&str, f64> = FxHashMap::default();
    for t in &trades {
        *by.entry(t.mint.as_str()).or_default() += t.pnl_sol;
    }
    let mut exits = std::collections::BTreeMap::new();
    for t in &trades {
        *exits.entry(t.reason).or_insert(0) += 1;
    }
    let mut pnls: Vec<f64> = trades.iter().map(|t| t.pnl_sol * sol_usd).collect();
    pnls.sort_by(|a, b| b.total_cmp(a));
    let drop = ((n as f64) * 0.01).ceil() as usize;
    let excl: Vec<f64> = pnls.iter().skip(drop.min(n)).copied().collect();
    MlReport {
        window_s: ev.window.1 - ev.window.0,
        sol_usd,
        start_sol: start,
        final_sol: cash,
        pnl_sol: cash - start,
        final_usd: cash * sol_usd,
        return_pct: (cash / start - 1.0) * 100.0,
        trades: n,
        coins_traded: by.len(),
        profitable_coins: by.values().filter(|p| **p > 0.0).count(),
        win_rate: if n > 0 { trades.iter().filter(|t| t.pnl_sol > 0.0).count() as f64 / n as f64 } else { 0.0 },
        max_drawdown_pct: max_dd * 100.0,
        failed_buys: failed,
        costs_sol: costs,
        costs_usd: costs * sol_usd,
        exits,
        held_to_5x: trades.iter().filter(|t| t.max_mult_held >= 5.0).count(),
        held_to_10x: trades.iter().filter(|t| t.max_mult_held >= 10.0).count(),
        best_ret: trades.iter().map(|t| t.ret).fold(f64::NEG_INFINITY, f64::max),
        profit_factor: {
            let gp: f64 = trades.iter().filter(|t| t.pnl_sol > 0.0).map(|t| t.pnl_sol).sum();
            let gl: f64 = -trades.iter().filter(|t| t.pnl_sol < 0.0).map(|t| t.pnl_sol).sum::<f64>();
            if gl > 0.0 { gp / gl } else if gp > 0.0 { f64::INFINITY } else { 0.0 }
        },
        pnl_usd: (cash - start) * sol_usd,
        expectancy_usd: if n > 0 { pnls.iter().sum::<f64>() / n as f64 } else { 0.0 },
        expectancy_excl_top1pct_usd: if excl.is_empty() { 0.0 } else { excl.iter().sum::<f64>() / excl.len() as f64 },
        trades_list: trades,
        diag,
    }
}

/// Grid over decision thresholds on development windows (scores must be out-of-sample for those
/// windows). Selection: maximize the worst development-window return.
pub fn tune(dev: &[(&Events, &Scores, f64)], rp: RadarParams, c: &Costs) -> (MlStrategy, Vec<(MlStrategy, Vec<f64>)>) {
    use rayon::prelude::*;
    let mut grid = Vec::new();
    // Quarter-Kelly sizing with compounding (user decision); the grid tunes entry / exit / veto settings.
    for &entry_p2 in &[0.3f32, 0.4, 0.5] {
        for &(dump_exit_min_mult, giveback_arm, giveback) in &[(1.2, 1.5, 0.35), (2.0, 2.0, 0.5)] {
            for &dev_kill_min_frac in &[0.0, 0.5] {
                for &devsell_veto in &[0.2f32, 1.0] {
                    for &exit_cont in &[0.15f32, 0.3] {
                        for &hold_if_p2_above in &[1.0f32, 0.5] {
                            for &kelly_loss in &[0.4, 0.6] {
                                grid.push(MlStrategy {
                                    entry_p2, dump_exit: 0.5, dump_exit_min_mult, giveback_arm, giveback, dev_kill_min_frac, devsell_veto,
                                    exit_cont, reentry_cooldown_s: 60, hold_if_p2_above, min_quote_res_sol: 1.0, max_positions: 15,
                                    kelly_fraction: 0.25, kelly_win: 1.0, kelly_loss, max_bet_frac: 0.10, ..Default::default()
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    // CPU-parallel over all cores (CatBoost training runs on the GPU separately).
    let all: Vec<(MlStrategy, Vec<f64>)> = grid
        .into_par_iter()
        .map(|s| (s, dev.iter().map(|(e, sc, usd)| simulate(e, rp, sc, &s, c, *usd).return_pct).collect()))
        .collect();
    let worst = |v: &Vec<f64>| v.iter().cloned().fold(f64::INFINITY, f64::min);
    let best = all.iter().max_by(|a, b| worst(&a.1).total_cmp(&worst(&b.1))).map(|x| x.0).unwrap_or_default();
    (best, all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backtest::{Ev, Events};

    fn ev(slot: u64, k: u64, user: u32, buy: bool) -> Ev {
        Ev {
            slot, order: (slot << 24) | (k << 8), time: (slot / 4) as i64, coin: 0, amm: true, buy, sol: 50_000_000,
            tokens: 1_000_000, user, creator: 999, quote_res: 100_000_000_000, token_res: 1_000_000_000_000,
            real_quote: 0, real_token: 0, fee_bps: 100, complete: false,
        }
    }

    /// Hawkes intensity: one event per slot for a long time converges to 1/(1-e^-beta); a gap decays it exactly.
    #[test]
    fn hawkes_intensity_steady_state_and_decay() {
        let events = Events::new(vec![], vec!["m".into()], (0, 0), vec![]);
        let mut eng = Engine::new(&events, RadarParams::default());
        for s in 1..=2000u64 {
            eng.apply(&ev(s, 0, s as u32, true));
        }
        let c = &eng.coins[0];
        let steady = 1.0 / (1.0 - (-HK_FAST).exp());
        assert!((c.hk_fast.0 - steady).abs() / steady < 1e-9, "{} vs {}", c.hk_fast.0, steady);
        let before = c.hk_slow.0;
        eng.apply(&ev(2100, 0, 1, true)); // 100-slot gap
        let after = eng.coins[0].hk_slow.0;
        assert!((after - (before * (-HK_SLOW * 100.0).exp() + 1.0)).abs() < 1e-9);
    }

    fn path(pts: &[(i64, f64)]) -> Vec<(i64, f64)> {
        pts.to_vec()
    }

    #[test]
    fn bins_edges() {
        assert_eq!([0.1, 0.35, 0.99, 1.0, 1.99, 2.0, 4.9, 5.0, 10.0, 49.0, 50.0, 1e6].map(bin_of), [0, 1, 1, 2, 2, 3, 3, 4, 5, 5, 6, 6]);
    }

    /// Triple barrier, -65% barrier peak class, hold barrier, end / rug labels on hand-made paths (p0 at t + 1).
    #[test]
    fn v5_labels_on_paths() {
        let cov = 1_000_000;
        // up to 2.5x, then crash to 0.3x
        let y = labels(&path(&[(0, 9.0), (101, 1.0), (200, 1.5), (300, 2.5), (400, 0.3)]), 100, &[], cov).unwrap();
        assert_eq!((y.tp1, y.bin, y.hold, y.rugdead), (1, 3, 1, 0));
        assert!((y.logmax - 2.5f32.ln()).abs() < 1e-6 && (y.end_h - 0.3).abs() < 1e-6 && (y.peak_barrier - 2.5).abs() < 1e-6);
        assert_eq!(y.t_2x, 200.0);
        // -40% first (stop barrier of tp1 hit, hold barrier not), then 3x
        let y = labels(&path(&[(101, 1.0), (150, 0.6), (300, 3.0)]), 100, &[], cov).unwrap();
        assert_eq!((y.tp1, y.hold, y.bin), (0, 1, 3));
        // -70% first: the -65% barrier freezes the peak class at the crash; later 10x only shows in logmax
        let y = labels(&path(&[(101, 1.0), (150, 0.3), (300, 10.0)]), 100, &[], cov).unwrap();
        assert_eq!((y.tp1, y.hold, y.bin), (0, 0, 0));
        assert!((y.logmax - 10f32.ln()).abs() < 1e-6);
        // slow bleed to 0.15 without ever 2x: rugdead
        let y = labels(&path(&[(101, 1.0), (200, 0.8), (300, 0.15)]), 100, &[], cov).unwrap();
        assert_eq!((y.rugdead, y.bin), (1, 1));
        // events after the horizon or after cover_end are ignored
        let y = labels(&path(&[(101, 1.0), (101 + H_S, 5.0)]), 100, &[], cov).unwrap();
        assert_eq!(y.tp1, 0);
        let y = labels(&path(&[(101, 1.0), (500, 5.0)]), 100, &[], 400).unwrap();
        assert_eq!((y.tp1, y.bin), (0, 2));
        // no price at or after t + 1
        assert!(labels(&path(&[(50, 1.0)]), 100, &[], cov).is_none());
        // death within 10 min
        assert_eq!(labels(&path(&[(101, 1.0)]), 100, &[650], cov).unwrap().dead_10m, 1);
        assert_eq!(labels(&path(&[(101, 1.0)]), 100, &[701], cov).unwrap().dead_10m, 0);
    }

    #[test]
    fn censoring_rule() {
        assert!(label_covered(0, &[], H_S));
        assert!(!label_covered(0, &[], H_S - 1));
        assert!(label_covered(0, &[500], 600), "died inside the data");
        assert!(!label_covered(0, &[700], 600), "death after the data end does not count");
        assert!(!label_covered(1000, &[500], 2000), "death before t does not cover t");
    }

    fn mk(slot: u64, time: i64, coin: u32, user: u32, buy: bool, sol: u64, q: u128, tk: u128) -> Ev {
        Ev { slot, order: (slot << 24) | ((user as u64 & 0xFFFF) << 8), time, coin, amm: true, buy, sol, tokens: 1_000, user, creator: 999,
             quote_res: q, token_res: tk, real_quote: 0, real_token: 0, fee_bps: 100, complete: false }
    }

    /// Death: real trades stop, price collapsed below 10% of the peak, 30 min pass -> one death at last_real + 1800.
    #[test]
    fn death_times_rule() {
        let evs = vec![
            mk(1, 0, 0, 1, true, 50_000_000, 100, 1),   // peak price 100
            mk(2, 10, 0, 2, false, 50_000_000, 5, 1),   // collapse to 5, last real trade at t = 10
            mk(3, 1000, 0, 3, true, 1_000, 5, 1),       // dust only
            mk(4, 5000, 0, 4, true, 50_000_000, 6, 1),  // revival (real) after the death point
        ];
        let mut ev = Events::new(evs, vec!["m".into()], (0, 10), vec![]);
        ev.cover_end = vec![20_000];
        assert_eq!(death_times(&ev), vec![vec![10 + crate::follow::DEAD_IDLE_S, 5000 + crate::follow::DEAD_IDLE_S]]);
    }

    /// Causality: a snapshot taken at event k is identical whether or not the events after k exist.
    #[test]
    fn features_do_not_see_the_future() {
        let mut evs = Vec::new();
        let mut q: u128 = 30_000_000_000;
        let mut tk: u128 = 1_000_000_000_000;
        let mut x: u64 = 7;
        for i in 0..3000u64 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let coin = ((x >> 33) % 3) as u32;
            let buy = (x >> 40) % 3 != 0;
            let sol = 5_000_000 + (x >> 20) % 900_000_000;
            if buy { q += sol as u128 / 2; tk -= tk / 400; } else { q -= q / 300; tk += tk / 500; }
            evs.push(mk(1000 + i, (i / 3) as i64, coin, ((x >> 45) % 40) as u32, buy, sol, q, tk));
        }
        let users: Vec<String> = (0..1000).map(|i| format!("w{i}")).collect();
        let full = Events::new(evs.clone(), vec!["a".into(), "b".into(), "c".into()], (0, 1000), users.clone());
        let rp = RadarParams { min_buyers: 3, min_buy_sol: 0.5, ..RadarParams::default() };
        let snaps = |ev: &Events| -> Vec<(u64, Vec<f32>)> {
            let mut eng = Engine::new(ev, rp).with_ctx(Context { wallets: vec![Some(WalletInfo { score: 0.5, n_closed: 3, ..Default::default() }); 1000], sol_usd: vec![] });
            ev.evs.iter().filter_map(|e| eng.apply(e).snapshot.map(|s| (e.order, s))).collect()
        };
        let all = snaps(&full);
        assert!(all.len() > 50, "radar must fire in the synthetic stream ({} snapshots)", all.len());
        let cut = 1700;
        let part = Events::new(evs[..cut].to_vec(), full.mints.clone(), (0, 1000), users);
        let pre = snaps(&part);
        let n = pre.len();
        assert!(n > 10);
        for (a, b) in pre.iter().zip(all.iter().take(n)) {
            assert_eq!(a.0, b.0);
            for (i, (u, v)) in a.1.iter().zip(&b.1).enumerate() {
                assert!(u == v || (u.is_nan() && v.is_nan()), "feature {} differs: {u} vs {v}", FEATURES[i]);
            }
        }
        assert_eq!(all[0].1.len(), FEATURES.len());
    }

    /// V5-13: the liquidity gate uses the real quote reserve on the curve (virtual ~30 SOL is not withdrawable).
    #[test]
    fn untradable_uses_real_quote_on_curve() {
        let events = Events::new(vec![], vec!["m".into()], (0, 0), vec![]);
        let mut eng = Engine::new(&events, RadarParams::default());
        let mut e = ev(10, 0, 1, true);
        e.amm = false;
        e.quote_res = 31_000_000_000;
        e.real_quote = 300_000_000;
        e.real_token = 700_000_000_000_000;
        eng.apply(&e);
        assert_eq!(eng.untradable(0, 0.5), Some("low_liquidity"));
        e.real_quote = 2_000_000_000;
        e.slot = 11;
        eng.apply(&e);
        assert_eq!(eng.untradable(0, 0.5), None);
        assert!((eng.exit_liquidity_sol(0) - 2.0).abs() < 1e-12);
        let mut a = ev(12, 0, 1, true);
        a.quote_res = 300_000_000;
        eng.apply(&a);
        assert_eq!(eng.untradable(0, 0.5), Some("low_liquidity"), "AMM uses the pool quote reserve");
        a.quote_res = 10_000_000_000;
        a.slot = 13;
        eng.apply(&a);
        assert_eq!(eng.untradable(0, 0.5), None);
    }

    /// Bundle marker: two distinct buyers in one transaction count as a bundle tx once the tx closes.
    #[test]
    fn same_tx_multi_buyer_is_bundle() {
        let events = Events::new(vec![], vec!["m".into()], (0, 0), vec![]);
        let mut eng = Engine::new(&events, RadarParams::default());
        let mut a = ev(10, 1, 1, true);
        let mut b = ev(10, 1, 2, true);
        b.order = a.order | 1;
        eng.apply(&a);
        eng.apply(&b);
        a = ev(11, 0, 3, true);
        eng.apply(&a); // next tx closes the previous one
        assert_eq!(eng.coins[0].tx_marks.iter().filter(|x| x.1 == 1).count(), 1);
        assert!(eng.coins[0].bundle_buy_sol > 0.09);
    }
}
