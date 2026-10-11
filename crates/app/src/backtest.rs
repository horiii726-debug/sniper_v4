//! EXPLORATORY rule-based backtest (run at the user's explicit request before the Stage 1–3 gates
//! have formally passed). Universe: pump.fun curve + PumpSwap SOL pools, the venues whose pool math is
//! validated (`pump-check`). No ML. One entry per signal (no scale-in), exit 100% (no partial TP).
//!
//! Causality: the simulator walks events in chain order. A decision taken while processing an event
//! of slot s is filled when the first event of a slot > s + latency arrives, against the pool state
//! after all events up to slot s + latency (we land last in that slot: conservative for chases).
//! Costs: venue fee from the latest event (fallback 125 bps), a fixed network cost per transaction
//! (base + priority + tip), and a deterministic pseudo-random failure rate for buys (fee paid, no fill).

use anyhow::{anyhow, Result};
use decoder::pump::{buy_tokens_out, sell_quote_out, Venue};
use decoder::txview::TxView;
use crate::radar_feed::RadarFeed;
use radar::{Params as RadarParams, Signal};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ev {
    pub slot: u64,
    pub order: u64,
    pub time: i64,
    pub coin: u32,
    pub amm: bool,
    pub buy: bool,
    pub sol: u64,
    pub tokens: u64,
    pub user: u32,
    pub creator: u32,
    pub quote_res: u128,
    pub token_res: u128,
    pub real_quote: u64,
    pub real_token: u64,
    pub fee_bps: u64,
    pub complete: bool,
}

pub struct Events {
    pub evs: Vec<Ev>,
    pub mints: Vec<String>,
    /// Block-time range of the backfill window (the decision window: signals only come from here).
    pub window: (i64, i64),
    /// Wallet address per user / creator id (cross-window wallet analysis).
    pub users: Vec<String>,
    /// Per coin: trade data is complete up to this block time (window end unless followed).
    pub cover_end: Vec<i64>,
    /// Per coin: real creation (time, price = quote/token reserves): in-window creations from the window-wide scan
    /// (`creations.bin.zst`), or the mint's first signature from `follow`.
    pub created: Vec<Option<(i64, Option<f64>)>>,
    /// The whole window was scanned for creation events (authoritative newborns possible).
    pub creations_scanned: bool,
    /// Per coin: time the coin was found dead by `follow` (V5 §3.2), if any.
    pub dead_at: Vec<Option<i64>>,
    pub followed: Vec<bool>,
}

impl Events {
    pub fn new(evs: Vec<Ev>, mints: Vec<String>, window: (i64, i64), users: Vec<String>) -> Self {
        let n = mints.len();
        Self { evs, mints, window, users, cover_end: vec![window.1; n], created: vec![None; n], creations_scanned: false, dead_at: vec![None; n], followed: vec![false; n] }
    }

    /// Last block time covered by any coin's data.
    pub fn data_end(&self) -> i64 {
        self.cover_end.iter().copied().max().unwrap_or(self.window.1).max(self.window.1)
    }
}

/// On-disk window cache (`pump_events_v2.bin`); field order is the format, do not change it.
#[derive(Serialize, Deserialize)]
struct WindowCache {
    evs: Vec<Ev>,
    mints: Vec<String>,
    window: (i64, i64),
    users: Vec<String>,
}

/// Older V4 cache (`pump_events.bin`): no wallet address table.
#[derive(Deserialize)]
struct WindowCacheV1 {
    evs: Vec<Ev>,
    mints: Vec<String>,
    window: (i64, i64),
}

/// Window events merged with the follow-up events (`<rec>/follow.bin.zst`) when present.
pub fn load(rec: &Path) -> Result<Events> {
    let mut ev = load_window(rec)?;
    let f = rec.join(crate::follow::FOLLOW_FILE);
    if f.exists() {
        crate::follow::merge(&mut ev, &crate::follow::read(&f)?)?;
    }
    Ok(ev)
}

