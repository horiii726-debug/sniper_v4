//! EXPLORATORY ML pipeline (user-requested before the Stage 1–5 gates): causal feature snapshots,
//! forward labels, and a simulator that trades on model scores.
//!
//! One code path computes features for both the training snapshots and the simulator, on the same
//! cadence: an event of at least REAL_MIN_SOL on a coin the radar has flagged HOT (sticky for
//! STICKY_S after its last event), at most once per SNAP_EVERY_S per coin. Snapshot key = event order.
//! Labels use the marginal pool price path (quote/token reserves) from the first event >= t + 1 s
//! (latency) forward; they are never used as features.

use crate::backtest::{Costs, Events};
use decoder::pump::{buy_tokens_out, sell_quote_out};
use radar::{Params as RadarParams, Radar, Signal, Trade};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::Write;

const REAL_MIN_SOL: f64 = 0.01;
const SNAP_EVERY_S: i64 = 2;
const STICKY_S: i64 = 600;
const TAU_FAST: f64 = 10.0;
const TAU_SLOW: f64 = 120.0;
/// Hawkes kernel decay per slot: fast ~ 10 slots, slow ~ 100 slots.
const HK_FAST: f64 = 0.1;
const HK_SLOW: f64 = 0.01;
/// pump.fun initial real token reserves (raw, 6 decimals): 793.1M tokens.
const CURVE_INITIAL_REAL_TOKEN: f64 = 793_100_000_000_000.0;

pub const FEATURES: [&str; 72] = [
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
}

fn ewma(e: &mut (f64, f64), t: f64, tau: f64) {
    e.0 = e.0 * (-(t - e.1) / tau).exp() + 1.0 / tau;
    e.1 = t;
}

fn ewma_at(e: &(f64, f64), t: f64, tau: f64) -> f64 {
    e.0 * (-(t - e.1) / tau).exp()
}

/// Shared causal state: pools, per-coin windows, radar, wallet first-seen.
pub struct Engine {
    pools: Vec<Pool>,
    coins: Vec<Coin>,
    radar: Radar,
    first_seen: FxHashMap<u32, i64>,
    window_start: i64,
    last_sweep: i64,
    sig: Vec<(u32, Signal)>,
}

/// What the engine tells the caller after applying one event.
pub struct Step {
    pub snapshot: Option<Vec<f32>>,
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
            last_sweep: i64::MIN,
            sig: Vec::new(),
        }
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
        let c = &mut self.coins[ci];
        if c.first_time == 0 {
            c.first_time = e.time;
            c.first_price = pool.price();
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
        if e.buy && c.early.len() < 20 && !c.early.contains(&e.user) {
            c.early.push(e.user);
        }
        self.first_seen.entry(e.user).or_insert(e.time);
        c.tr.push_back((e.time, e.user, e.buy, solf));
        while c.tr.front().is_some_and(|x| x.0 <= e.time - 300) {
            c.tr.pop_front();
        }
        if !e.amm && c.on_amm {
            // late curve event after migration: ignore for the price path
        } else {
            c.px.push_back((e.time, pool.price()));
        }
        while c.px.front().is_some_and(|x| x.0 <= e.time - 300) {
            c.px.pop_front();
        }
        let real = solf >= REAL_MIN_SOL;
        if real && e.buy {
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
        if self.radar.on_trade(&tr) == Some(Signal::Promote) {
            c.hot_time.get_or_insert(e.time);
        }
        let due = real && c.hot_time.is_some() && e.time - c.last_event <= STICKY_S && e.time - c.last_snap >= SNAP_EVERY_S;
        let snapshot = if due {
            c.last_snap = e.time;
            Some(self.features(ci, e.time))
        } else {
            None
        };
        Step { snapshot, dev_sell, dev_sell_frac }
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

    /// None if tradable, otherwise the reason.
    pub fn untradable(&self, coin: u32, min_quote_sol: f64) -> Option<&'static str> {
        let p = &self.pools[coin as usize];
        if !p.known {
            Some("pool_unknown")
        } else if p.complete && !p.amm {
            Some("migrating_curve_complete")
        } else if (p.quote_res as f64 / 1e9) < min_quote_sol {
            Some("low_liquidity")
        } else {
            None
        }
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
        vec![
            (t - c.first_time) as f32,
            (c.first_time > self.window_start + 60) as u8 as f32,
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
            if c.first_price > 0.0 { (price / c.first_price) as f32 } else { 1.0 },
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
        ]
    }
}

