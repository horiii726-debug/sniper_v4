//! V5 Phases 6–7 (EXPLORATORY, pre-gate): strategy simulator on window + follow-up events.
//!
//! Entry (§7.2): only at radar signal rows inside the backfill window, by expected value
//!   EV = sum_k P_cal(bin_k) * R_k, where R_k = mean NET return (same cost model) of this exit policy for
//!   coins in bin k measured on training windows (`estimate_rk`, oracle mode). Because R_k is already net of
//!   fees and transaction costs, the plan's `- roundtrip_cost` term is inside R_k (not subtracted twice).
//! Slots (§7.3): ranked by EV; a candidate replaces the weakest open position when its EV beats that
//!   position's current hold EV by `replace_margin`; at most `max_entries_per_min` entries per minute.
//! Exits (§7.4): PRE_TP stop after a grace period, TP1 / TP2 partial sells, trailing exit on the bag
//!   (trail = max(bag_trail, trail_k * vol_60s), widened by 0.1 while P(hold) >= hold_min), and always-on
//!   exits: rug model, creator sell >= dev_kill_min_frac of their balance, AMM quote-reserve drop, max hold,
//!   hazard. No EndOfWindow sell: a position lives until an exit or the coin's data ends (`DataEnd`).
//! Costs (§8): venue fee from each pool, per-transaction base + priority + tip (partial sells pay their own),
//!   deterministic pseudo-random buy failures (fee paid, no fill), fills after `latency_slots`.

use crate::backtest::Events;
use crate::ml_sim::{self, Context, Engine, Labels, FEATURES};
use radar::Params as RadarParams;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Minimum real exit liquidity (SOL) to enter (curve: real quote reserve, V5-13).
const MIN_EXIT_LIQ_SOL: f64 = 0.5;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct StratV5 {
    pub ev_min: f64,
    pub rug_veto: f32,
    pub max_ret_60s_entry: f32,
    pub max_top10_share: f32,
    pub max_bundle_share: f32,
    pub max_positions: usize,
    pub replace_margin: f64,
    pub max_entries_per_min: usize,
    pub bet_sol: f64,
    pub max_total_exposure: f64,
    pub pre_tp_stop: f64,
    pub stop_grace_s: i64,
    pub tp1_mult: f64,
    pub tp1_sell_frac: f64,
    pub tp2_mult: f64,
    pub tp2_sell_frac: f64,
    pub bag_trail: f64,
    pub trail_k: f64,
    pub hold_min: f32,
    /// DevSell exit: cumulative tokens sold by the creator family >= this fraction of its peak observed holdings.
    pub dev_kill_min_frac: f64,
    pub lp_drop_frac: f64,
    pub dead_exit: f32,
    pub max_hold_s: i64,
}

