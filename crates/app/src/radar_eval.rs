//! Radar evaluation on a backfill recording (Stage 1H-style, rule-based, no trading).
//!
//! Labels (offline, use the whole window): per coin, smoothed price = trailing median of 3 trades of at
//! least MIN_PRICE_SOL. Peak = max smoothed price; base = min smoothed price before the peak;
//! run = peak / base; t_cross = first time after the base at which price >= 1.3 x base (G1H.2 rule).
//! Radar (causal): HOT intervals from `radar::Radar`. A runner is CAUGHT when the coin is HOT at t_cross.
//! Upside after flag = peak / price at the last promotion at or before t_cross.

use anyhow::{anyhow, Result};
use decoder::balance_delta::{decode, Side};
use radar::{Params, Radar, Signal, Trade};
use rustc_hash::FxHashMap;
use serde::Serialize;
use std::path::Path;

const MIN_PRICE_SOL: f64 = 0.01;
const MIN_PRICED: usize = 10;
pub const TIERS: [f64; 7] = [2.0, 3.0, 5.0, 10.0, 20.0, 50.0, 100.0];

pub struct Dataset {
    pub mints: Vec<String>,
    pub trades: Vec<Trade>,
    pub coins: usize,
    pub window: (i64, i64),
}

/// Decode a recording into time-ordered trades.
pub fn load(rec: &Path) -> Result<Dataset> {
    let mut mints: FxHashMap<String, u32> = FxHashMap::default();
    let mut traders: FxHashMap<String, u32> = FxHashMap::default();
    let mut trades = Vec::new();
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
        for (i, t) in r["transactions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]).iter().enumerate() {
            let (_, Some(tr)) = decode(t) else { continue };
            let n = mints.len() as u32;
            let coin = *mints.entry(tr.mint.clone()).or_insert(n);
            let n = traders.len() as u32;
            let trader = *traders.entry(tr.trader.clone()).or_insert(n);
            trades.push(Trade {
                time,
                order: (slot << 20) | i as u64,
                coin,
                trader,
                buy: tr.side == Side::Buy,
                sol: tr.sol_lamports as f64 / 1e9,
                price: tr.price_sol(),
            });
        }
    }
    trades.sort_by_key(|t| t.order);
    let mut names = vec![String::new(); mints.len()];
    for (m, i) in mints {
        names[i as usize] = m;
    }
    Ok(Dataset { coins: names.len(), mints: names, trades, window: (t0, t1) })
}

#[derive(Clone, Copy)]
struct Label {
    run: f64,
    base_order: u64,
    cross_order: u64,
    peak: f64,
}

/// Smoothed price at each priced trade, per coin (order, price).
fn smoothed(ds: &Dataset) -> Vec<Vec<(u64, f64)>> {
    let mut raw: Vec<Vec<(u64, f64)>> = vec![Vec::new(); ds.coins];
    for t in &ds.trades {
        if t.sol >= MIN_PRICE_SOL && t.price.is_finite() && t.price > 0.0 {
            raw[t.coin as usize].push((t.order, t.price));
        }
    }
    raw.into_iter()
        .map(|v| {
            if v.len() < MIN_PRICED {
                return Vec::new();
            }
            (2..v.len())
                .map(|i| {
                    let mut w = [v[i - 2].1, v[i - 1].1, v[i].1];
                    w.sort_by(|a, b| a.total_cmp(b));
                    (v[i].0, w[1])
                })
                .collect()
        })
        .collect()
}

fn labels(sm: &[Vec<(u64, f64)>]) -> Vec<Option<Label>> {
    sm.iter()
        .map(|s| {
            if s.is_empty() {
                return None;
            }
            let (ip, &(_, peak)) = s.iter().enumerate().max_by(|a, b| a.1 .1.total_cmp(&b.1 .1))?;
            let (ib, &(_, base)) = s[..=ip].iter().enumerate().min_by(|a, b| a.1 .1.total_cmp(&b.1 .1))?;
            let run = peak / base;
            let cross_order = s[ib..=ip].iter().find(|(_, p)| *p >= 1.3 * base).map(|x| x.0).unwrap_or(s[ip].0);
            Some(Label { run, base_order: s[ib].0, cross_order, peak })
        })
        .collect()
}

#[derive(Serialize, Clone)]
pub struct TierResult {
    pub tier: f64,
    pub runners: usize,
    pub caught: usize,
    pub recall: f64,
    /// Runners whose +30% crossing came from the first real buy after the base (no causal warning possible).
    pub instant_jumps: usize,
    pub recall_excl_instant: f64,
    pub naive_caught: usize,
    pub median_upside_after_flag: f64,
}