/// Creation events of a whole window (`<rec>/creations.bin.zst`, bincode + zstd), written by the raw decode.
pub const CREATIONS_FILE: &str = "creations.bin.zst";
const CREATIONS_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreationRec {
    pub mint: String,
    pub slot: u64,
    pub order: u64,
    pub time: i64,
    pub amm: bool,
    pub migration: bool,
    pub price: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreationsFile {
    pub version: u32,
    pub window: (i64, i64),
    pub first_slot: u64,
    pub last_slot: u64,
    /// Every slot of the window was scanned.
    pub complete: bool,
    pub items: Vec<CreationRec>,
}

pub fn read_creations(path: &Path) -> Result<CreationsFile> {
    let f: CreationsFile = bincode::deserialize(&zstd::decode_all(std::fs::File::open(path)?)?)?;
    if f.version != CREATIONS_VERSION {
        return Err(anyhow!("{}: creations version {} != {CREATIONS_VERSION}", path.display(), f.version));
    }
    Ok(f)
}

fn write_creations(path: &Path, f: &CreationsFile) -> Result<u64> {
    let bytes = zstd::encode_all(&bincode::serialize(f)?[..], 3)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(bytes.len() as u64)
}

/// Mark in-window, non-migration creations of the window's coins (no-op without a matching sidecar).
fn attach_creations(ev: &mut Events, rec: &Path) -> Result<()> {
    let p = rec.join(CREATIONS_FILE);
    if !p.exists() {
        return Ok(());
    }
    let f = read_creations(&p)?;
    if f.window != ev.window {
        tracing::warn!("{}: window {:?} != events window {:?}; ignored", p.display(), f.window, ev.window);
        return Ok(());
    }
    let idx: FxHashMap<&str, usize> = ev.mints.iter().enumerate().map(|(i, m)| (m.as_str(), i)).collect();
    for it in f.items.iter().filter(|x| !x.migration) {
        if let Some(&c) = idx.get(it.mint.as_str()) {
            if ev.created[c].is_none_or(|x| it.time < x.0) {
                ev.created[c] = Some((it.time, Some(it.price)));
            }
        }
    }
    ev.creations_scanned = f.complete;
    Ok(())
}

/// Decode a recording into pump/PumpSwap events (window only), cached as `<rec>/pump_events_v2.bin`; the raw
/// decode also writes the window's creation events (`creations.bin.zst`).
pub fn load_window(rec: &Path) -> Result<Events> {
    let mut ev = load_window_events(rec)?;
    attach_creations(&mut ev, rec)?;
    Ok(ev)
}

fn load_window_events(rec: &Path) -> Result<Events> {
    let cache = rec.join("pump_events_v2.bin");
    if cache.exists() {
        let c: WindowCache = bincode::deserialize(&std::fs::read(&cache)?)?;
        return Ok(Events::new(c.evs, c.mints, c.window, c.users));
    }
    let v1 = rec.join("pump_events.bin");
    if v1.exists() {
        // V4 caches carry wallet ids only: placeholder names keep ids distinct (no cross-window wallet joins).
        let c: WindowCacheV1 = bincode::deserialize(&std::fs::read(&v1)?)?;
        let n = c.evs.iter().map(|e| e.user.max(e.creator)).max().map_or(0, |m| m as usize + 1);
        let users = (0..n).map(|i| format!("v4id{i}")).collect();
        return Ok(Events::new(c.evs, c.mints, c.window, users));
    }
    let mut dec = crate::pump_check::load_decoder()?;
    let mut creations: Vec<CreationRec> = Vec::new();
    let (mut s0, mut s1) = (u64::MAX, 0u64);
    let mut mints: FxHashMap<String, u32> = FxHashMap::default();
    let mut users: FxHashMap<String, u32> = FxHashMap::default();
    let mut evs = Vec::new();
    let (mut t0, mut t1) = (i64::MAX, i64::MIN);
    for m in recorder::Reader::open(rec)? {
        let m = m?;
        if m.source != events::Source::Rpc {
            continue;
        }
        let v: serde_json::Value = serde_json::from_slice(&m.payload)?;
        let r = &v["result"];
        if r.is_null() {
            continue;
        }
        let slot = v["params"]["slot"].as_u64().ok_or_else(|| anyhow!("no slot"))?;
        let time = r["blockTime"].as_i64().unwrap_or(0);
        t0 = t0.min(time);
        t1 = t1.max(time);
        s0 = s0.min(slot);
        s1 = s1.max(slot);
        for (i, t) in r["transactions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]).iter().enumerate() {
            let Some(tx) = TxView::parse(t) else { continue };
            for cr in dec.decode_creations(&tx) {
                creations.push(CreationRec {
                    mint: cr.mint, slot, order: (slot << 24) | ((i as u64) << 8), time: if cr.time > 0 { cr.time } else { time },
                    amm: cr.venue == Venue::Amm, migration: cr.migration, price: cr.price,
                });
            }
            for (k, e) in dec.decode(&tx).into_iter().enumerate() {
                let n = mints.len() as u32;
                let coin = *mints.entry(e.mint).or_insert(n);
                let n = users.len() as u32;
                let user = *users.entry(e.user).or_insert(n);
                let n = users.len() as u32;
                let creator = *users.entry(e.creator).or_insert(n);
                evs.push(Ev {
                    slot,
                    order: (slot << 24) | ((i as u64) << 8) | k as u64,
                    time,
                    coin,
                    amm: e.venue == Venue::Amm,
                    buy: e.buy,
                    sol: e.sol,
                    tokens: e.tokens,
                    user,
                    creator,
                    quote_res: e.quote_res,
                    token_res: e.token_res,
                    real_quote: e.real_quote,
                    real_token: e.real_token,
                    fee_bps: e.fee_bps,
                    complete: e.complete,
                });
            }
        }
    }
    evs.sort_by_key(|e| e.order);
    let mut names = vec![String::new(); mints.len()];
    for (m, i) in mints {
        names[i as usize] = m;
    }
    let mut unames = vec![String::new(); users.len()];
    for (u, i) in users {
        unames[i as usize] = u;
    }
    let out = WindowCache { evs, mints: names, window: (t0, t1), users: unames };
    creations.sort_by_key(|c| c.order);
    write_creations(&rec.join(CREATIONS_FILE), &CreationsFile { version: CREATIONS_VERSION, window: (t0, t1), first_slot: s0, last_slot: s1, complete: true, items: creations })?;
    std::fs::write(&cache, bincode::serialize(&out)?)?;
    Ok(Events::new(out.evs, out.mints, out.window, out.users))
}

/// Creation scan of a raw recording that was already decoded (writes `creations.bin.zst`). Needs the raw segments.
pub fn scan_creations(rec: &Path) -> Result<CreationsFile> {
    let mut dec = crate::pump_check::load_decoder()?;
    let (mut t0, mut t1, mut s0, mut s1) = (i64::MAX, i64::MIN, u64::MAX, 0u64);
    let mut items = Vec::new();
    let mut n_blocks = 0u64;
    for m in recorder::Reader::open(rec)? {
        let m = m?;
        if m.source != events::Source::Rpc {
            continue;
        }
        let v: serde_json::Value = serde_json::from_slice(&m.payload)?;
        let r = &v["result"];
        if r.is_null() {
            continue;
        }
        n_blocks += 1;
        let slot = v["params"]["slot"].as_u64().ok_or_else(|| anyhow!("no slot"))?;
        let time = r["blockTime"].as_i64().unwrap_or(0);
        (t0, t1, s0, s1) = (t0.min(time), t1.max(time), s0.min(slot), s1.max(slot));
        for (i, t) in r["transactions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]).iter().enumerate() {
            let Some(tx) = TxView::parse(t) else { continue };
            for cr in dec.decode_creations(&tx) {
                items.push(CreationRec {
                    mint: cr.mint, slot, order: (slot << 24) | ((i as u64) << 8), time: if cr.time > 0 { cr.time } else { time },
                    amm: cr.venue == Venue::Amm, migration: cr.migration, price: cr.price,
                });
            }
        }
    }
    if n_blocks == 0 {
        return Err(anyhow!("{}: no raw blocks (raw segments deleted?): the window stays in proxy newborn mode", rec.display()));
    }
    items.sort_by_key(|c| c.order);
    let f = CreationsFile { version: CREATIONS_VERSION, window: (t0, t1), first_slot: s0, last_slot: s1, complete: true, items };
    write_creations(&rec.join(CREATIONS_FILE), &f)?;
    Ok(f)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Strategy {
    pub entry_buyers_60s: usize,
    pub entry_max_median_buy_sol: f64,
    pub entry_min_flow_sol: f64,
    pub entry_max_buy_sell_ratio: f64,
    pub entry_min_accel: f64,
    pub entry_min_quote_res_sol: f64,
    pub size_frac: f64,
    pub max_positions: usize,
    pub max_impact: f64,
    pub hard_stop: f64,
    pub mee_fast_vs_peak: f64,
    pub mee_drawdown: f64,
    pub scout_secs: i64,
    pub inactivity_secs: i64,
    pub reentry_cooldown_secs: i64,
    /// Baseline exit instead of MEE: fixed trailing stop from the post-entry peak value.
    pub trailing_only: Option<f64>,
}

impl Default for Strategy {
    fn default() -> Self {
        Self {
            entry_buyers_60s: 10,
            entry_max_median_buy_sol: 1.0,
            entry_min_flow_sol: 0.0,
            entry_max_buy_sell_ratio: 4.0,
            entry_min_accel: 1.5,
            entry_min_quote_res_sol: 5.0,
            size_frac: 0.10,
            max_positions: 5,
            max_impact: 0.02,
            hard_stop: 0.5,
            mee_fast_vs_peak: 0.25,
            mee_drawdown: 0.4,
            scout_secs: 180,
            inactivity_secs: 90,
            reentry_cooldown_secs: 300,
            trailing_only: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Costs {
    pub latency_slots: u64,
    /// Lamports per transaction: base fee + priority fee + tip.
    pub tx_cost_lamports: u64,
    pub buy_fail_prob: f64,
    pub default_fee_bps: u64,
}

impl Default for Costs {
    fn default() -> Self {
        Self { latency_slots: 2, tx_cost_lamports: 500_000, buy_fail_prob: 0.15, default_fee_bps: 125 }
    }
}

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
    /// Withdrawable SOL (V4-4): real quote reserve on the curve, pool quote reserve on the AMM.
    fn exit_liquidity_sol(&self) -> f64 {
        (if self.amm { self.quote_res as f64 } else { self.real_quote as f64 }) / 1e9
    }
}

struct Pos {
    tokens: u64,
    cost: f64,
    entry_time: i64,
    peak_value: f64,
    peak_fast: f64,
    first_10x: bool,
    max_mult: f64,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Reason {
    DevSell,
    HardStop,
    Mee,
    Trailing,
    Scout,
    Inactivity,
    EndOfWindow,
}

enum Order {
    Buy { coin: u32, lamports: u64 },
    Sell { coin: u32, reason: Reason },
}

#[derive(Default)]
struct CoinState {
    win: VecDeque<(i64, u32, bool, f64)>,
    flow30: VecDeque<(i64, f64)>,
    fast: (f64, f64),
    slow: (f64, f64),
    first_time: Option<i64>,
    last_real_buy: i64,
    dev_sell_time: Option<i64>,
    blacklisted: bool,
    last_exit: Option<i64>,
    pending: bool,
}

const TAU_FAST: f64 = 10.0;
const TAU_SLOW: f64 = 120.0;
const REAL_MIN_SOL: f64 = 0.01;

fn ewma(e: &mut (f64, f64), t: f64, v: f64, tau: f64) {
    e.0 = e.0 * (-(t - e.1) / tau).exp() + v / tau;
    e.1 = t;
}

fn ewma_at(e: &(f64, f64), t: f64, tau: f64) -> f64 {
    e.0 * (-(t - e.1) / tau).exp()
}

/// Per-coin trace of the decision path (filled when diagnostics are on).
#[derive(Debug, Serialize, Clone, Default)]
pub struct CoinDiag {
    pub first_hot_time: Option<i64>,
    /// First condition that blocked an entry evaluation, counted per real buy while HOT and flat.
    pub blocked: std::collections::BTreeMap<&'static str, u32>,
    pub entries: Vec<(i64, f64)>,
}

#[derive(Debug, Serialize, Clone)]
pub struct TradeRec {
    pub mint: String,
    pub entry_time: i64,
    pub exit_time: i64,
    pub cost_sol: f64,
    pub proceeds_sol: f64,
    pub pnl_sol: f64,
    pub ret: f64,
    pub reason: String,
    /// Best value multiple the position reached while held (value / cost).
    pub max_mult_held: f64,
}

#[derive(Debug, Serialize, Clone)]
pub struct Report {
    pub window_s: i64,
    pub sol_usd: f64,
    pub start_sol: f64,
    pub final_sol: f64,
    pub start_usd: f64,
    pub final_usd: f64,
    pub return_pct: f64,
    pub trades: usize,
    pub wins: usize,
    pub win_rate: f64,
    pub avg_win_sol: f64,
    pub avg_loss_sol: f64,
    pub expectancy_sol: f64,
    pub expectancy_excl_top1pct_sol: f64,
    pub max_drawdown_pct: f64,
    pub failed_buys: usize,
    pub tx_costs_sol: f64,
    pub exits_by_reason: std::collections::BTreeMap<String, usize>,
    pub positions_reaching_10x: usize,
    pub best_trade_ret: f64,
    pub worst_trade_ret: f64,
    pub trades_list: Vec<TradeRec>,
    #[serde(skip)]
    pub diag: Vec<CoinDiag>,
}

fn hash01(a: u64, b: u64) -> f64 {
    let mut x = a.wrapping_mul(0x9E3779B97F4A7C15) ^ b.wrapping_mul(0xC2B2AE3D27D4EB4F);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58476D1CE4E5B9);
    x ^= x >> 29;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

pub fn simulate(ev: &Events, st: &Strategy, c: &Costs, radar_p: RadarParams, start_sol: f64, sol_usd: f64) -> Report {
    let nc = ev.mints.len();
    let mut pools: Vec<Pool> = vec![Pool::default(); nc];
    let mut cs: Vec<CoinState> = (0..nc).map(|_| CoinState::default()).collect();
    let mut feed = RadarFeed::new(ev, radar_p);
    let mut swept = Vec::new();
    let mut hot: FxHashSet<u32> = FxHashSet::default();
    let mut cash = start_sol;
    let mut pos: FxHashMap<u32, Pos> = FxHashMap::default();
    let mut pending: Vec<(u64, Order)> = Vec::new();
    let mut trades: Vec<TradeRec> = Vec::new();
    let mut failed_buys = 0usize;
    let mut tx_costs = 0.0;
    let mut peak_eq = start_sol;
    let mut max_dd = 0.0f64;
    let mut last_sweep = i64::MIN;
    let mut diag: Vec<CoinDiag> = vec![CoinDiag::default(); nc];
    let txc = c.tx_cost_lamports as f64 / 1e9;
    let fee_of = |p: &Pool| if p.fee_bps > 0 { p.fee_bps } else { c.default_fee_bps };
    let value_of = |p: &Pool, tokens: u64| -> f64 {
        if !p.known || tokens == 0 {
            return 0.0;
        }
        let mut q = sell_quote_out(p.quote_res, p.token_res, tokens, if p.fee_bps > 0 { p.fee_bps } else { c.default_fee_bps });
        if !p.amm {
            q = q.min(p.real_quote as u128);
        }
        q as f64 / 1e9
    };

    let mut i = 0usize;
    let evs = &ev.evs;
    while i <= evs.len() {
        let cur_slot = if i < evs.len() { evs[i].slot } else { u64::MAX };
        let cur_time = if i < evs.len() { evs[i].time } else { ev.window.1 + 1 };
        // 1) Execute pending orders whose target slot is complete.
        let mut k = 0;
        while k < pending.len() {
            if pending[k].0 < cur_slot {
                let (_, o) = pending.swap_remove(k);
                match o {
                    Order::Buy { coin, lamports } => {
                        let st_c = &mut cs[coin as usize];
                        st_c.pending = false;
                        let p = pools[coin as usize];
                        cash -= txc;
                        tx_costs += txc;
                        if hash01(coin as u64, cur_slot) < c.buy_fail_prob || !p.known || (p.complete && !p.amm) {
                            failed_buys += 1;
                            continue;
                        }
                        let mut toks = buy_tokens_out(p.quote_res, p.token_res, lamports, fee_of(&p));
                        if !p.amm {
                            toks = toks.min(p.real_token as u128);
                        }
                        if toks == 0 {
                            failed_buys += 1;
                            continue;
                        }
                        cash -= lamports as f64 / 1e9;
                        let cost = lamports as f64 / 1e9;
                        pos.insert(coin, Pos { tokens: toks as u64, cost, entry_time: cur_time, peak_value: cost, peak_fast: 0.0, first_10x: false, max_mult: 1.0 });
                    }
                    Order::Sell { coin, reason } => {
                        let Some(ps) = pos.remove(&coin) else { continue };
                        cs[coin as usize].pending = false;
                        cash -= txc;
                        tx_costs += txc;
                        let proceeds = value_of(&pools[coin as usize], ps.tokens);
                        cash += proceeds;
                        cs[coin as usize].last_exit = Some(cur_time);
                        if reason == Reason::DevSell {
                            cs[coin as usize].blacklisted = true;
                        }
                        let pnl = proceeds - ps.cost - 2.0 * txc;
                        trades.push(TradeRec {
                            mint: ev.mints[coin as usize].clone(),
                            entry_time: ps.entry_time - ev.window.0,
                            exit_time: cur_time - ev.window.0,
                            cost_sol: ps.cost,
                            proceeds_sol: proceeds,
                            pnl_sol: pnl,
                            ret: pnl / (ps.cost + 2.0 * txc),
                            reason: format!("{reason:?}"),
                            max_mult_held: ps.max_mult,
                        });
                    }
                }
            } else {
                k += 1;
            }
        }
        if i == evs.len() {
            // End of window: liquidate everything at the final state.
            let open: Vec<u32> = pos.keys().copied().collect();
            if open.is_empty() {
                break;
            }
            for coin in open {
                pending.push((0, Order::Sell { coin, reason: Reason::EndOfWindow }));
            }
            continue;
        }
        let e = &evs[i];
        i += 1;
        // 2) Time-based checks (inactivity, scout) once per second.
        if e.time > last_sweep {
            last_sweep = e.time;
            let ids: Vec<u32> = pos.keys().copied().collect();
            for coin in ids {
                let c_s = &cs[coin as usize];
                if c_s.pending {
                    continue;
                }
                let ps = &pos[&coin];
                let val = value_of(&pools[coin as usize], ps.tokens);
                let reason = if e.time - c_s.last_real_buy > st.inactivity_secs {
                    Some(Reason::Inactivity)
                } else if e.time - ps.entry_time > st.scout_secs && ps.peak_value < ps.cost * 1.2 && val < ps.cost {
                    Some(Reason::Scout)
                } else {
                    None
                };
                if let Some(r) = reason {
                    cs[coin as usize].pending = true;
                    pending.push((e.slot + c.latency_slots, Order::Sell { coin, reason: r }));
                }
            }
            // Equity / drawdown.
            let eq = cash + pos.iter().map(|(cid, p)| value_of(&pools[*cid as usize], p.tokens)).sum::<f64>();
            peak_eq = peak_eq.max(eq);
            max_dd = max_dd.max(1.0 - eq / peak_eq);
        }
        // 3) Apply the market event.
        let p = &mut pools[e.coin as usize];
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
        let solf = e.sol as f64 / 1e9;
        let cstate = &mut cs[e.coin as usize];
        cstate.first_time.get_or_insert(e.time);
        let real = solf >= REAL_MIN_SOL;
        if real {
            cstate.win.push_back((e.time, e.user, e.buy, solf));
            cstate.flow30.push_back((e.time, if e.buy { solf } else { -solf }));
            if e.buy {
                ewma(&mut cstate.fast, e.time as f64, 1.0, TAU_FAST);
                ewma(&mut cstate.slow, e.time as f64, 1.0, TAU_SLOW);
                cstate.last_real_buy = e.time;
            }
        }
        while cstate.win.front().is_some_and(|w| w.0 <= e.time - 60) {
            cstate.win.pop_front();
        }
        while cstate.flow30.front().is_some_and(|w| w.0 <= e.time - 30) {
            cstate.flow30.pop_front();
        }
        let dev_sell = !e.buy && e.user == e.creator && e.tokens > 0;
        if dev_sell {
            cstate.dev_sell_time = Some(e.time);
        }
        swept.clear();
        let rs = feed.step(e, &mut swept);
        for (cid, s) in swept.drain(..) {
            if s == Signal::Demote {
                hot.remove(&cid);
            }
        }
        if rs == Some(Signal::Promote) {
            hot.insert(e.coin);
            diag[e.coin as usize].first_hot_time.get_or_insert(e.time - ev.window.0);
        }
        // 4) Position management for this coin (P0 first).
        if let Some(ps) = pos.get_mut(&e.coin) {
            if !cs[e.coin as usize].pending {
                let pool = pools[e.coin as usize];
                let val = value_of(&pool, ps.tokens);
                ps.peak_value = ps.peak_value.max(val);
                ps.max_mult = ps.max_mult.max(val / ps.cost);
                if val >= 10.0 * ps.cost {
                    ps.first_10x = true;
                }
                let cst = &cs[e.coin as usize];
                let fast = ewma_at(&cst.fast, e.time as f64, TAU_FAST);
                ps.peak_fast = ps.peak_fast.max(fast);
                let flow30: f64 = cst.flow30.iter().map(|x| x.1).sum();
                let reason = if dev_sell {
                    Some(Reason::DevSell)
                } else if val <= (1.0 - st.hard_stop) * ps.cost {
                    Some(Reason::HardStop)
                } else if let Some(tr_dd) = st.trailing_only {
                    (val <= (1.0 - tr_dd) * ps.peak_value).then_some(Reason::Trailing)
                } else if (fast < st.mee_fast_vs_peak * ps.peak_fast && flow30 < 0.0)
                    || (val <= (1.0 - st.mee_drawdown) * ps.peak_value && fast < 0.5 * ps.peak_fast)
                {
                    Some(Reason::Mee)
                } else {
                    None
                };
                if let Some(r) = reason {
                    let lat = if r == Reason::DevSell { 1 } else { c.latency_slots };
                    cs[e.coin as usize].pending = true;
                    pending.push((e.slot + lat, Order::Sell { coin: e.coin, reason: r }));
                }
            }
            continue;
        }
        // 5) Entry gate (rule mode, §20.1 without ML).
        if !(real && e.buy) || !hot.contains(&e.coin) {
            continue;
        }
        let dg = &mut diag[e.coin as usize];
        let mut block = |r: &'static str| *dg.blocked.entry(r).or_default() += 1;
        if pos.len() >= st.max_positions {
            block("max_positions");
            continue;
        }
        let cst = &cs[e.coin as usize];
        if cst.blacklisted {
            block("blacklisted_after_dev_sell");
            continue;
        }
        if cst.pending {
            continue;
        }
        if cst.last_exit.is_some_and(|t| e.time - t < st.reentry_cooldown_secs) {
            block("reentry_cooldown");
            continue;
        }
        if cst.dev_sell_time.is_some_and(|t| e.time - t < 300) {
            block("dev_sold_recently");
            continue;
        }
        let pool = pools[e.coin as usize];
        if !pool.known || (pool.complete && !pool.amm) {
            block("curve_complete_waiting_amm");
            continue;
        }
        if pool.exit_liquidity_sol() < st.entry_min_quote_res_sol {
            block("low_liquidity");
            continue;
        }
        let mut buyers: Vec<u32> = Vec::new();
        let (mut bs, mut ss) = (0.0, 0.0);
        let mut sizes: Vec<f64> = Vec::new();
        for &(_, u, b, s) in &cst.win {
            if b {
                buyers.push(u);
                bs += s;
                sizes.push(s);
            } else {
                ss += s;
            }
        }
        buyers.sort_unstable();
        buyers.dedup();
        if buyers.len() < st.entry_buyers_60s {
            block("few_buyers_60s");
            continue;
        }
        if bs - ss < st.entry_min_flow_sol || bs <= ss {
            block("net_flow_not_positive");
            continue;
        }
        if ss > 0.0 && bs / ss > st.entry_max_buy_sell_ratio {
            block("buy_sell_ratio_too_high");
            continue;
        }
        sizes.sort_by(|a, b| a.total_cmp(b));
        if sizes[sizes.len() / 2] > st.entry_max_median_buy_sol {
            block("median_buy_too_big");
            continue;
        }
        let young = (e.time - cst.first_time.unwrap_or(e.time)) < TAU_SLOW as i64;
        let accel = ewma_at(&cst.fast, e.time as f64, TAU_FAST) / (ewma_at(&cst.slow, e.time as f64, TAU_SLOW) + 1e-9);
        if !young && accel < st.entry_min_accel {
            block("accel_too_low");
            continue;
        }
        // Size: fraction of equity, capped by price impact (2·size/Q approximates CPMM impact).
        let equity = cash + pos.iter().map(|(cid, p)| value_of(&pools[*cid as usize], p.tokens)).sum::<f64>();
        let q = pool.quote_res as f64 / 1e9;
        let size = (st.size_frac * equity).min(st.max_impact * q / 2.0);
        if size < 0.005 || cash < size + 3.0 * txc {
            block("no_cash_or_size_too_small");
            continue;
        }
        dg.entries.push((e.time - ev.window.0, size));
        cs[e.coin as usize].pending = true;
        pending.push((e.slot + c.latency_slots, Order::Buy { coin: e.coin, lamports: (size * 1e9) as u64 }));
    }

    let final_sol = cash;
    let n = trades.len();
    let wins: Vec<&TradeRec> = trades.iter().filter(|t| t.pnl_sol > 0.0).collect();
    let losses: Vec<&TradeRec> = trades.iter().filter(|t| t.pnl_sol <= 0.0).collect();
    let avg = |v: &[&TradeRec]| if v.is_empty() { 0.0 } else { v.iter().map(|t| t.pnl_sol).sum::<f64>() / v.len() as f64 };
    let mut pnls: Vec<f64> = trades.iter().map(|t| t.pnl_sol).collect();
    pnls.sort_by(|a, b| b.total_cmp(a));
    let drop = (n as f64 * 0.01).ceil() as usize;
    let excl: Vec<f64> = pnls.iter().skip(drop.min(n)).copied().collect();
    let mut reasons = std::collections::BTreeMap::new();
    for t in &trades {
        *reasons.entry(t.reason.clone()).or_insert(0) += 1;
    }
    Report {
        window_s: ev.window.1 - ev.window.0,
        sol_usd,
        start_sol,
        final_sol,
        start_usd: start_sol * sol_usd,
        final_usd: final_sol * sol_usd,
        return_pct: (final_sol / start_sol - 1.0) * 100.0,
        trades: n,
        wins: wins.len(),
        win_rate: if n > 0 { wins.len() as f64 / n as f64 } else { 0.0 },
        avg_win_sol: avg(&wins),
        avg_loss_sol: avg(&losses),
        expectancy_sol: if n > 0 { pnls.iter().sum::<f64>() / n as f64 } else { 0.0 },
        expectancy_excl_top1pct_sol: if excl.is_empty() { 0.0 } else { excl.iter().sum::<f64>() / excl.len() as f64 },
        max_drawdown_pct: max_dd * 100.0,
        failed_buys,
        tx_costs_sol: tx_costs,
        exits_by_reason: reasons,
        positions_reaching_10x: trades.iter().filter(|t| t.max_mult_held >= 10.0).count(),
        best_trade_ret: trades.iter().map(|t| t.ret).fold(f64::NEG_INFINITY, f64::max),
        worst_trade_ret: trades.iter().map(|t| t.ret).fold(f64::INFINITY, f64::min),
        trades_list: trades,
        diag,
    }
}

/// Marginal price after each event (quote/token reserves); curve until completion, then the AMM pool.
pub fn price_paths(ev: &Events) -> Vec<Vec<(i64, f64)>> {
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
            out[c].push((e.time - ev.window.0, e.quote_res as f64 / e.token_res as f64));
        }
    }
    out
}

/// Grid on development windows only. Selection: maximize the WORST window return (robustness),
/// fixed before looking at the test windows.
pub fn tune(dev: &[(&Events, f64)], costs: &Costs, rp: RadarParams) -> (Strategy, Vec<(Strategy, Vec<f64>)>) {
    let mut all = Vec::new();
    for &b in &[5usize, 10, 20] {
        for &med in &[0.3, 1.0] {
            for &acc in &[1.5, 3.0] {
                for &dd in &[0.3, 0.5] {
                    for &stop in &[0.3, 0.5] {
                        let s = Strategy { entry_buyers_60s: b, entry_max_median_buy_sol: med, entry_min_accel: acc, mee_drawdown: dd, hard_stop: stop, ..Default::default() };
                        let rets: Vec<f64> = dev.iter().map(|(e, usd)| simulate(e, &s, costs, rp, crate::units::start_sol(), *usd).return_pct).collect();
                        all.push((s, rets));
                    }
                }
            }
        }
    }
    let best = all
        .iter()
        .max_by(|a, b| {
            let wa = a.1.iter().cloned().fold(f64::INFINITY, f64::min);
            let wb = b.1.iter().cloned().fold(f64::INFINITY, f64::min);
            wa.total_cmp(&wb)
        })
        .map(|x| x.0)
        .unwrap_or_default();
    (best, all)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D2: the creations sidecar round-trips; attach marks in-window, non-migration creations of the window's coins
    /// and sets creations_scanned; a sidecar of another window is ignored.
    #[test]
    fn creations_sidecar_roundtrip_and_attach() {
        let d = std::env::temp_dir().join(format!("creations_test_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let ev0 = |coin: u32, time: i64| Ev { slot: 1, order: 1 << 24, time, coin, amm: false, buy: true, sol: 1, tokens: 1, user: 0, creator: 0,
            quote_res: 1, token_res: 1, real_quote: 0, real_token: 0, fee_bps: 0, complete: false };
        let cache = WindowCache { evs: vec![ev0(0, 100), ev0(1, 150)], mints: vec!["A".into(), "B".into()], window: (100, 200), users: vec!["u".into()] };
        std::fs::write(d.join("pump_events_v2.bin"), bincode::serialize(&cache).unwrap()).unwrap();
        let rec = |mint: &str, time: i64, migration: bool| CreationRec { mint: mint.into(), slot: 1, order: 1, time, amm: migration, migration, price: 2.0 };
        let f = CreationsFile { version: CREATIONS_VERSION, window: (100, 200), first_slot: 1, last_slot: 9, complete: true,
            items: vec![rec("A", 120, false), rec("B", 130, true), rec("Z", 140, false)] };
        write_creations(&d.join(CREATIONS_FILE), &f).unwrap();
        assert_eq!(read_creations(&d.join(CREATIONS_FILE)).unwrap().items, f.items);
        let ev = load_window(&d).unwrap();
        assert!(ev.creations_scanned);
        assert_eq!(ev.created, vec![Some((120, Some(2.0))), None], "migration ignored, unknown mint skipped");
        let other = CreationsFile { window: (1, 2), ..f };
        write_creations(&d.join(CREATIONS_FILE), &other).unwrap();
        let ev = load_window(&d).unwrap();
        assert!(!ev.creations_scanned && ev.created == vec![None, None]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// V4-4: exit liquidity is the real quote reserve on the curve, the pool quote reserve on the AMM.
    #[test]
    fn exit_liquidity_uses_real_quote_on_curve() {
        let curve = Pool { quote_res: 31_000_000_000, real_quote: 300_000_000, amm: false, known: true, ..Default::default() };
        assert!((curve.exit_liquidity_sol() - 0.3).abs() < 1e-12);
        let amm = Pool { quote_res: 10_000_000_000, real_quote: 0, amm: true, known: true, ..Default::default() };
        assert!((amm.exit_liquidity_sol() - 10.0).abs() < 1e-12);
    }
}