impl Default for StratV5 {
    /// Starting values of V5 plan §7.5.
    fn default() -> Self {
        Self {
            ev_min: 0.15, rug_veto: 0.5, max_ret_60s_entry: 0.5, max_top10_share: 0.5, max_bundle_share: 0.3,
            max_positions: 18, replace_margin: 0.15, max_entries_per_min: 3, bet_sol: 0.05, max_total_exposure: 0.6,
            pre_tp_stop: 0.35, stop_grace_s: 20, tp1_mult: 2.0, tp1_sell_frac: 0.5, tp2_mult: 5.0, tp2_sell_frac: 0.25,
            bag_trail: 0.55, trail_k: 3.0, hold_min: 0.4, dev_kill_min_frac: 0.5, lp_drop_frac: 0.6, dead_exit: 0.7, max_hold_s: 36_000,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CostsV5 {
    pub latency_slots: u64,
    pub tx_base_lamports: u64,
    pub priority_lamports: u64,
    pub tip_lamports: u64,
    pub buy_fail_prob: f64,
    pub default_fee_bps: u64,
    /// Seed of the deterministic buy-failure draws (Monte Carlo varies it).
    pub fail_seed: u64,
    /// Starting capital, SOL.
    pub start_sol: f64,
}

impl Default for CostsV5 {
    fn default() -> Self {
        // 500,000 lamports per transaction in total, as in V4 (base 5,000 + priority 195,000 + tip 300,000).
        Self { latency_slots: 2, tx_base_lamports: 5_000, priority_lamports: 195_000, tip_lamports: 300_000, buy_fail_prob: 0.15, default_fee_bps: 125, fail_seed: 0, start_sol: crate::units::start_sol() }
    }
}

impl CostsV5 {
    pub fn tx_sol(&self) -> f64 {
        (self.tx_base_lamports + self.priority_lamports + self.tip_lamports) as f64 / 1e9
    }
}

/// Calibrated model outputs for one snapshot row.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pred {
    pub bin: [f32; 7],
    pub tp1: f32,
    pub rug: f32,
    pub hold: f32,
    pub dead: f32,
}

/// Source of model predictions at snapshot rows (score file from Python, or Rust inference).
pub trait Scorer {
    fn score(&mut self, order: u64, x: &[f32]) -> Option<Pred>;
}

/// Predictions read from `scores_<W>.csv.zst` (columns by name; see python/ml/v5_train.py).
pub struct FileScores(pub FxHashMap<u64, Pred>);

impl FileScores {
    pub fn read(path: &std::path::Path) -> anyhow::Result<Self> {
        let raw = zstd::decode_all(std::fs::File::open(path)?)?;
        let text = String::from_utf8(raw)?;
        let mut lines = text.lines();
        let hdr: Vec<&str> = lines.next().unwrap_or("").split(',').collect();
        let col = |n: &str| hdr.iter().position(|h| *h == n).ok_or_else(|| anyhow::anyhow!("{}: no column {n}", path.display()));
        let io = col("order")?;
        let ib: Vec<usize> = (0..7).map(|k| col(&format!("p_bin{k}"))).collect::<anyhow::Result<_>>()?;
        let (it, ir, ih, id) = (col("p_tp1")?, col("p_rug")?, col("p_hold")?, col("p_dead")?);
        let mut m = FxHashMap::default();
        for l in lines {
            let v: Vec<&str> = l.split(',').collect();
            let f = |i: usize| v.get(i).and_then(|x| x.parse::<f32>().ok()).unwrap_or(f32::NAN);
            let mut bin = [0f32; 7];
            for (k, i) in ib.iter().enumerate() {
                bin[k] = f(*i);
            }
            m.insert(v[io].parse::<u64>()?, Pred { bin, tp1: f(it), rug: f(ir), hold: f(ih), dead: f(id) });
        }
        Ok(Self(m))
    }
}

impl Scorer for FileScores {
    fn score(&mut self, order: u64, _x: &[f32]) -> Option<Pred> {
        self.0.get(&order).copied()
    }
}

/// Borrowed score file (shared across parallel simulations).
pub struct FileScoresRef<'a>(pub &'a FileScores);

impl Scorer for FileScoresRef<'_> {
    fn score(&mut self, order: u64, _x: &[f32]) -> Option<Pred> {
        self.0 .0.get(&order).copied()
    }
}

/// No model: neutral predictions (used by the oracle run that measures R_k).
pub struct NoScores;