#[derive(Serialize, Clone)]
pub struct EvalResult {
    pub params: Params,
    pub window_s: i64,
    pub trades: usize,
    pub coins: usize,
    pub coins_with_price_path: usize,
    pub promoted_coins: usize,
    pub promotions: usize,
    pub hot_max_concurrent: usize,
    /// Of promotions: share whose max smoothed price afterwards is >= 2x / >= 5x the price at promotion.
    pub precision_2x: f64,
    pub precision_5x: f64,
    pub naive_n: usize,
    pub naive_promoted_coins: usize,
    pub naive_precision_2x: f64,
    pub base_rate_run_2x: f64,
    pub tiers: Vec<TierResult>,
    /// Coins first seen after the warm-up (Radar 1 newborn proxy).
    pub tiers_newborn: Vec<TierResult>,
    /// Coins already trading at the window start (Radar 3 awakening candidates).
    pub tiers_existing: Vec<TierResult>,
    pub newborn_coins: usize,
    /// Pre-existing coins with run >= 2x that were not HOT at the +30% crossing.
    pub missed_existing: Vec<serde_json::Value>,
}

fn price_at(s: &[(u64, f64)], order: u64) -> Option<f64> {
    let i = s.partition_point(|(o, _)| *o <= order);
    if i == 0 {
        None
    } else {
        Some(s[i - 1].1)
    }
}

fn max_after(s: &[(u64, f64)], order: u64) -> Option<f64> {
    let i = s.partition_point(|(o, _)| *o <= order);
    s[i..].iter().map(|x| x.1).reduce(f64::max)
}

/// Promotion events (coin, order) and HOT intervals for a radar run.
/// Coins whose first trade is later than this are newborn (Radar 1) on a backfill window.
pub const NEWBORN_WARMUP_S: i64 = 60;
/// Existing-coin runs whose base is in the first WARMUP_EXCLUDE_S of the window are not scored.
pub const WARMUP_EXCLUDE_S: i64 = 120;

fn run_radar(ds: &Dataset, p: Params) -> (Vec<(u32, u64)>, Vec<Vec<(u64, u64)>>, usize, Vec<bool>) {
    let mut r = Radar::new(p).with_newborn_after(ds.window.0 + NEWBORN_WARMUP_S);
    let mut promos = Vec::new();
    let mut open: FxHashMap<u32, u64> = FxHashMap::default();
    let mut intervals: Vec<Vec<(u64, u64)>> = vec![Vec::new(); ds.coins];
    let mut last_sweep = i64::MIN;
    let mut sig = Vec::new();
    for t in &ds.trades {
        if t.time > last_sweep {
            last_sweep = t.time;
            r.sweep(t.time, &mut sig);
            for (c, s) in sig.drain(..) {
                if s == Signal::Demote {
                    if let Some(st) = open.remove(&c) {
                        intervals[c as usize].push((st, t.order));
                    }
                }
            }
        }
        if r.on_trade(t) == Some(Signal::Promote) {
            promos.push((t.coin, t.order));
            open.insert(t.coin, t.order);
        }
    }
    for (c, st) in open {
        intervals[c as usize].push((st, u64::MAX));
    }
    let newborn = (0..ds.coins as u32).map(|c| r.is_newborn(c).unwrap_or(false)).collect();
    (promos, intervals, r.hot_max, newborn)
}

/// Labels, smoothed prices and the naive baseline, computed once per dataset.
pub struct Prepared {
    sm: Vec<Vec<(u64, f64)>>,
    lb: Vec<Option<Label>>,
    /// Per coin: order at which the raw 60 s trade count first reached n (index n-1).
    naive_firsts: Vec<Vec<u64>>,
}

pub fn prepare(ds: &Dataset) -> Prepared {
    let sm = smoothed(ds);
    let lb = labels(&sm);
    let mut win: Vec<std::collections::VecDeque<i64>> = vec![Default::default(); ds.coins];
    let mut naive_firsts: Vec<Vec<u64>> = vec![Vec::new(); ds.coins];
    for t in &ds.trades {
        let w = &mut win[t.coin as usize];
        w.push_back(t.time);
        while w.front().is_some_and(|x| *x <= t.time - 60) {
            w.pop_front();
        }
        let f = &mut naive_firsts[t.coin as usize];
        while f.len() < w.len() {
            f.push(t.order);
        }
    }
    Prepared { sm, lb, naive_firsts }
}

