//! Radar evaluation (Stage 1H-style, rule-based, no trading) on a raw backfill recording or on decoded events.
//!
//! Labels (offline, use the whole window): per coin, smoothed price = trailing median of 3 trades of at
//! least MIN_PRICE_SOL. Peak = max smoothed price; base = min smoothed price before the peak;
//! run = peak / base; t_cross = first time after the base at which price >= 1.3 x base (G1H.2 rule);
//! t_cross10 = first time after the base at which price >= 1.1 x base ("before the rise", user decision 7).
//! Radar (causal): HOT and armed (WATCH or HOT) intervals from `radar::Radar`, births from on-chain creations
//! (authoritative) or the labelled proxy. A runner is CAUGHT when the coin is HOT at t_cross; caught "before +10%"
//! when HOT (or armed) at t_cross10. Every rate carries its n; n < 30 is direction only.

use crate::backtest::Events;
use crate::radar_feed::{BirthTable, TradeBuilder};
use anyhow::{anyhow, Result};
use decoder::balance_delta::{decode, Side};
use decoder::txview::TxView;
use radar::{Birth, BirthSource, NewbornMode, Params, Radar, Signal, Trade};
use rustc_hash::{FxHashMap, FxHashSet};
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
    /// Birth of each coin, fixed before its first trade (causal; no follow-up selection).
    pub births: Vec<Birth>,
    pub birth_source: BirthSource,
}

/// Raw recording: trades from balance deltas, births from the window's own creation events (complete scan).
pub fn load(rec: &Path, p: &Params) -> Result<Dataset> {
    if p.newborn_mode == NewbornMode::Proxy {
        return Err(anyhow!("raw radar eval cannot use the proxy newborn rule (no curve progress in balance deltas)"));
    }
    let mut dec = crate::pump_check::load_decoder()?;
    let mut mints: FxHashMap<String, u32> = FxHashMap::default();
    let mut traders: FxHashMap<String, u32> = FxHashMap::default();
    let mut created: FxHashMap<String, i64> = FxHashMap::default();
    let mut first_time: FxHashMap<u32, i64> = FxHashMap::default();
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
            if let Some(tx) = TxView::parse(t) {
                for cr in dec.decode_creations(&tx).into_iter().filter(|c| !c.migration) {
                    created.entry(cr.mint).or_insert(if cr.time > 0 { cr.time } else { time });
                }
            }
            let (_, Some(tr)) = decode(t) else { continue };
            let n = mints.len() as u32;
            let coin = *mints.entry(tr.mint.clone()).or_insert(n);
            let n = traders.len() as u32;
            let trader = *traders.entry(tr.trader.clone()).or_insert(n);
            first_time.entry(coin).or_insert(time);
            trades.push(Trade::basic(time, (slot << 20) | i as u64, coin, trader, tr.side == Side::Buy, tr.sol_lamports as f64 / 1e9, tr.price_sol()));
        }
    }
    trades.sort_by_key(|t| t.order);
    let mut names = vec![String::new(); mints.len()];
    for (m, i) in mints {
        names[i as usize] = m;
    }
    let legacy = p.newborn_mode == NewbornMode::LegacyFirstSeen;
    let births = names
        .iter()
        .enumerate()
        .map(|(c, m)| {
            if legacy {
                return match first_time.get(&(c as u32)) {
                    Some(&ft) if ft > t0 + p.proxy_warmup_s => Birth::New { t: ft, proxy: true },
                    _ => Birth::Old,
                };
            }
            match created.get(m) {
                Some(&ct) if ct >= t0 => Birth::New { t: ct, proxy: false },
                _ => Birth::Old,
            }
        })
        .collect();
    let birth_source = if legacy { BirthSource::LegacyFirstSeen } else { BirthSource::Authoritative };
    Ok(Dataset { coins: names.len(), mints: names, trades, window: (t0, t1), births, birth_source })
}