impl Scorer for NoScores {
    fn score(&mut self, _order: u64, _x: &[f32]) -> Option<Pred> {
        Some(Pred { bin: [1.0 / 7.0; 7], tp1: 0.5, rug: 0.0, hold: 0.0, dead: 0.0 })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum Stage {
    PreTp,
    Bag1,
    Bag2,
}

struct Pos {
    tokens0: u64,
    tokens: u64,
    cost: f64,
    entry_t: i64,
    /// Average fill price, lamports per raw token (same unit as `Engine::price`).
    entry_px: f64,
    peak_px: f64,
    max_mult: f64,
    stage: Stage,
    proceeds: f64,
    tx_paid: f64,
    legs: Vec<Leg>,
    last: Pred,
    last_vol: f64,
    entry_ev: f64,
    entry_label: Option<Labels>,
    /// (time, quote reserve) within 60 s, AMM pools only.
    q_hist: VecDeque<(i64, f64)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Leg {
    pub t: i64,
    pub reason: &'static str,
    pub tokens_frac: f64,
    pub proceeds_sol: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TradeV5 {
    pub mint: String,
    pub entry_t: i64,
    pub exit_t: i64,
    pub cost_sol: f64,
    pub proceeds_sol: f64,
    pub tx_sol: f64,
    pub pnl_sol: f64,
    pub ret: f64,
    pub max_mult: f64,
    pub final_reason: &'static str,
    pub entry_ev: f64,
    pub true_bin: Option<u8>,
    pub true_logmax: Option<f32>,
    pub legs: Vec<Leg>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReportV5 {
    pub window_s: i64,
    pub data_s: i64,
    pub sol_usd: f64,
    pub start_sol: f64,
    pub final_sol: f64,
    pub return_pct: f64,
    pub trades: usize,
    pub win_rate: f64,
    pub profit_factor: f64,
    pub expectancy_sol: f64,
    pub expectancy_excl_top1pct_sol: f64,
    pub max_drawdown_pct: f64,
    pub costs_sol: f64,
    pub failed_buys: usize,
    pub exits_by_reason: std::collections::BTreeMap<&'static str, usize>,
    pub final_reasons: std::collections::BTreeMap<&'static str, usize>,
    pub runners_10x: usize,
    pub runners_10x_captured_5x: usize,
    pub moonshot_capture: f64,
    pub signals_in_window: usize,
    pub blocked: std::collections::BTreeMap<&'static str, usize>,
    pub trades_list: Vec<TradeV5>,
    /// Per signal row: (coin, first blocking reason or "entered").
    #[serde(skip)]
    pub signal_outcomes: Vec<(u32, &'static str)>,
}

fn hash01(a: u64, b: u64, seed: u64) -> f64 {
    let mut x = a.wrapping_mul(0x9E3779B97F4A7C15) ^ b.wrapping_mul(0xC2B2AE3D27D4EB4F) ^ seed.wrapping_mul(0xD6E8FEB86659FD93);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58476D1CE4E5B9);
    x ^= x >> 29;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

enum Order {
    Buy { coin: u32, lamports: u64, ev: f64, label: Option<Labels> },
    Sell { coin: u32, tokens: u64, reason: &'static str, close: bool },
}

/// Per-window inputs that do not depend on the strategy (precompute once, reuse across a grid).
pub struct WindowData<'a> {
    pub name: String,
    pub ev: &'a Events,
    pub ctx: Context,
    pub sol_usd: f64,
    pub paths: Vec<Vec<(i64, f64)>>,
    pub deaths: Vec<Vec<i64>>,
}

impl<'a> WindowData<'a> {
    pub fn new(name: String, ev: &'a Events, ctx: Context, sol_usd: f64) -> Self {
        Self { name, paths: ml_sim::paths(ev), deaths: ml_sim::death_times(ev), ev, ctx, sol_usd }
    }
}

struct Idx {
    ret_60s: usize,
    top10: usize,
    bundle: usize,
    vol: usize,
}

fn idx() -> Idx {
    let f = |n: &str| FEATURES.iter().position(|x| *x == n).unwrap_or_else(|| panic!("feature {n}"));
    Idx { ret_60s: f("ret_60s"), top10: f("top10_supply_share"), bundle: f("f2_bundle_buy_share"), vol: f("vol_60s") }
}

/// Simulate one window. `rk` = net return per entry bin (from `estimate_rk`). `oracle` = enter every
/// in-window signal with a fixed bet, no gates, unlimited cash and slots (used to measure R_k).
#[allow(clippy::too_many_arguments)]
pub fn simulate(w: &WindowData, rp: RadarParams, scorer: &mut dyn Scorer, st: &StratV5, c: &CostsV5, rk: &[f64; 7], oracle: bool) -> ReportV5 {
    let ev = w.ev;
    let ix = idx();
    let mut eng = Engine::new(ev, rp).with_ctx(w.ctx.clone());
    let start = c.start_sol;
    let mut cash = start;
    let txc = c.tx_sol();
    let mut pos: FxHashMap<u32, Pos> = FxHashMap::default();
    let mut pending: Vec<(u64, Order)> = Vec::new();
    let mut busy: FxHashSet<u32> = FxHashSet::default();
    let mut pending_buy_cost = 0.0;
    let mut blacklist: FxHashSet<u32> = FxHashSet::default();
    let mut entries: VecDeque<i64> = VecDeque::new();
    let mut trades: Vec<TradeV5> = Vec::new();
    let (mut failed, mut costs) = (0usize, 0.0);
    let (mut peak_eq, mut max_dd) = (start, 0.0f64);
    let mut last_sec = i64::MIN;
    let mut exits: std::collections::BTreeMap<&'static str, usize> = Default::default();
    let mut blocked: std::collections::BTreeMap<&'static str, usize> = Default::default();
    let mut signals = 0usize;
    let mut outcomes: Vec<(u32, &'static str)> = Vec::new();
    // Runners for moonshot capture: coins with an in-window signal whose label says >= 10x within H.
    let mut runner: FxHashSet<u32> = FxHashSet::default();
    let mut captured: FxHashSet<u32> = FxHashSet::default();
    let ev_of = |p: &Pred| -> f64 { p.bin.iter().zip(rk).map(|(a, b)| *a as f64 * b).sum() };
    // Current EV of holding: +100% before -50% (the hold label) as a two-outcome bet.
    let hold_ev = |p: &Pred| -> f64 { p.hold as f64 * 1.0 - (1.0 - p.hold as f64) * 0.5 };
    let evs = &ev.evs;
    let mut i = 0usize;
    loop {
        let (cur_slot, cur_t) = if i < evs.len() { (evs[i].slot, evs[i].time) } else { (u64::MAX, ev.data_end() + 1) };
        // 1) Fill due orders: sells first so replacement proceeds fund the buy.
        let mut due: Vec<Order> = Vec::new();
        let mut k = 0;
        while k < pending.len() {
            if pending[k].0 < cur_slot {
                due.push(pending.swap_remove(k).1);
            } else {
                k += 1;
            }
        }
        due.sort_by_key(|o| matches!(o, Order::Buy { .. }));
        for o in due {
            match o {
                Order::Sell { coin, tokens, reason, close } => {
                    let Some(p) = pos.get_mut(&coin) else { continue };
                    let n = tokens.min(p.tokens);
                    let got = eng.pool_value(coin, n, c.default_fee_bps);
                    cash += got - txc;
                    costs += txc;
                    p.tx_paid += txc;
                    p.tokens -= n;
                    p.proceeds += got;
                    p.legs.push(Leg { t: cur_t - ev.window.0, reason, tokens_frac: n as f64 / p.tokens0.max(1) as f64, proceeds_sol: got });
                    *exits.entry(reason).or_default() += 1;
                    if close || p.tokens == 0 {
                        busy.remove(&coin);
                        let p = pos.remove(&coin).expect("present");
                        if reason == "DevSell" || reason == "RugModel" {
                            blacklist.insert(coin);
                        }
                        let pnl = p.proceeds - p.cost - p.tx_paid;
                        trades.push(TradeV5 {
                            mint: ev.mints[coin as usize].clone(), entry_t: p.entry_t - ev.window.0, exit_t: cur_t - ev.window.0, cost_sol: p.cost,
                            proceeds_sol: p.proceeds, tx_sol: p.tx_paid, pnl_sol: pnl, ret: pnl / p.cost, max_mult: p.max_mult, final_reason: reason,
                            entry_ev: p.entry_ev, true_bin: p.entry_label.map(|l| l.bin), true_logmax: p.entry_label.map(|l| l.logmax), legs: p.legs,
                        });
                    } else {
                        busy.remove(&coin);
                    }
                }
                Order::Buy { coin, lamports, ev: e_ev, label } => {
                    busy.remove(&coin);
                    let size = lamports as f64 / 1e9;
                    pending_buy_cost -= size;
                    cash -= txc;
                    costs += txc;
                    let toks = eng.fill_buy(coin, lamports, c.default_fee_bps);
                    if hash01(coin as u64, cur_slot, c.fail_seed) < c.buy_fail_prob || toks == 0 || (!oracle && cash < size) {
                        failed += 1;
                        continue;
                    }
                    cash -= size;
                    let px = eng.price(coin);
                    pos.insert(coin, Pos {
                        tokens0: toks as u64, tokens: toks as u64, cost: size, entry_t: cur_t, entry_px: lamports as f64 / toks as f64, peak_px: px, max_mult: 1.0,
                        stage: Stage::PreTp, proceeds: 0.0, tx_paid: txc, legs: Vec::new(), last: Pred::default(), last_vol: 0.0, entry_ev: e_ev,
                        entry_label: label, q_hist: VecDeque::new(),
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
                let t = pos[&coin].tokens;
                pending.push((0, Order::Sell { coin, tokens: t, reason: "DataEnd", close: true }));
            }
            continue;
        }
        let e = &evs[i];
        i += 1;
        let step = eng.apply(e);
        // 2) Once per second: max hold, data end of a coin, equity.
        if e.time > last_sec {
            last_sec = e.time;
            let ids: Vec<u32> = pos.keys().copied().filter(|c| !busy.contains(c)).collect();
            for coin in ids {
                let p = &pos[&coin];
                let reason = if e.time - p.entry_t >= st.max_hold_s {
                    Some("MaxHold")
                } else if e.time > ev.cover_end[coin as usize] {
                    Some("DataEnd")
                } else {
                    None
                };
                if let Some(r) = reason {
                    busy.insert(coin);
                    pending.push((e.slot + c.latency_slots, Order::Sell { coin, tokens: p.tokens, reason: r, close: true }));
                }
            }
            let eq = cash + pos.iter().map(|(cid, p)| eng.pool_value(*cid, p.tokens, c.default_fee_bps)).sum::<f64>();
            peak_eq = peak_eq.max(eq);
            if peak_eq > 0.0 {
                max_dd = max_dd.max(1.0 - eq / peak_eq);
            }
        }
        let pred = step.snapshot.as_ref().and_then(|x| scorer.score(e.order, x));
        // 3) Open position on this coin.
        if pos.contains_key(&e.coin) {
            if busy.contains(&e.coin) {
                continue;
            }
            let price = eng.price(e.coin);
            let qres = eng.quote_res_sol(e.coin);
            let is_amm = eng.is_amm(e.coin);
            let p = pos.get_mut(&e.coin).expect("present");
            let val = eng.pool_value(e.coin, p.tokens, c.default_fee_bps);
            p.peak_px = p.peak_px.max(price);
            let m = if p.entry_px > 0.0 { price / p.entry_px } else { 1.0 };
            p.max_mult = p.max_mult.max(m);
            if p.max_mult >= 5.0 {
                captured.insert(e.coin);
            }
            if let (Some(pr), Some(x)) = (pred, step.snapshot.as_ref()) {
                p.last = pr;
                p.last_vol = x[ix.vol] as f64;
            }
            if is_amm {
                p.q_hist.push_back((e.time, qres));
                while p.q_hist.len() > 1 && p.q_hist[1].0 <= e.time - 60 {
                    p.q_hist.pop_front();
                }
            }
            let lp_drop = is_amm && p.q_hist.front().is_some_and(|(_, q0)| *q0 > 0.0 && qres < (1.0 - st.lp_drop_frac) * q0);
            let mut sell: Option<(u64, &'static str, bool)> = None;
            if step.dev_family_sell && step.dev_cum_sell_frac >= st.dev_kill_min_frac {
                sell = Some((p.tokens, "DevSell", true));
            } else if pred.is_some_and(|q| q.rug >= st.rug_veto) {
                sell = Some((p.tokens, "RugModel", true));
            } else if pred.is_some_and(|q| q.dead >= st.dead_exit) {
                sell = Some((p.tokens, "Hazard", true));
            } else if lp_drop {
                sell = Some((p.tokens, "LpDrop", true));
            } else {
                match p.stage {
                    Stage::PreTp => {
                        if m >= st.tp1_mult {
                            let n = ((p.tokens0 as f64 * st.tp1_sell_frac) as u64).min(p.tokens);
                            p.stage = Stage::Bag1;
                            sell = Some((n, "TP1", n >= p.tokens));
                        } else if e.time - p.entry_t >= st.stop_grace_s && val <= (1.0 - st.pre_tp_stop) * p.cost {
                            sell = Some((p.tokens, "Stop", true));
                        }
                    }
                    Stage::Bag1 | Stage::Bag2 => {
                        if p.stage == Stage::Bag1 && m >= st.tp2_mult {
                            let n = ((p.tokens0 as f64 * st.tp2_sell_frac) as u64).min(p.tokens);
                            p.stage = Stage::Bag2;
                            sell = Some((n, "TP2", n >= p.tokens));
                        } else {
                            let mut trail = st.bag_trail.max(st.trail_k * p.last_vol);
                            if p.last.hold >= st.hold_min {
                                trail += 0.1;
                            }
                            if price <= p.peak_px * (1.0 - trail.min(0.99)) {
                                sell = Some((p.tokens, "Trail", true));
                            }
                        }
                    }
                }
            }
            if let Some((n, r, close)) = sell {
                if n > 0 {
                    busy.insert(e.coin);
                    let lat = if r == "DevSell" { 1 } else { c.latency_slots };
                    pending.push((e.slot + lat, Order::Sell { coin: e.coin, tokens: n, reason: r, close }));
                }
            }
            continue;
        }
        // 4) Entry: radar signal rows inside the decision window only.
        if !step.signal || e.time > ev.window.1 {
            continue;
        }
        let Some(x) = step.snapshot.as_ref() else { continue };
        signals += 1;
        let ci = e.coin as usize;
        let label = ml_sim::labels(&w.paths[ci], e.time, &w.deaths[ci], ev.cover_end[ci]);
        if label.is_some_and(|l| l.logmax >= 10f32.ln()) {
            runner.insert(e.coin);
        }
        let coin_id = e.coin;
        let mut block = |r: &'static str| {
            *blocked.entry(r).or_default() += 1;
            outcomes.push((coin_id, r));
        };
        if busy.contains(&e.coin) {
            block("order_pending");
            continue;
        }
        if blacklist.contains(&e.coin) {
            block("blacklisted");
            continue;
        }
        if let Some(r) = eng.untradable(e.coin, MIN_EXIT_LIQ_SOL) {
            block(r);
            continue;
        }
        let Some(pr) = pred else {
            block("no_score");
            continue;
        };
        let e_ev = ev_of(&pr);
        if !oracle {
            let gate = if e_ev < st.ev_min {
                Some("ev_below_min")
            } else if pr.rug >= st.rug_veto {
                Some("rug_veto")
            } else if x[ix.ret_60s] > st.max_ret_60s_entry {
                Some("ret_60s_too_high")
            } else if x[ix.top10] > st.max_top10_share {
                Some("top10_share_too_high")
            } else if x[ix.bundle] > st.max_bundle_share {
                Some("bundle_share_too_high")
            } else {
                None
            };
            if let Some(g) = gate {
                block(g);
                continue;
            }
            while entries.front().is_some_and(|t| *t <= e.time - 60) {
                entries.pop_front();
            }
            if entries.len() >= st.max_entries_per_min {
                block("max_entries_per_min");
                continue;
            }
            let open_cost: f64 = pos.values().map(|p| p.cost * p.tokens as f64 / p.tokens0.max(1) as f64).sum::<f64>() + pending_buy_cost;
            let eq = cash + pos.iter().map(|(cid, p)| eng.pool_value(*cid, p.tokens, c.default_fee_bps)).sum::<f64>();
            if open_cost + st.bet_sol > st.max_total_exposure * eq {
                block("exposure_cap");
                continue;
            }
            let n_busy_buys = busy.iter().filter(|b| !pos.contains_key(b)).count();
            if pos.len() + n_busy_buys >= st.max_positions {
                // Replace the weakest open position (by current hold EV) if the candidate is clearly better.
                let weakest = pos.iter().filter(|(cid, _)| !busy.contains(cid)).map(|(cid, p)| (*cid, hold_ev(&p.last))).min_by(|a, b| a.1.total_cmp(&b.1));
                match weakest {
                    Some((wc, wev)) if e_ev > wev + st.replace_margin => {
                        busy.insert(wc);
                        let t = pos[&wc].tokens;
                        pending.push((e.slot + c.latency_slots, Order::Sell { coin: wc, tokens: t, reason: "Replaced", close: true }));
                    }
                    _ => {
                        block("slots_full");
                        continue;
                    }
                }
            } else if cash < st.bet_sol + 3.0 * txc {
                block("no_cash");
                continue;
            }
            entries.push_back(e.time);
        }
        outcomes.push((e.coin, "entered"));
        busy.insert(e.coin);
        pending_buy_cost += st.bet_sol;
        pending.push((e.slot + c.latency_slots, Order::Buy { coin: e.coin, lamports: (st.bet_sol * 1e9) as u64, ev: e_ev, label }));
    }
    let n = trades.len();
    let mut pnls: Vec<f64> = trades.iter().map(|t| t.pnl_sol).collect();
    pnls.sort_by(|a, b| b.total_cmp(a));
    let drop = (n as f64 * 0.01).ceil() as usize;
    let excl: Vec<f64> = pnls.iter().skip(drop.min(n)).copied().collect();
    let gp: f64 = trades.iter().filter(|t| t.pnl_sol > 0.0).map(|t| t.pnl_sol).sum();
    let gl: f64 = -trades.iter().filter(|t| t.pnl_sol < 0.0).map(|t| t.pnl_sol).sum::<f64>();
    let mut final_reasons: std::collections::BTreeMap<&'static str, usize> = Default::default();
    for t in &trades {
        *final_reasons.entry(t.final_reason).or_default() += 1;
    }
    let caught = runner.iter().filter(|c| captured.contains(c)).count();
    ReportV5 {
        window_s: ev.window.1 - ev.window.0,
        data_s: ev.data_end() - ev.window.0,
        sol_usd: w.sol_usd,
        start_sol: start,
        final_sol: cash,
        return_pct: (cash / start - 1.0) * 100.0,
        trades: n,
        win_rate: if n > 0 { trades.iter().filter(|t| t.pnl_sol > 0.0).count() as f64 / n as f64 } else { 0.0 },
        profit_factor: if gl > 0.0 { gp / gl } else if gp > 0.0 { f64::INFINITY } else { 0.0 },
        expectancy_sol: if n > 0 { pnls.iter().sum::<f64>() / n as f64 } else { 0.0 },
        expectancy_excl_top1pct_sol: if excl.is_empty() { 0.0 } else { excl.iter().sum::<f64>() / excl.len() as f64 },
        max_drawdown_pct: max_dd * 100.0,
        costs_sol: costs,
        failed_buys: failed,
        exits_by_reason: exits,
        final_reasons,
        runners_10x: runner.len(),
        runners_10x_captured_5x: caught,
        moonshot_capture: if runner.is_empty() { 0.0 } else { caught as f64 / runner.len() as f64 },
        signals_in_window: signals,
        blocked,
        trades_list: trades,
        signal_outcomes: outcomes,
    }
}

/// R_k (§7.2): mean net return of this exit policy per true entry bin, from oracle runs (every in-window signal
/// entered, fixed bet) on TRAINING windows only. Each training window uses its own out-of-sample scores when
/// given (so model exits act as in the test), else no model (price exits only). Bins with few trades are shrunk
/// toward the pooled mean (prior weight 5 trades). Returns (R, trades per bin).
pub fn estimate_rk(train: &[(&WindowData, Option<&FileScores>)], rp: RadarParams, st: &StratV5, c: &CostsV5) -> ([f64; 7], [usize; 7]) {
    use rayon::prelude::*;
    let reps: Vec<ReportV5> = train.par_iter().map(|(w, sc)| match sc {
        Some(sc) => simulate(w, rp, &mut FileScoresRef(sc), st, c, &[0.0; 7], true),
        None => simulate(w, rp, &mut NoScores, st, c, &[0.0; 7], true),
    }).collect();
    let (mut sum, mut cnt) = ([0f64; 7], [0usize; 7]);
    let (mut all_s, mut all_n) = (0.0, 0usize);
    for r in &reps {
        for t in &r.trades_list {
            if let Some(b) = t.true_bin {
                sum[b as usize] += t.ret;
                cnt[b as usize] += 1;
                all_s += t.ret;
                all_n += 1;
            }
        }
    }
    let prior = if all_n > 0 { all_s / all_n as f64 } else { -1.0 };
    let mut rk = [0f64; 7];
    for k in 0..7 {
        rk[k] = (sum[k] + 5.0 * prior) / (cnt[k] as f64 + 5.0);
    }
    (rk, cnt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backtest::Ev;

    struct Fixed(Pred);
    impl Scorer for Fixed {
        fn score(&mut self, _o: u64, _x: &[f32]) -> Option<Pred> {
            Some(self.0)
        }
    }

    fn mk(slot: u64, time: i64, user: u32, buy: bool, sol: u64, q: u128, tk: u128) -> Ev {
        Ev { slot, order: (slot << 24) | ((user as u64 & 0xFFFF) << 8), time, coin: 0, amm: true, buy, sol, tokens: 1_000_000, user, creator: 999_999,
             quote_res: q, token_res: tk, real_quote: 0, real_token: 0, fee_bps: 100, complete: false }
    }

    /// A coin that pumps 10x after the signal: the exit ladder sells TP1 at 2x, TP2 at 5x and trails the bag;
    /// the trade is profitable, legs sum to the whole position, and costs are charged per transaction.
    #[test]
    fn ladder_on_a_runner() {
        let mut evs = Vec::new();
        let tk: u128 = 1_000_000_000_000_000;
        let mut slot = 100;
        // 40 s of steady buying by many wallets at a flat-ish price (radar promotes), then a 10x pump, then a dump.
        for s in 0..40i64 {
            for u in 0..3u32 {
                slot += 1;
                evs.push(mk(slot, s, (s as u32) * 3 + u, true, 300_000_000, 50_000_000_000 + (s as u128) * 100_000_000, tk));
            }
        }
        let mut q: u128 = 54_000_000_000;
        for s in 40..400i64 {
            slot += 1;
            q = (q as f64 * 1.012) as u128;
            evs.push(mk(slot, s, 10_000 + s as u32, true, 300_000_000, q, tk));
        }
        for s in 400..500i64 {
            slot += 1;
            q = (q as f64 * 0.97) as u128;
            evs.push(mk(slot, s, 20_000 + s as u32, false, 300_000_000, q, tk));
        }
        let mut ev = Events::new(evs, vec!["M".into()], (0, 120), vec![]);
        ev.cover_end = vec![100_000];
        let rp = RadarParams { min_buyers: 5, min_buy_sol: 1.0, max_median_buy_sol: 1.0, ..RadarParams::default() };
        let w = WindowData::new("t".into(), &ev, Context::default(), 200.0);
        let pr = Pred { bin: [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], tp1: 0.9, rug: 0.0, hold: 0.0, dead: 0.0 };
        let rk = [0.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0];
        let c = CostsV5 { buy_fail_prob: 0.0, ..Default::default() };
        let st = StratV5 { max_ret_60s_entry: 10.0, max_top10_share: 1.0, max_bundle_share: 1.0, ..Default::default() };
        let r = simulate(&w, rp, &mut Fixed(pr), &st, &c, &rk, false);
        assert!(r.signals_in_window >= 1, "{r:?}");
        assert_eq!(r.trades, 1, "{:?}", r.blocked);
        let t = &r.trades_list[0];
        let reasons: Vec<&str> = t.legs.iter().map(|l| l.reason).collect();
        assert_eq!(reasons, ["TP1", "TP2", "Trail"], "{t:?}");
        // Price units: the pump (+1.2%/s from t = 40) doubles the quote reserve after ~58 s and x5 after ~135 s,
        // so TP1 cannot fire before ~t = 95 and TP2 not before ~t = 170; the peak multiple is ~(1.012^360) / fees.
        assert!(t.legs[0].t >= 90, "TP1 too early: {t:?}");
        assert!(t.legs[1].t >= 165, "TP2 too early: {t:?}");
        assert!(t.max_mult > 5.0 && t.max_mult < 100.0, "multiple in wrong units: {}", t.max_mult);
        let frac: f64 = t.legs.iter().map(|l| l.tokens_frac).sum();
        assert!((frac - 1.0).abs() < 1e-9);
        assert!(t.pnl_sol > 0.0 && t.max_mult >= 5.0);
        assert!((t.tx_sol - 4.0 * c.tx_sol()).abs() < 1e-12, "1 buy + 3 sell legs pay their own tx");
        assert!((r.final_sol - (r.start_sol + t.pnl_sol)).abs() < 1e-9, "cash accounting");
    }

    /// Runner stream of `ladder_on_a_runner` (steady buying 0-40 s, then +1.2%/s until 400 s).
    fn runner_events() -> Vec<Ev> {
        let mut evs = Vec::new();
        let tk: u128 = 1_000_000_000_000_000;
        let mut slot = 100;
        for s in 0..40i64 {
            for u in 0..3u32 {
                slot += 1;
                evs.push(mk(slot, s, (s as u32) * 3 + u, true, 300_000_000, 50_000_000_000 + (s as u128) * 100_000_000, tk));
            }
        }
        let mut q: u128 = 54_000_000_000;
        for s in 40..400i64 {
            slot += 1;
            q = (q as f64 * 1.012) as u128;
            evs.push(mk(slot, s, 10_000 + s as u32, true, 300_000_000, q, tk));
        }
        evs
    }

    /// V5-8: two creator sells of 49% and 24.9% of the original holdings (each < 50% of the balance at the time)
    /// trigger DevSell once the cumulative family fraction reaches 0.739 >= 0.5; the old single-sell rule did not.
    #[test]
    fn devsell_kill_is_cumulative() {
        let mut evs = runner_events();
        let creator = 999_999u32;
        let dev = |slot: u64, t: i64, buy: bool, tokens: u64| {
            let mut e = mk(slot, t, creator, buy, 10_000_000, 0, 1_000_000_000_000_000);
            e.tokens = tokens;
            e.order = (slot << 24) | (0xFFFF << 8) | 1;
            e
        };
        // Creator events reuse the pool state of the surrounding events (price unaffected).
        let at = |evs: &Vec<Ev>, t: i64| evs.iter().rposition(|x| x.time <= t).unwrap();
        for (t, buy, tokens) in [(0i64, true, 1_000_000u64), (60, false, 490_000), (70, false, 249_000)] {
            let i = at(&evs, t);
            let mut e = dev(evs[i].slot, evs[i].time, buy, tokens);
            e.quote_res = evs[i].quote_res;
            e.token_res = evs[i].token_res;
            evs.insert(i + 1, e);
        }
        let mut ev = Events::new(evs, vec!["M".into()], (0, 120), vec![]);
        ev.cover_end = vec![100_000];
        let rp = RadarParams { min_buyers: 5, min_buy_sol: 1.0, max_median_buy_sol: 1.0, ..RadarParams::default() };
        let w = WindowData::new("t".into(), &ev, Context::default(), 200.0);
        let pr = Pred { bin: [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], tp1: 0.9, rug: 0.0, hold: 0.0, dead: 0.0 };
        let c = CostsV5 { buy_fail_prob: 0.0, ..Default::default() };
        let st = StratV5 { max_ret_60s_entry: 10.0, max_top10_share: 1.0, max_bundle_share: 1.0, ..Default::default() };
        let r = simulate(&w, rp, &mut Fixed(pr), &st, &c, &[0.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0], false);
        assert_eq!(r.trades, 1, "{:?}", r.blocked);
        let t = &r.trades_list[0];
        assert_eq!(t.final_reason, "DevSell", "{t:?}");
        assert!(t.exit_t >= 70 && t.exit_t < 90, "{t:?}");
    }

    /// EV gate: same runner, but the model says bin 1 with negative R -> no entry.
    #[test]
    fn ev_gate_blocks() {
        let mut evs = Vec::new();
        for s in 0..60i64 {
            for u in 0..3u32 {
                evs.push(mk(100 + (s as u64) * 3 + u as u64, s, (s as u32) * 3 + u, true, 300_000_000, 50_000_000_000, 1_000_000_000_000_000));
            }
        }
        let ev = Events::new(evs, vec!["M".into()], (0, 60), vec![]);
        let rp = RadarParams { min_buyers: 5, min_buy_sol: 1.0, max_median_buy_sol: 1.0, ..RadarParams::default() };
        let w = WindowData::new("t".into(), &ev, Context::default(), 200.0);
        let pr = Pred { bin: [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0], ..Default::default() };
        let r = simulate(&w, rp, &mut Fixed(pr), &StratV5::default(), &CostsV5::default(), &[0.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0], false);
        assert_eq!(r.trades, 0);
        assert!(r.blocked.get("ev_below_min").copied().unwrap_or(0) >= 1, "{:?}", r.blocked);
    }
}