fn naive(prep: &Prepared, n: usize) -> Vec<(u32, u64)> {
    prep.naive_firsts.iter().enumerate().filter(|(_, f)| f.len() >= n).map(|(c, f)| (c as u32, f[n - 1])).collect()
}

pub fn evaluate(ds: &Dataset, prep: &Prepared, p: Params) -> EvalResult {
    let (sm, lb) = (&prep.sm, &prep.lb);
    let (promos, intervals, hot_max, newborn) = run_radar(ds, p);
    let precision = |ps: &[(u32, u64)], x: f64| -> f64 {
        let v: Vec<bool> = ps
            .iter()
            .filter_map(|(c, o)| {
                let s = &sm[*c as usize];
                let p0 = price_at(s, *o)?;
                let mx = max_after(s, *o)?;
                Some(mx / p0 >= x)
            })
            .collect();
        if v.is_empty() {
            f64::NAN
        } else {
            v.iter().filter(|b| **b).count() as f64 / v.len() as f64
        }
    };
    let promoted: rustc_hash::FxHashSet<u32> = promos.iter().map(|x| x.0).collect();
    // Naive baseline matched to the same number of promoted coins (closest n).
    let mut best_n = 1usize;
    let mut best_gap = usize::MAX;
    let mut best_np = Vec::new();
    for n in 1..=400usize {
        let np = naive(prep, n);
        let np_len = np.len();
        let gap = np_len.abs_diff(promoted.len());
        if gap < best_gap {
            best_gap = gap;
            best_n = n;
            best_np = np;
        }
        if np_len < promoted.len() {
            break;
        }
    }
    let naive_first: FxHashMap<u32, u64> = best_np.iter().map(|&(c, o)| (c, o)).collect();
    let priced = lb.iter().filter(|l| l.is_some()).count();
    let order_time: FxHashMap<u64, i64> = ds.trades.iter().map(|t| (t.order, t.time)).collect();
    let base_time = |o: u64| order_time.get(&o).map(|t| t - ds.window.0).unwrap_or(0);
    // Instant jump: at most one real buy between base and crossing (inclusive).
    let instant: Vec<bool> = {
        let mut cnt = vec![0usize; ds.coins];
        for t in &ds.trades {
            if let Some(l) = &lb[t.coin as usize] {
                if t.buy && t.sol >= p.real_min_sol && t.order > l.base_order && t.order <= l.cross_order {
                    cnt[t.coin as usize] += 1;
                }
            }
        }
        cnt.iter().map(|n| *n <= 1).collect()
    };
    let tiers_for = |subset: Option<bool>| -> Vec<TierResult> { TIERS
        .iter()
        .map(|&x| {
            let mut runners = 0;
            let mut instant_jumps = 0;
            let mut caught = 0;
            let mut ncaught = 0;
            let mut ups = Vec::new();
            for (c, l) in lb.iter().enumerate() {
                let Some(l) = l else { continue };
                if l.run < x || subset.is_some_and(|nb| newborn[c] != nb) {
                    continue;
                }
                // Existing coins: runs whose base lies in the warm-up are excluded (radar has no history yet).
                if !newborn[c] && base_time(l.base_order) < WARMUP_EXCLUDE_S {
                    continue;
                }
                runners += 1;
                if instant[c] {
                    instant_jumps += 1;
                }
                if intervals[c].iter().any(|&(a, b)| a < l.cross_order && b >= l.cross_order) {
                    caught += 1;
                    let last_promo = intervals[c].iter().filter(|&&(a, _)| a < l.cross_order).map(|x| x.0).max().expect("caught");
                    // Price at promotion, or the first observable price after it (newborns promote on trade #1).
                    if let Some(p0) = price_at(&sm[c], last_promo).or_else(|| sm[c].first().map(|x| x.1)) {
                        ups.push(l.peak / p0);
                    }
                }
                if naive_first.get(&(c as u32)).is_some_and(|o| *o < l.cross_order) {
                    ncaught += 1;
                }
            }
            ups.sort_by(|a, b| a.total_cmp(b));
            TierResult {
                tier: x,
                runners,
                caught,
                recall: if runners > 0 { caught as f64 / runners as f64 } else { f64::NAN },
                instant_jumps,
                recall_excl_instant: if runners > instant_jumps { caught as f64 / (runners - instant_jumps) as f64 } else { f64::NAN },
                naive_caught: ncaught,
                median_upside_after_flag: if ups.is_empty() { f64::NAN } else { ups[ups.len() / 2] },
            }
        })
        .collect() };
    let tiers = tiers_for(None);
    let tiers_newborn = tiers_for(Some(true));
    let tiers_existing = tiers_for(Some(false));
    let runs2 = lb.iter().flatten().filter(|l| l.run >= 2.0).count();
    EvalResult {
        params: p,
        window_s: ds.window.1 - ds.window.0,
        trades: ds.trades.len(),
        coins: ds.coins,
        coins_with_price_path: priced,
        promoted_coins: promoted.len(),
        promotions: promos.len(),
        hot_max_concurrent: hot_max,
        precision_2x: precision(&promos, 2.0),
        precision_5x: precision(&promos, 5.0),
        naive_n: best_n,
        naive_promoted_coins: best_np.len(),
        naive_precision_2x: precision(&best_np, 2.0),
        base_rate_run_2x: runs2 as f64 / priced.max(1) as f64,
        tiers,
        tiers_newborn,
        tiers_existing,
        newborn_coins: newborn.iter().filter(|b| **b).count(),
        missed_existing: {
            let time_of = |o: u64| ds.trades.iter().find(|t| t.order >= o).map(|t| t.time - ds.window.0);
            lb.iter()
                .enumerate()
                .filter_map(|(c, l)| {
                    let l = l.as_ref()?;
                    if newborn[c] || l.run < 2.0 || intervals[c].iter().any(|&(a, b)| a <= l.cross_order && b >= l.cross_order) {
                        return None;
                    }
                    let first_promo = intervals[c].iter().map(|x| x.0).min();
                    let real_buyers_before: std::collections::BTreeSet<u32> = ds.trades.iter()
                        .filter(|t| t.coin as usize == c && t.buy && t.sol >= p.real_min_sol && t.order >= l.base_order && t.order <= l.cross_order)
                        .map(|t| t.trader).collect();
                    Some(serde_json::json!({
                        "mint": ds.mints[c], "run": l.run,
                        "base_t": time_of(l.base_order), "cross_t": time_of(l.cross_order),
                        "first_promo_t": first_promo.and_then(time_of),
                        "real_buyers_base_to_cross": real_buyers_before.len(),
                    }))
                })
                .collect()
        },
    }
}