/// Marginal price path per coin: (time, price); curve until the coin first trades on the AMM.
fn paths(ev: &Events) -> Vec<Vec<(i64, f64)>> {
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

/// Labels from time t: p0 = first price at time >= t + 1.
/// (up2, up5, up10 before -50% within 900 s; cont = +30% before -25% within 300 s; rug = <= 0.2 p0 within 300 s; fwd max mult 900 s)
fn labels(path: &[(i64, f64)], t: i64, dev_sells: &[i64]) -> Option<[f32; 12]> {
    let i0 = path.partition_point(|x| x.0 < t + 1);
    let p0 = path.get(i0)?.1;
    if p0 <= 0.0 {
        return None;
    }
    let mut up = [0f32; 3];
    let mut dead = false;
    let mut cont: Option<bool> = None;
    let mut rug = 0f32;
    let mut mx = 1.0f64;
    // Peak within 30 min WITHOUT a -50% barrier (no-stop-loss holding), and the multiple at the horizon end.
    let mut peak30 = 1.0f64;
    let mut end30 = 1.0f64;
    for &(tt, p) in &path[i0..] {
        if tt <= t + 1800 {
            peak30 = peak30.max(p / p0);
            end30 = p / p0;
        }
    }
    for &(tt, p) in &path[i0..] {
        if tt > t + 900 {
            break;
        }
        let m = p / p0;
        if !dead {
            mx = mx.max(m);
            for (k, x) in [2.0, 5.0, 10.0].iter().enumerate() {
                if m >= *x {
                    up[k] = 1.0;
                }
            }
            if m <= 0.5 {
                dead = true;
            }
        }
        if tt <= t + 300 {
            if cont.is_none() {
                if m >= 1.3 {
                    cont = Some(true);
                } else if m <= 0.75 {
                    cont = Some(false);
                }
            }
            if m <= 0.2 {
                rug = 1.0;
            }
        }
    }
    // Dump: price <= 0.8 p0 within 30 s. Dev sell: creator sells within 600 s after t.
    let dump = path[i0..].iter().take_while(|x| x.0 <= t + 30).any(|x| x.1 <= 0.8 * p0) as u8 as f32;
    let devs = dev_sells.iter().any(|&d| d > t && d <= t + 600) as u8 as f32;
    Some([
        up[0], up[1], up[2], cont.unwrap_or(false) as u8 as f32, rug, mx as f32,
        (peak30 >= 2.0) as u8 as f32, (peak30 >= 5.0) as u8 as f32, (peak30 >= 10.0) as u8 as f32, end30 as f32, dump, devs,
    ])
}

/// Training snapshots with labels for one recording.
pub fn write_snapshots(ev: &Events, rp: RadarParams, out: &std::path::Path) -> anyhow::Result<usize> {
    let mut eng = Engine::new(ev, rp);
    let pth = paths(ev);
    let mut dev_sells: Vec<Vec<i64>> = vec![Vec::new(); ev.mints.len()];
    for e in &ev.evs {
        if !e.buy && e.user == e.creator && e.tokens > 0 {
            dev_sells[e.coin as usize].push(e.time);
        }
    }
    let mut f = std::io::BufWriter::new(std::fs::File::create(out)?);
    write!(f, "coin,order,time")?;
    for n in FEATURES {
        write!(f, ",{n}")?;
    }
    writeln!(f, ",y_up2,y_up5,y_up10,y_cont,y_rug,y_fwd_max,y_peak2,y_peak5,y_peak10,y_end30,y_dump30s,y_devsell10m")?;
    let mut n = 0;
    for e in &ev.evs {
        if let Some(x) = eng.apply(e).snapshot {
            let Some(y) = labels(&pth[e.coin as usize], e.time, &dev_sells[e.coin as usize]) else { continue };
            write!(f, "{},{},{}", e.coin, e.order, e.time - ev.window.0)?;
            for v in &x {
                write!(f, ",{v}")?;
            }
            writeln!(f, ",{},{},{},{},{},{},{},{},{},{},{},{}", y[0], y[1], y[2], y[3], y[4], y[5], y[6], y[7], y[8], y[9], y[10], y[11])?;
            n += 1;
        }
    }
    f.flush()?;
    Ok(n)
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
    pub final_usd: f64,
    pub return_pct: f64,
    pub trades: usize,
    pub coins_traded: usize,
    pub profitable_coins: usize,
    pub win_rate: f64,
    pub max_drawdown_pct: f64,
    pub failed_buys: usize,
    pub costs_usd: f64,
    pub exits: std::collections::BTreeMap<&'static str, usize>,
    pub held_to_5x: usize,
    pub held_to_10x: usize,
    pub best_ret: f64,
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
    let start = 100.0 / sol_usd;
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
                    pos.insert(coin, Pos { tokens: toks as u64, cost, entry_t: cur_t, peak: cost, max_mult: 1.0, low_count: 0, low_since: None, p2 });
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
            } else if st.giveback > 0.0 && p.max_mult >= st.giveback_arm && val <= (1.0 - st.giveback) * p.peak {
                Some("GiveBackFromPeak")
            } else {
                None
            };
            if reason.is_none() {
                if let (Some(x), Some(s)) = (&step.snapshot, sc.get(&e.order)) {
                    let (p_cont, p_rug) = (s[2], s[3]);
                    // Strong reversal override (§22.3): money leaving, sellers outnumber buyers, price well off the high.
                    let strong = x[18] < -0.5 && x[41] > x[9] && x[33] < 0.6;
                    if p_rug >= st.rug_veto {
                        reason = Some("RugModel");
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
        let size = (st.size_frac * eq).min(st.max_impact * eng.quote_res_sol(e.coin) / 2.0);
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
        final_usd: cash * sol_usd,
        return_pct: (cash / start - 1.0) * 100.0,
        trades: n,
        coins_traded: by.len(),
        profitable_coins: by.values().filter(|p| **p > 0.0).count(),
        win_rate: if n > 0 { trades.iter().filter(|t| t.pnl_sol > 0.0).count() as f64 / n as f64 } else { 0.0 },
        max_drawdown_pct: max_dd * 100.0,
        failed_buys: failed,
        costs_usd: costs * sol_usd,
        exits,
        held_to_5x: trades.iter().filter(|t| t.max_mult_held >= 5.0).count(),
        held_to_10x: trades.iter().filter(|t| t.max_mult_held >= 10.0).count(),
        best_ret: trades.iter().map(|t| t.ret).fold(f64::NEG_INFINITY, f64::max),
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
    for &entry_p2 in &[0.4f32, 0.5, 0.6] {
        for &(dump_exit_min_mult, giveback_arm, giveback) in &[(1.2, 1.5, 0.35), (2.0, 2.0, 0.5), (3.0, 3.0, 0.5)] {
            for &dev_kill_min_frac in &[0.0, 0.3, 0.6] {
                for &devsell_veto in &[0.2f32, 1.0] {
                    for &exit_cont in &[0.15f32, 0.3] {
                        for &reentry_cooldown_s in &[60i64, 300] {
                            grid.push(MlStrategy {
                                entry_p2, dump_exit: 0.5, dump_exit_min_mult, giveback_arm, giveback, dev_kill_min_frac, devsell_veto,
                                exit_cont, reentry_cooldown_s, max_positions: 10, size_frac: 0.1, ..Default::default()
                            });
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
        let events = Events { evs: vec![], mints: vec!["m".into()], window: (0, 0) };
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

    /// Bundle marker: two distinct buyers in one transaction count as a bundle tx once the tx closes.
    #[test]
    fn same_tx_multi_buyer_is_bundle() {
        let events = Events { evs: vec![], mints: vec!["m".into()], window: (0, 0) };
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