/// Decoded events (window part only): the same trade builder and births as the Engine (no cluster lookup).
pub fn from_events(ev: &Events, p: &Params) -> Dataset {
    let mut tb = TradeBuilder::new(ev.mints.len(), None);
    let bt = BirthTable::new(ev, p);
    let mut births: Vec<Option<Birth>> = vec![None; ev.mints.len()];
    let mut trades = Vec::new();
    for e in ev.evs.iter().filter(|e| e.time <= ev.window.1) {
        let ci = e.coin as usize;
        if births[ci].is_none() {
            births[ci] = Some(bt.birth(e.coin, e));
        }
        trades.push(tb.build(e));
    }
    Dataset {
        mints: ev.mints.clone(),
        coins: ev.mints.len(),
        window: ev.window,
        births: births.into_iter().map(|b| b.unwrap_or(Birth::Old)).collect(),
        birth_source: bt.source,
        trades,
    }
}

/// `events`: decoded event cache (`pump_events_v2.bin` / `pump_events.bin`); `raw`: raw recording; `auto`: events
/// when a cache exists, else raw.
pub fn load_auto(rec: &Path, p: &Params, source: &str) -> Result<Dataset> {
    let has_events = rec.join("pump_events_v2.bin").exists() || rec.join("pump_events.bin").exists();
    match source {
        "events" => Ok(from_events(&crate::backtest::load_window(rec)?, p)),
        "raw" => load(rec, p),
        "auto" if has_events => Ok(from_events(&crate::backtest::load_window(rec)?, p)),
        "auto" => load(rec, p),
        s => Err(anyhow!("--source {s}: auto|events|raw")),
    }
}

#[derive(Clone, Copy)]
struct Label {
    run: f64,
    base_order: u64,
    cross_order: u64,
    cross10_order: u64,
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
            let cross = |m: f64| s[ib..=ip].iter().find(|(_, p)| *p >= m * base).map(|x| x.0).unwrap_or(s[ip].0);
            Some(Label { run: peak / base, base_order: s[ib].0, cross_order: cross(1.3), cross10_order: cross(1.1), peak })
        })
        .collect()
}

#[derive(Serialize, Clone)]
pub struct TierResult {
    pub tier: f64,
    pub runners: usize,
    /// fewer than 30 runners: direction only
    pub direction_only: bool,
    /// HOT at the +30% crossing
    pub caught: usize,
    pub recall: f64,
    /// HOT at the +10% crossing (before the rise)
    pub caught_hot_before_10: usize,
    pub recall_hot_before_10: f64,
    /// WATCH or HOT at the +10% / +30% crossing
    pub caught_armed_before_10: usize,
    pub recall_armed_before_10: f64,
    pub caught_armed_before_30: usize,
    /// Runners whose +30% crossing came from the first real buy after the base (no causal warning possible).
    pub instant_jumps: usize,
    pub recall_excl_instant: f64,
    pub naive_caught: usize,
    pub median_upside_after_flag: f64,
}

#[derive(Serialize, Clone, Default)]
pub struct CusumEval {
    /// Runners >= 5x (scored subset) and how many had an alarm between their base and the +10% crossing.
    pub runners_5x: usize,
    pub alarmed_before_10: usize,
    /// Median seconds from base to the first such alarm.
    pub median_delay_s: f64,
    /// Coins with run < 2x: observed coin-hours and alarms (false-alarm rate, ARL proxy).
    pub nonrunner_coin_hours: f64,
    pub nonrunner_alarms: usize,
    pub alarms_per_coin_hour: f64,
}