/// Grid search on DEVELOPMENT recordings only (summed counts over all of them). Newborns are HOT by
/// rule (Radar 1); the grid tunes the awakening rules (Radar 3). Objective, fixed before looking at the
/// final holdouts: maximize mean recall over the 2x and 5x tiers on pre-existing coins (summed), subject
/// to precision_2x >= naive precision_2x on every recording; ties -> fewer promoted coins.
pub fn tune(sets: &[(&Dataset, &Prepared)]) -> (Params, Vec<Vec<EvalResult>>) {
    let mut grid = Vec::new();
    for &min_buyers in &[3usize, 4] {
        for &min_buy_sol in &[0.5, 1.0] {
            for &min_accel in &[1.5, 2.0] {
                for &slow_min_buyers in &[4usize, 6] {
                    for &slow_min_buy_sol in &[1.0, 2.0] {
                        for &(lift, lift_min_buyers) in
                            &[(0.0, 2usize), (1.1, 2), (1.1, 3), (1.15, 2), (1.15, 3), (1.2, 2), (1.2, 3)]
                        {
                            grid.push(Params {
                                win_s: 60, min_buyers, min_buy_sol, min_accel, max_median_buy_sol: 1.0,
                                slow_win_s: 300, slow_min_buyers, slow_min_buy_sol, lift, lift_min_buyers,
                                ..Default::default()
                            });
                        }
                    }
                }
            }
        }
    }
    let all: Vec<Vec<EvalResult>> = grid.iter().map(|p| sets.iter().map(|(d, pr)| evaluate(d, pr, *p)).collect()).collect();
    let recall = |rs: &Vec<EvalResult>, i: usize| {
        let (c, n) = rs.iter().fold((0, 0), |a, e| (a.0 + e.tiers_existing[i].caught, a.1 + e.tiers_existing[i].runners));
        if n == 0 { 0.0 } else { c as f64 / n as f64 }
    };
    let score = |rs: &Vec<EvalResult>| (recall(rs, 0) + recall(rs, 2)) / 2.0;
    let promoted = |rs: &Vec<EvalResult>| rs.iter().map(|e| e.promoted_coins).sum::<usize>();
    let best = grid
        .iter()
        .zip(&all)
        .filter(|(_, rs)| rs.iter().all(|e| e.precision_2x >= e.naive_precision_2x))
        .max_by(|a, b| score(a.1).total_cmp(&score(b.1)).then(promoted(b.1).cmp(&promoted(a.1))))
        .map(|(p, _)| *p)
        .unwrap_or_default();
    (best, all)
}