#[derive(Serialize, Clone)]
pub struct EvalResult {
    pub params: Params,
    pub birth_source: &'static str,
    pub window_s: i64,
    pub trades: usize,
    pub coins: usize,
    pub coins_with_price_path: usize,
    pub promoted_coins: usize,
    pub promotions: usize,
    pub hot_per_min: f64,
    pub watch_per_min: f64,
    pub watch_entries: usize,
    pub hot_max_concurrent: usize,
    pub watch_max_concurrent: usize,
    /// Of promotions: share whose max smoothed price afterwards is >= 2x / >= 5x the price at promotion.
    pub precision_2x: f64,
    pub precision_5x: f64,
    pub naive_n: usize,
    pub naive_promoted_coins: usize,
    pub naive_precision_2x: f64,
    pub base_rate_run_2x: f64,
    pub tiers: Vec<TierResult>,
    /// Coins born in the window (on-chain creation or labelled proxy).
    pub tiers_newborn: Vec<TierResult>,
    /// Pre-existing coins (old-coin path).
    pub tiers_existing: Vec<TierResult>,
    pub newborn_coins: usize,
    pub cusum: CusumEval,
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

/// Existing-coin runs whose base is in the first WARMUP_EXCLUDE_S of the window are not scored.
pub const WARMUP_EXCLUDE_S: i64 = 120;

struct RadarRun {
    promos: Vec<(u32, u64)>,
    hot: Vec<Vec<(u64, u64)>>,
    armed: Vec<Vec<(u64, u64)>>,
    hot_max: usize,
    watch_max: usize,
    n_watch: usize,
    /// (order, time) of each CUSUM alarm per coin.
    alarms: Vec<Vec<(u64, i64)>>,
}

fn run_radar(ds: &Dataset, p: Params) -> RadarRun {
    let mut r = Radar::new(p);
    let n = ds.coins;
    let mut out = RadarRun { promos: Vec::new(), hot: vec![Vec::new(); n], armed: vec![Vec::new(); n], hot_max: 0, watch_max: 0, n_watch: 0, alarms: vec![Vec::new(); n] };
    let (mut open_hot, mut open_arm): (FxHashMap<u32, u64>, FxHashMap<u32, u64>) = Default::default();
    let mut seen = vec![false; n];
    let mut last_sweep = i64::MIN;
    let mut sig = Vec::new();
    for t in &ds.trades {
        if t.time > last_sweep {
            last_sweep = t.time;
            r.sweep(t.time, &mut sig);
            for (c, s) in sig.drain(..) {
                if s == Signal::Demote {
                    if let Some(st) = open_hot.remove(&c) {
                        out.hot[c as usize].push((st, t.order));
                    }
                }
                if matches!(s, Signal::Demote | Signal::Unwatch) {
                    if let Some(st) = open_arm.remove(&c) {
                        out.armed[c as usize].push((st, t.order));
                    }
                }
            }
        }
        let ci = t.coin as usize;
        if !seen[ci] {
            seen[ci] = true;
            r.set_birth(t.coin, ds.births[ci]);
        }
        let before = r.n_alarms(t.coin);
        let s = r.on_trade(t);
        if r.n_alarms(t.coin) > before {
            out.alarms[ci].push((t.order, t.time));
        }
        match s {
            Some(Signal::Promote) => {
                out.promos.push((t.coin, t.order));
                open_hot.insert(t.coin, t.order);
                open_arm.entry(t.coin).or_insert(t.order);
            }
            Some(Signal::Watch) => {
                out.n_watch += 1;
                open_arm.entry(t.coin).or_insert(t.order);
            }
            _ => {}
        }
    }
    for (c, st) in open_hot {
        out.hot[c as usize].push((st, u64::MAX));
    }
    for (c, st) in open_arm {
        out.armed[c as usize].push((st, u64::MAX));
    }
    out.hot_max = r.hot_max;
    out.watch_max = r.watch_max;
    out
}

/// Labels, smoothed prices and the naive baseline, computed once per dataset.
pub struct Prepared {
    sm: Vec<Vec<(u64, f64)>>,
    lb: Vec<Option<Label>>,
    /// Per coin: order at which the raw 60 s trade count first reached n (index n-1).
    naive_firsts: Vec<Vec<u64>>,
    /// Per coin: (first, last) trade time.
    span: Vec<(i64, i64)>,
}

pub fn prepare(ds: &Dataset) -> Prepared {
    let sm = smoothed(ds);
    let lb = labels(&sm);
    let mut win: Vec<std::collections::VecDeque<i64>> = vec![Default::default(); ds.coins];
    let mut naive_firsts: Vec<Vec<u64>> = vec![Vec::new(); ds.coins];
    let mut span = vec![(i64::MAX, i64::MIN); ds.coins];
    for t in &ds.trades {
        let ci = t.coin as usize;
        span[ci] = (span[ci].0.min(t.time), span[ci].1.max(t.time));
        let w = &mut win[ci];
        w.push_back(t.time);
        while w.front().is_some_and(|x| *x <= t.time - 60) {
            w.pop_front();
        }
        let f = &mut naive_firsts[ci];
        while f.len() < w.len() {
            f.push(t.order);
        }
    }
    Prepared { sm, lb, naive_firsts, span }
}

fn naive(prep: &Prepared, n: usize) -> Vec<(u32, u64)> {
    prep.naive_firsts.iter().enumerate().filter(|(_, f)| f.len() >= n).map(|(c, f)| (c as u32, f[n - 1])).collect()
}

fn covers(iv: &[(u64, u64)], o: u64) -> bool {
    iv.iter().any(|&(a, b)| a < o && b >= o)
}

fn rate(k: usize, n: usize) -> f64 {
    if n > 0 {
        k as f64 / n as f64
    } else {
        f64::NAN
    }
}

pub fn evaluate(ds: &Dataset, prep: &Prepared, p: Params) -> EvalResult {
    let (sm, lb) = (&prep.sm, &prep.lb);
    let rr = run_radar(ds, p);
    let newborn: Vec<bool> = ds.births.iter().map(|b| b.is_new()).collect();
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
    let promoted: FxHashSet<u32> = rr.promos.iter().map(|x| x.0).collect();
    // Naive baseline matched to the same number of promoted coins (closest n).
    let (mut best_n, mut best_gap, mut best_np) = (1usize, usize::MAX, Vec::new());
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
    let time_of = |o: u64| order_time.get(&o).copied();
    let base_time = |o: u64| time_of(o).map(|t| t - ds.window.0).unwrap_or(0);
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
    let scored = |c: usize, l: &Label| newborn[c] || base_time(l.base_order) >= WARMUP_EXCLUDE_S;
    let tiers_for = |subset: Option<bool>| -> Vec<TierResult> {
        TIERS
            .iter()
            .map(|&x| {
                let (mut runners, mut instant_jumps, mut caught, mut ncaught) = (0, 0, 0, 0);
                let (mut hot10, mut arm10, mut arm30) = (0, 0, 0);
                let mut ups = Vec::new();
                for (c, l) in lb.iter().enumerate() {
                    let Some(l) = l else { continue };
                    if l.run < x || subset.is_some_and(|nb| newborn[c] != nb) || !scored(c, l) {
                        continue;
                    }
                    runners += 1;
                    instant_jumps += instant[c] as usize;
                    hot10 += covers(&rr.hot[c], l.cross10_order) as usize;
                    arm10 += covers(&rr.armed[c], l.cross10_order) as usize;
                    arm30 += covers(&rr.armed[c], l.cross_order) as usize;
                    if covers(&rr.hot[c], l.cross_order) {
                        caught += 1;
                        let last_promo = rr.hot[c].iter().filter(|&&(a, _)| a < l.cross_order).map(|x| x.0).max().expect("caught");
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
                    direction_only: runners < 30,
                    caught,
                    recall: rate(caught, runners),
                    caught_hot_before_10: hot10,
                    recall_hot_before_10: rate(hot10, runners),
                    caught_armed_before_10: arm10,
                    recall_armed_before_10: rate(arm10, runners),
                    caught_armed_before_30: arm30,
                    instant_jumps,
                    recall_excl_instant: if runners > instant_jumps { caught as f64 / (runners - instant_jumps) as f64 } else { f64::NAN },
                    naive_caught: ncaught,
                    median_upside_after_flag: if ups.is_empty() { f64::NAN } else { ups[ups.len() / 2] },
                }
            })
            .collect()
    };
    let cusum = {
        let mut ce = CusumEval::default();
        let mut delays = Vec::new();
        for (c, l) in lb.iter().enumerate() {
            let Some(l) = l else { continue };
            if l.run >= 5.0 && scored(c, l) {
                ce.runners_5x += 1;
                if let Some(&(_, at)) = rr.alarms[c].iter().find(|(o, _)| *o >= l.base_order && *o <= l.cross10_order) {
                    ce.alarmed_before_10 += 1;
                    delays.push((at - time_of(l.base_order).unwrap_or(at)) as f64);
                }
            }
        }
        for c in 0..ds.coins {
            if lb[c].is_some_and(|l| l.run >= 2.0) || prep.span[c].0 == i64::MAX {
                continue;
            }
            ce.nonrunner_coin_hours += ((prep.span[c].1 - prep.span[c].0).max(60)) as f64 / 3600.0;
            ce.nonrunner_alarms += rr.alarms[c].len();
        }
        delays.sort_by(|a, b| a.total_cmp(b));
        ce.median_delay_s = if delays.is_empty() { f64::NAN } else { delays[delays.len() / 2] };
        ce.alarms_per_coin_hour = if ce.nonrunner_coin_hours > 0.0 { ce.nonrunner_alarms as f64 / ce.nonrunner_coin_hours } else { f64::NAN };
        ce
    };
    let runs2 = lb.iter().flatten().filter(|l| l.run >= 2.0).count();
    let minutes = ((ds.window.1 - ds.window.0).max(1)) as f64 / 60.0;
    EvalResult {
        params: p,
        birth_source: ds.birth_source.as_str(),
        window_s: ds.window.1 - ds.window.0,
        trades: ds.trades.len(),
        coins: ds.coins,
        coins_with_price_path: priced,
        promoted_coins: promoted.len(),
        promotions: rr.promos.len(),
        hot_per_min: rr.promos.len() as f64 / minutes,
        watch_per_min: rr.n_watch as f64 / minutes,
        watch_entries: rr.n_watch,
        hot_max_concurrent: rr.hot_max,
        watch_max_concurrent: rr.watch_max,
        precision_2x: precision(&rr.promos, 2.0),
        precision_5x: precision(&rr.promos, 5.0),
        naive_n: best_n,
        naive_promoted_coins: best_np.len(),
        naive_precision_2x: precision(&best_np, 2.0),
        base_rate_run_2x: runs2 as f64 / priced.max(1) as f64,
        tiers: tiers_for(None),
        tiers_newborn: tiers_for(Some(true)),
        tiers_existing: tiers_for(Some(false)),
        newborn_coins: newborn.iter().filter(|b| **b).count(),
        cusum,
        missed_existing: lb
            .iter()
            .enumerate()
            .filter_map(|(c, l)| {
                let l = l.as_ref()?;
                if newborn[c] || l.run < 2.0 || rr.hot[c].iter().any(|&(a, b)| a <= l.cross_order && b >= l.cross_order) {
                    return None;
                }
                let rel = |o: u64| time_of(o).map(|t| t - ds.window.0);
                Some(serde_json::json!({ "mint": ds.mints[c], "run": l.run, "base_t": rel(l.base_order), "cross_t": rel(l.cross_order),
                    "first_promo_t": rr.hot[c].iter().map(|x| x.0).min().and_then(rel) }))
            })
            .take(200)
            .collect(),
    }
}

/// Legacy V4 grid (`radar-tune --legacy`): newborns HOT by rule; maximize mean recall over the 2x and 5x tiers on
/// pre-existing coins (summed), subject to precision_2x >= naive on every recording; ties -> fewer promoted coins.
pub fn tune(sets: &[(&Dataset, &Prepared)], base: Params) -> (Params, Vec<Vec<EvalResult>>) {
    let mut grid = Vec::new();
    for &min_buyers in &[3usize, 4] {
        for &min_buy_sol in &[0.5, 1.0] {
            for &min_accel in &[1.5, 2.0] {
                for &slow_min_buyers in &[4usize, 6] {
                    for &slow_min_buy_sol in &[1.0, 2.0] {
                        for &(lift, lift_min_buyers) in &[(0.0, 2usize), (1.1, 2), (1.1, 3), (1.15, 2), (1.15, 3), (1.2, 2), (1.2, 3)] {
                            grid.push(Params {
                                win_s: 60, min_buyers, min_buy_sol, min_accel, max_median_buy_sol: 1.0, old_max_median_buy_sol: Some(1.0),
                                slow_win_s: 300, slow_min_buyers, slow_min_buy_sol, lift, lift_min_buyers, ..base
                            });
                        }
                    }
                }
            }
        }
    }
    use rayon::prelude::*;
    let all: Vec<Vec<EvalResult>> = grid.par_iter().map(|p| sets.iter().map(|(d, pr)| evaluate(d, pr, *p)).collect()).collect();
    let recall = |rs: &Vec<EvalResult>, i: usize| {
        let (c, n) = rs.iter().fold((0, 0), |a, e| (a.0 + e.tiers_existing[i].caught, a.1 + e.tiers_existing[i].runners));
        if n == 0 {
            0.0
        } else {
            c as f64 / n as f64
        }
    };
    let score = |rs: &Vec<EvalResult>| (recall(rs, 0) + recall(rs, 2)) / 2.0;
    let promoted = |rs: &Vec<EvalResult>| rs.iter().map(|e| e.promoted_coins).sum::<usize>();
    let best = grid
        .iter()
        .zip(&all)
        .filter(|(_, rs)| rs.iter().all(|e| e.precision_2x >= e.naive_precision_2x))
        .max_by(|a, b| score(a.1).total_cmp(&score(b.1)).then(promoted(b.1).cmp(&promoted(a.1))))
        .map(|(p, _)| *p)
        .unwrap_or(base);
    (best, all)
}

fn tier5(v: &[TierResult]) -> Option<&TierResult> {
    v.iter().find(|t| t.tier == 5.0)
}

/// V6 selection score of one window: mean(recall HOT before +10%, recall HOT at +30%) at tier 5x, averaged over
/// new and old coins (NaN parts skipped).
fn v6_score(e: &EvalResult) -> f64 {
    let part = |v: &[TierResult]| tier5(v).map(|t| [t.recall_hot_before_10, t.recall].iter().filter(|x| x.is_finite()).sum::<f64>() / 2.0);
    let xs: Vec<f64> = [part(&e.tiers_newborn), part(&e.tiers_existing)].into_iter().flatten().filter(|x| x.is_finite()).collect();
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    match v.len() {
        0 => f64::NAN,
        n if n % 2 == 1 => v[n / 2],
        n => 0.5 * (v[n / 2 - 1] + v[n / 2]),
    }
}

/// V6 grid (16 points <= 24, file 01 §9.3) on DEV windows only. Selection rule fixed before results: maximize the
/// median over dev windows of `v6_score`, subject to hot_per_min <= max_hot_per_min and precision_2x >= naive
/// precision_2x on every dev window; ties -> fewer promoted coins. Also returns an ablation of the chosen params.
#[allow(clippy::type_complexity)]
pub fn tune_v6(sets: &[(&Dataset, &Prepared)], base: Params, max_hot_per_min: f64) -> (Option<Params>, Vec<(Params, Vec<EvalResult>)>, Vec<(String, Vec<EvalResult>)>) {
    use rayon::prelude::*;
    let mut grid = Vec::new();
    for &h in &[4.0, 5.0] {
        for &old_min_clusters in &[4usize, 6] {
            for &old_min_nfl_60 in &[0.0, 0.02] {
                for &old_max_buy_sell_ratio in &[Some(4.0), None] {
                    grid.push(Params { old_min_clusters, old_min_nfl_60, old_max_buy_sell_ratio, cusum: radar::CusumParams { h, ..base.cusum }, ..base });
                }
            }
        }
    }
    let all: Vec<(Params, Vec<EvalResult>)> = grid.par_iter().map(|p| (*p, sets.iter().map(|(d, pr)| evaluate(d, pr, *p)).collect())).collect();
    let ok = |rs: &Vec<EvalResult>| rs.iter().all(|e| e.hot_per_min <= max_hot_per_min && e.precision_2x >= e.naive_precision_2x);
    let promoted = |rs: &Vec<EvalResult>| rs.iter().map(|e| e.promoted_coins).sum::<usize>();
    let best = all
        .iter()
        .filter(|(_, rs)| ok(rs))
        .max_by(|a, b| {
            let (sa, sb) = (median(a.1.iter().map(v6_score).collect()), median(b.1.iter().map(v6_score).collect()));
            sa.total_cmp(&sb).then(promoted(&b.1).cmp(&promoted(&a.1)))
        })
        .map(|x| x.0);
    let chosen = best.unwrap_or(base);
    let variants: Vec<(String, Params)> = vec![
        ("no_cusum".into(), Params { cusum: radar::CusumParams { enabled: false, ..chosen.cusum }, ..chosen }),
        ("no_flow_rule".into(), Params { old_flow_rule: false, ..chosen }),
        ("no_watch".into(), Params { watch: false, ..chosen }),
        ("old_median_block_1.0".into(), Params { old_max_median_buy_sol: Some(1.0), ..chosen }),
    ];
    let ablation = variants.par_iter().map(|(n, p)| (n.clone(), sets.iter().map(|(d, pr)| evaluate(d, pr, *p)).collect())).collect();
    (best, all, ablation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backtest::Ev;

    #[test]
    fn from_events_births_and_cross10() {
        let mut evs = Vec::new();
        // coin 0 trades from t=0 (old), coin 1 first trades at t=200 on a fresh curve (proxy newborn)
        for i in 0..40u64 {
            let (coin, t) = if i < 20 { (0u32, i as i64) } else { (1u32, 200 + i as i64) };
            let q = 30_000_000_000u128 + (i as u128) * 1_000_000_000;
            evs.push(Ev { slot: 10 + i, order: (10 + i) << 24, time: t, coin, amm: false, buy: true, sol: 100_000_000, tokens: 1_000_000, user: i as u32,
                          creator: 999, quote_res: q, token_res: 1_000_000_000_000_000, real_quote: (q - 30_000_000_000) as u64,
                          real_token: 790_000_000_000_000, fee_bps: 100, complete: false });
        }
        let ev = Events::new(evs, vec!["a".into(), "b".into()], (0, 1000), vec![]);
        let ds = from_events(&ev, &Params::default());
        assert_eq!(ds.birth_source, BirthSource::Proxy);
        assert_eq!(ds.births, vec![Birth::Old, Birth::New { t: 220, proxy: true }]);
        assert!(ds.trades[5].liq_sol.is_finite() && ds.trades[5].mprice > 0.0);
        let path: Vec<(u64, f64)> = [1.0, 0.9, 0.95, 1.05, 1.12, 1.4, 2.0].iter().enumerate().map(|(i, p)| (i as u64, *p)).collect();
        let l = labels(&[path])[0].unwrap();
        assert_eq!((l.base_order, l.cross10_order, l.cross_order), (1, 3, 5));
    }
}
