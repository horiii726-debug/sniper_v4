//! Stage 1 radar (V6, file 01 §5, prediction spec Bagian 3–7, 15). Causal: each trade updates only that coin's
//! state and every decision uses nothing after the trade. No ML, no trading.
//!
//! Births come from the caller (`set_birth`, on-chain creation events or an explicit proxy); a newborn is never
//! auto-HOT. Two promotion paths:
//! - NEW coin (birth New, age <= `new_max_age_s`): strict fast rule WITH the median-buy and buy/sell filters.
//! - OLD coin (everything else): V4 awakening rules (slow climb, price lift, fast rule without the median block)
//!   plus a flow rule: net flow / REAL liquidity, unique real buying clusters, cluster growth or a recent CUSUM alarm.
//! Every trade is kept (dust tagged, never dropped); buyers are counted per wallet CLUSTER and creator trades are
//! not counted as real buyers. States: Cold -> Watch (newborns, or old coins with leading signals while the price is
//! flat: cluster growth, split / dust buying from one cluster, CUSUM alarm) -> Hot (Promote). While Watch / Hot the
//! radar emits rate-limited `Decide` rows so the model decides more than once (V5-2).
//! The legacy params file (`config/radar_params_v4_legacy.json`) reproduces the V4 radar exactly.

pub mod cusum;
#[cfg(test)]
mod v4_ref;

pub use cusum::{BaselineKind, Channel, CusumParams};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy)]
pub struct Trade {
    /// Seconds (block time) and an intra-second order key (slot, tx index).
    pub time: i64,
    pub order: u64,
    pub coin: u32,
    pub trader: u32,
    /// Wallet cluster of the trader (funding / co-buying, offline lookup); == trader when unknown.
    pub cluster: u32,
    pub buy: bool,
    pub sol: f64,
    /// Execution price of this trade (sol / tokens): lift rule.
    pub price: f64,
    /// Marginal pool price after the trade; NaN if unknown.
    pub mprice: f64,
    /// Real quote liquidity after the trade, SOL (curve: real SOL reserve; AMM: pool quote reserve); NaN if unknown.
    pub liq_sol: f64,
    /// Trade on an AMM pool rather than the bonding curve.
    pub amm: bool,
    /// Trader is the coin's creator (or in the creator's cluster when a cluster lookup is loaded).
    pub by_creator: bool,
}

impl Trade {
    /// Minimal trade: cluster = trader, mprice = price, liquidity unknown, curve, not the creator.
    pub fn basic(time: i64, order: u64, coin: u32, trader: u32, buy: bool, sol: f64, price: f64) -> Trade {
        Trade { time, order, coin, trader, cluster: trader, buy, sol, price, mprice: price, liq_sol: f64::NAN, amm: false, by_creator: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum Birth {
    /// Created at `t`. proxy = false: on-chain CreateEvent / pool launch (not a migration); proxy = true: inferred.
    New { t: i64, proxy: bool },
    /// Not created inside the observed data (pre-existing) or unknown -> old-coin path.
    Old,
}

impl Birth {
    pub fn is_new(&self) -> bool {
        matches!(self, Birth::New { .. })
    }
    /// 0 old / unknown, 1 proxy newborn, 2 on-chain creation.
    pub fn code(&self) -> u8 {
        match self {
            Birth::Old => 0,
            Birth::New { proxy: true, .. } => 1,
            Birth::New { proxy: false, .. } => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CoinKind {
    New,
    Old,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum RadarState {
    #[default]
    Cold,
    Watch,
    Hot,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    /// -> Hot
    Promote,
    /// Hot -> Cold (sweep)
    Demote,
    /// Cold -> Watch
    Watch,
    /// Watch -> Cold (sweep)
    Unwatch,
    /// Repeated decision row while Watch / Hot (V5-2)
    Decide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewbornMode {
    /// On-chain creations when the window was scanned for them, else the proxy rule.
    #[default]
    Auto,
    /// Force on-chain creations (diagnostics).
    Authoritative,
    /// Force the proxy rule: first trade after a warm-up, on the curve, with low curve progress.
    Proxy,
    /// V4-1 reproduction: first seen after window start + warm-up.
    LegacyFirstSeen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BirthSource {
    Authoritative,
    Proxy,
    LegacyFirstSeen,
}

impl BirthSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            BirthSource::Authoritative => "authoritative",
            BirthSource::Proxy => "proxy",
            BirthSource::LegacyFirstSeen => "legacy_first_seen",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Params {
    // ---- V4 fields (names and meaning unchanged) ----
    pub win_s: i64,
    pub real_min_sol: f64,
    pub min_buyers: usize,
    pub min_buy_sol: f64,
    /// New-coin path only (old path: `old_max_buy_sell_ratio`).
    pub max_buy_sell_ratio: f64,
    /// New-coin path only (old path: `old_max_median_buy_sol`).
    pub max_median_buy_sol: f64,
    pub tau_fast_s: f64,
    pub tau_slow_s: f64,
    pub min_accel: f64,
    pub demote_idle_s: i64,
    /// Slow-climb rule: over `slow_win_s`, unique real buyers >= slow_min_buyers, real buy SOL >= slow_min_buy_sol
    /// and positive net flow (disabled while slow_min_buyers == usize::MAX).
    pub slow_win_s: i64,
    pub slow_min_buyers: usize,
    pub slow_min_buy_sol: f64,
    /// Price-lift rule: last real-trade price >= lift x the slow window's minimum, >= lift_min_buyers buyers and
    /// positive net flow since that minimum. 0 disables.
    pub lift: f64,
    pub lift_min_buyers: usize,
    // ---- V6 fields ----
    pub newborn_mode: NewbornMode,
    pub proxy_warmup_s: i64,
    pub proxy_max_progress: f64,
    /// V4 Radar 1 (legacy only): a newborn is HOT on its first real buy.
    pub newborn_auto_hot: bool,
    /// New-coin path while age <= this (None = forever).
    pub new_max_age_s: Option<i64>,
    /// Creator trades do not count as real buyers / buy SOL / buy arrivals.
    pub exclude_creator: bool,
    /// Count buyers per cluster (true) or per wallet (false).
    pub count_clusters: bool,
    /// Old-coin path filters (None = off).
    pub old_max_median_buy_sol: Option<f64>,
    pub old_max_buy_sell_ratio: Option<f64>,
    pub old_flow_rule: bool,
    pub old_min_clusters: usize,
    /// (buy SOL - sell SOL) / real liquidity over win_s.
    pub old_min_nfl_60: f64,
    pub old_min_liq_sol: f64,
    pub old_min_growth: f64,
    /// Floor of every liquidity denominator, SOL.
    pub liq_floor_sol: f64,
    pub watch: bool,
    pub watch_min_clusters: usize,
    pub watch_min_growth: f64,
    /// Old-coin WATCH only while price <= window low x (1 + this) ("flat"); None = no price condition.
    pub watch_max_rise: Option<f64>,
    pub watch_max_s: i64,
    pub split_min_buys: usize,
    pub split_min_sol: f64,
    pub split_min_liq_frac: f64,
    pub split_max_avg_sol: f64,
    /// Decide-row spacing per coin (0 = no Decide rows).
    pub decide_every_s: i64,
    pub decide_in_watch: bool,
    pub decide_max_age_s: i64,
    pub cusum: CusumParams,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            win_s: 60,
            real_min_sol: 0.01,
            min_buyers: 10,
            min_buy_sol: 3.0,
            max_buy_sell_ratio: 4.0,
            max_median_buy_sol: 0.4,
            tau_fast_s: 10.0,
            tau_slow_s: 120.0,
            min_accel: 1.5,
            demote_idle_s: 300,
            slow_win_s: 300,
            slow_min_buyers: usize::MAX,
            // finite (serde_json writes +inf as null); the slow rule is disabled by slow_min_buyers == usize::MAX
            slow_min_buy_sol: 1e18,
            lift: 0.0,
            lift_min_buyers: 2,
            newborn_mode: NewbornMode::Auto,
            proxy_warmup_s: 60,
            proxy_max_progress: 0.05,
            newborn_auto_hot: false,
            new_max_age_s: Some(1800),
            exclude_creator: true,
            count_clusters: true,
            old_max_median_buy_sol: None,
            old_max_buy_sell_ratio: Some(4.0),
            old_flow_rule: true,
            old_min_clusters: 5,
            old_min_nfl_60: 0.01,
            old_min_liq_sol: 5.0,
            old_min_growth: 1.5,
            liq_floor_sol: 1.0,
            watch: true,
            watch_min_clusters: 3,
            watch_min_growth: 2.0,
            watch_max_rise: Some(0.10),
            watch_max_s: 1800,
            split_min_buys: 5,
            split_min_sol: 0.3,
            split_min_liq_frac: 0.005,
            split_max_avg_sol: 0.05,
            decide_every_s: 5,
            decide_in_watch: true,
            decide_max_age_s: 1800,
            cusum: CusumParams::default(),
        }
    }
}

impl Params {
    /// Trade history kept per coin, seconds.
    pub fn horizon_s(&self) -> i64 {
        self.slow_win_s.max(SPLIT_WIN_S).max(2 * self.win_s)
    }
}

pub const SPLIT_WIN_S: i64 = 300;

#[derive(Default)]
struct Ewma {
    r: f64,
    t: f64,
}

impl Ewma {
    /// R(t) = R(t_prev) e^{-(t - t_prev)/tau} + v/tau  (V4 §11.3)
    fn add(&mut self, t: f64, v: f64, tau: f64) {
        self.r = self.r * (-(t - self.t) / tau).exp() + v / tau;
        self.t = t;
    }
    fn at(&self, t: f64, tau: f64) -> f64 {
        self.r * (-(t - self.t) / tau).exp()
    }
}

#[derive(Debug, Clone, Copy)]
struct Rec {
    time: i64,
    who: u32,
    buy: bool,
    dust: bool,
    creator: bool,
    sol: f64,
    /// execution price (lift rule)
    price: f64,
    /// marginal price if known, else execution price
    px: f64,
}

struct Coin {
    first_time: i64,
    birth: Birth,
    tr: VecDeque<Rec>,
    fast: Ewma,
    slow: Ewma,
    last_real_buy: i64,
    state: RadarState,
    state_since: i64,
    watch_trigger_t: i64,
    promoted_once: bool,
    last_decide: i64,
    decide_until: i64,
    decisions_in_state: u32,
    liq: Option<f64>,
    cusum: cusum::CoinCusum,
    last_alarm: Option<(i64, Channel)>,
    n_alarms: u32,
}

impl Coin {
    fn new(t: i64, birth: Birth, p: &Params) -> Self {
        Coin {
            first_time: t,
            birth,
            tr: VecDeque::new(),
            fast: Ewma { r: 0.0, t: t as f64 },
            slow: Ewma { r: 0.0, t: t as f64 },
            last_real_buy: t,
            state: RadarState::Cold,
            state_since: t,
            watch_trigger_t: t,
            promoted_once: false,
            last_decide: i64::MIN / 2,
            decide_until: i64::MIN,
            decisions_in_state: 0,
            liq: None,
            cusum: cusum::CoinCusum::new(&p.cusum),
            last_alarm: None,
            n_alarms: 0,
        }
    }
}

fn kind(c: &Coin, now: i64, p: &Params) -> CoinKind {
    match c.birth {
        Birth::New { t, .. } if p.new_max_age_s.is_none_or(|m| now - t <= m) => CoinKind::New,
        _ => CoinKind::Old,
    }
}

fn age_ref(c: &Coin) -> i64 {
    match c.birth {
        Birth::New { t, .. } => t,
        Birth::Old => c.first_time,
    }
}

/// One code path for rules and features. Windows are (now - w, now].
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Metrics {
    /// Unique real, non-creator buying clusters (or wallets) in win_s.
    pub clusters_win: u32,
    /// Same in the previous win_s.
    pub clusters_prev_win: u32,
    /// Real, non-creator buy SOL in win_s.
    pub buy_sol_win: f64,
    /// Real sell SOL in win_s (creator included: real selling pressure).
    pub sell_sol_win: f64,
    /// Median real non-creator buy size in win_s (sorted[len/2], V4 definition), 0 if none.
    pub median_buy_win: f64,
    /// clusters_win / (clusters_prev_win + 1).
    pub growth: f64,
    /// Last price / min price over win_s - 1.
    pub rise_from_low_win: Option<f64>,
    /// Dust buys (sol < real_min_sol, creator excluded) in win_s.
    pub dust_n_win: u32,
    pub dust_sol_win: f64,
    /// Largest cluster's dust SOL / dust SOL (None if no dust).
    pub dust_top_cluster_share_win: Option<f64>,
    /// Cluster with the most buy SOL over 300 s (dust included, creator excluded): its SOL and buy count.
    pub split_max_sol_300: f64,
    pub split_max_n_300: u32,
    pub liq_sol: Option<f64>,
    /// (buy_sol_win - sell_sol_win) / max(liquidity, liq_floor_sol); None if liquidity unknown.
    pub nfl_win: Option<f64>,
}

fn metrics(c: &Coin, now: i64, p: &Params) -> Metrics {
    let (w0, w1) = (now - p.win_s, now - 2 * p.win_s);
    let mut cur: Vec<u32> = Vec::new();
    let mut prev: Vec<u32> = Vec::new();
    let mut sizes: Vec<f64> = Vec::new();
    let mut dust_by: FxHashMap<u32, f64> = FxHashMap::default();
    let mut split: FxHashMap<u32, (f64, u32)> = FxHashMap::default();
    let mut m = Metrics::default();
    let (mut lo, mut last_px) = (f64::INFINITY, f64::NAN);
    for r in &c.tr {
        if r.time > now - SPLIT_WIN_S && r.buy && !r.creator {
            let e = split.entry(r.who).or_insert((0.0, 0));
            e.0 += r.sol;
            e.1 += 1;
        }
        if r.time > w1 && r.time <= w0 && r.buy && !r.dust && !r.creator {
            prev.push(r.who);
        }
        if r.time <= w0 {
            continue;
        }
        if r.px > 0.0 && r.px.is_finite() {
            lo = lo.min(r.px);
            last_px = r.px;
        }
        if r.buy {
            if r.dust {
                if !r.creator {
                    m.dust_n_win += 1;
                    m.dust_sol_win += r.sol;
                    *dust_by.entry(r.who).or_default() += r.sol;
                }
            } else if !r.creator {
                cur.push(r.who);
                m.buy_sol_win += r.sol;
                sizes.push(r.sol);
            }
        } else if !r.dust {
            m.sell_sol_win += r.sol;
        }
    }
    cur.sort_unstable();
    cur.dedup();
    prev.sort_unstable();
    prev.dedup();
    m.clusters_win = cur.len() as u32;
    m.clusters_prev_win = prev.len() as u32;
    m.growth = m.clusters_win as f64 / (m.clusters_prev_win as f64 + 1.0);
    if !sizes.is_empty() {
        sizes.sort_by(|a, b| a.total_cmp(b));
        m.median_buy_win = sizes[sizes.len() / 2];
    }
    m.rise_from_low_win = (lo.is_finite() && last_px.is_finite()).then(|| last_px / lo - 1.0);
    if m.dust_sol_win > 0.0 {
        m.dust_top_cluster_share_win = Some(dust_by.values().fold(0.0, |a: f64, b| a.max(*b)) / m.dust_sol_win);
    }
    // deterministic tie-break (lowest cluster id) so the result does not depend on hash order
    if let Some((_, &(s, n))) = split.iter().max_by(|a, b| a.1 .0.total_cmp(&b.1 .0).then(b.0.cmp(a.0))) {
        m.split_max_sol_300 = s;
        m.split_max_n_300 = n;
    }
    m.liq_sol = c.liq;
    m.nfl_win = c.liq.map(|l| (m.buy_sol_win - m.sell_sol_win) / l.max(p.liq_floor_sol));
    m
}

fn accel_ok(c: &Coin, now: i64, p: &Params) -> bool {
    ((now - age_ref(c)) as f64) < p.tau_slow_s || c.fast.at(now as f64, p.tau_fast_s) / (c.slow.at(now as f64, p.tau_slow_s) + 1e-9) >= p.min_accel
}

fn ratio_ok(m: &Metrics, max_bs: Option<f64>) -> bool {
    max_bs.is_none_or(|r| !(m.sell_sol_win > 0.0 && m.buy_sol_win / m.sell_sol_win > r))
}

fn fast_rule(m: &Metrics, p: &Params, max_med: Option<f64>, max_bs: Option<f64>, accel: bool) -> bool {
    m.clusters_win as usize >= p.min_buyers
        && m.buy_sol_win >= p.min_buy_sol
        && m.buy_sol_win > m.sell_sol_win
        && ratio_ok(m, max_bs)
        && max_med.is_none_or(|x| m.median_buy_win <= x)
        && accel
}

/// NEW-coin path (strict, no auto-HOT): fast rule WITH the median and buy/sell filters (file 01 §2.4).
fn new_rule(c: &Coin, m: &Metrics, now: i64, p: &Params) -> bool {
    fast_rule(m, p, Some(p.max_median_buy_sol), Some(p.max_buy_sell_ratio), accel_ok(c, now, p))
}

/// OLD-coin path: awakening rules without the median block, plus the flow / liquidity rule.
fn old_rules(c: &Coin, m: &Metrics, now: i64, p: &Params, alarm_recent: bool) -> bool {
    slow_rule(c, now, p)
        || lift_rule(c, now, p)
        || fast_rule(m, p, p.old_max_median_buy_sol, p.old_max_buy_sell_ratio, accel_ok(c, now, p))
        || (p.old_flow_rule && flow_rule(m, p, alarm_recent))
}

fn flow_rule(m: &Metrics, p: &Params, alarm_recent: bool) -> bool {
    m.clusters_win as usize >= p.old_min_clusters
        && m.buy_sol_win > m.sell_sol_win
        && m.liq_sol.is_none_or(|l| l >= p.old_min_liq_sol)
        && m.nfl_win.is_none_or(|x| x >= p.old_min_nfl_60)
        && (m.growth >= p.old_min_growth || alarm_recent)
        && ratio_ok(m, p.old_max_buy_sell_ratio)
}

fn flat(m: &Metrics, p: &Params) -> bool {
    p.watch_max_rise.is_none_or(|x| m.rise_from_low_win.is_none_or(|r| r <= x))
}

/// Old-coin leading signals (prediction spec Bagian 15): real cluster growth, or split / dust buying by one cluster.
fn watch_trigger(m: &Metrics, p: &Params) -> bool {
    (m.clusters_win as usize >= p.watch_min_clusters && m.growth >= p.watch_min_growth) || split(m, p)
}

fn split(m: &Metrics, p: &Params) -> bool {
    m.split_max_n_300 as usize >= p.split_min_buys
        && m.split_max_sol_300 >= p.split_min_sol.max(m.liq_sol.map_or(0.0, |l| p.split_min_liq_frac * l))
        && m.split_max_sol_300 / m.split_max_n_300.max(1) as f64 <= p.split_max_avg_sol
}

fn slow_rule(c: &Coin, now: i64, p: &Params) -> bool {
    if p.slow_min_buyers == usize::MAX {
        return false;
    }
    let (mut b, mut s) = (0.0, 0.0);
    let mut buyers: Vec<u32> = Vec::new();
    for r in c.tr.iter().filter(|r| !r.dust && r.time > now - p.slow_win_s) {
        if r.buy {
            if !r.creator {
                b += r.sol;
                buyers.push(r.who);
            }
        } else {
            s += r.sol;
        }
    }
    buyers.sort_unstable();
    buyers.dedup();
    buyers.len() >= p.slow_min_buyers && b >= p.slow_min_buy_sol && b > s
}

fn lift_rule(c: &Coin, now: i64, p: &Params) -> bool {
    if p.lift <= 0.0 {
        return false;
    }
    let w: Vec<&Rec> = c.tr.iter().filter(|r| !r.dust && r.time > now - p.slow_win_s).collect();
    if w.len() < 2 {
        return false;
    }
    let last = w[w.len() - 1].price;
    let (imin, min) = w.iter().enumerate().fold((0, f64::INFINITY), |a, (i, r)| if r.price.is_finite() && r.price < a.1 { (i, r.price) } else { a });
    if !(min > 0.0) || last < p.lift * min {
        return false;
    }
    let (mut b, mut s) = (0.0, 0.0);
    let mut buyers: Vec<u32> = Vec::new();
    for r in w.iter().skip(imin + 1) {
        if r.buy {
            if !r.creator {
                b += r.sol;
                buyers.push(r.who);
            }
        } else {
            s += r.sol;
        }
    }
    buyers.sort_unstable();
    buyers.dedup();
    buyers.len() >= p.lift_min_buyers && b > s
}

#[derive(Default)]
struct Counters {
    hot_now: usize,
    hot_max: usize,
    watch_now: usize,
    watch_max: usize,
}

fn transition(c: &mut Coin, to: RadarState, now: i64, p: &Params, k: &mut Counters) {
    match c.state {
        RadarState::Hot => k.hot_now -= 1,
        RadarState::Watch => k.watch_now -= 1,
        RadarState::Cold => {}
    }
    match to {
        RadarState::Hot => {
            k.hot_now += 1;
            k.hot_max = k.hot_max.max(k.hot_now);
        }
        RadarState::Watch => {
            k.watch_now += 1;
            k.watch_max = k.watch_max.max(k.watch_now);
        }
        RadarState::Cold => {}
    }
    c.state = to;
    c.state_since = now;
    if to != RadarState::Cold {
        c.decisions_in_state = 1;
        c.last_decide = now;
        c.decide_until = now + p.decide_max_age_s;
        c.watch_trigger_t = now;
    } else {
        c.decisions_in_state = 0;
    }
}

/// Radar view of one coin (features read this; same values as the rules).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CoinView {
    pub state: RadarState,
    pub kind: CoinKind,
    pub birth: Birth,
    pub state_since: i64,
    pub decisions_in_state: u32,
    pub cusum_s: [Option<f64>; cusum::N_CH],
    pub cusum_pop: [Option<bool>; cusum::N_CH],
    pub last_alarm: Option<(i64, Channel)>,
    pub n_alarms: u32,
    pub metrics: Metrics,
}

pub struct Radar {
    p: Params,
    coins: FxHashMap<u32, Coin>,
    pending_birth: FxHashMap<u32, Birth>,
    newborn_after: Option<i64>,
    pop: cusum::PopBaseline,
    k: Counters,
    pub hot_now: usize,
    pub hot_max: usize,
    pub watch_now: usize,
    pub watch_max: usize,
}

impl Radar {
    pub fn new(p: Params) -> Self {
        Self {
            p,
            coins: FxHashMap::default(),
            pending_birth: FxHashMap::default(),
            newborn_after: None,
            pop: cusum::PopBaseline::new(),
            k: Counters::default(),
            hot_now: 0,
            hot_max: 0,
            watch_now: 0,
            watch_max: 0,
        }
    }

    /// Deprecated shim (V4-1 reproduction): coins without `set_birth` whose first trade is after `t` are proxy newborns.
    #[deprecated(note = "births come from app::radar_feed::BirthTable via set_birth")]
    pub fn with_newborn_after(mut self, t: i64) -> Self {
        self.newborn_after = Some(t);
        self
    }

    /// Birth of a coin; must be called before its first trade (otherwise the coin is Old).
    pub fn set_birth(&mut self, coin: u32, b: Birth) {
        if !self.coins.contains_key(&coin) {
            self.pending_birth.insert(coin, b);
        }
    }

    fn sync(&mut self) {
        self.hot_now = self.k.hot_now;
        self.hot_max = self.k.hot_max;
        self.watch_now = self.k.watch_now;
        self.watch_max = self.k.watch_max;
    }

    /// Demote HOT coins without a real buy for `demote_idle_s`; unwatch WATCH coins idle for `demote_idle_s` or without
    /// a trigger for `watch_max_s` (call before each trade, in time order).
    pub fn sweep(&mut self, now: i64, out: &mut Vec<(u32, Signal)>) {
        let p = self.p;
        for (id, c) in self.coins.iter_mut() {
            match c.state {
                RadarState::Hot if now - c.last_real_buy > p.demote_idle_s => {
                    transition(c, RadarState::Cold, now, &p, &mut self.k);
                    out.push((*id, Signal::Demote));
                }
                RadarState::Watch if now - c.last_real_buy > p.demote_idle_s || now - c.watch_trigger_t > p.watch_max_s => {
                    transition(c, RadarState::Cold, now, &p, &mut self.k);
                    out.push((*id, Signal::Unwatch));
                }
                _ => {}
            }
        }
        self.sync();
    }

    /// Feed one trade. Priority of the returned signal: Promote > Watch > Decide.
    pub fn on_trade(&mut self, t: &Trade) -> Option<Signal> {
        let p = self.p;
        if !self.coins.contains_key(&t.coin) {
            let b = self.pending_birth.remove(&t.coin).unwrap_or(match self.newborn_after {
                Some(a) if t.time > a => Birth::New { t: t.time, proxy: true },
                _ => Birth::Old,
            });
            self.coins.insert(t.coin, Coin::new(t.time, b, &p));
        }
        let c = self.coins.get_mut(&t.coin).expect("inserted");
        let dust = t.sol < p.real_min_sol;
        let creator = p.exclude_creator && t.by_creator;
        let who = if p.count_clusters { t.cluster } else { t.trader };
        // 1) CUSUM: close the steps that ended before this trade (with the liquidity known before it), then add it.
        let mut alarm = None;
        if p.cusum.enabled {
            let birth_t = if let Birth::New { t: b, .. } = c.birth { Some(b) } else { None };
            alarm = c.cusum.advance(t.time, &p.cusum, &mut self.pop, birth_t, c.liq.map(|l| l.max(p.liq_floor_sol)));
            if !dust && !creator {
                c.cusum.add(who, t.buy, t.sol);
            }
            if let Some(ch) = alarm {
                c.last_alarm = Some((t.time, ch));
                c.n_alarms += 1;
            }
        }
        if t.liq_sol.is_finite() && t.liq_sol >= 0.0 {
            c.liq = Some(t.liq_sol);
        }
        // 2) keep every trade, dust tagged
        c.tr.push_back(Rec {
            time: t.time,
            who,
            buy: t.buy,
            dust,
            creator,
            sol: t.sol,
            price: t.price,
            px: if t.mprice.is_finite() && t.mprice > 0.0 { t.mprice } else { t.price },
        });
        let h = p.horizon_s();
        while c.tr.front().is_some_and(|r| r.time <= t.time - h) {
            c.tr.pop_front();
        }
        // 3) real-buy clock + EWMA of real buy arrivals
        if !dust && t.buy && !creator {
            c.fast.add(t.time as f64, 1.0, p.tau_fast_s);
            c.slow.add(t.time as f64, 1.0, p.tau_slow_s);
            c.last_real_buy = t.time;
        }
        let real = !dust;
        let k = kind(c, t.time, &p);
        let alarm_recent = c.last_alarm.is_some_and(|(a, _)| t.time - a <= p.cusum.hold_s);
        if alarm.is_some() && c.state != RadarState::Cold {
            c.decide_until = c.decide_until.max(t.time + p.decide_max_age_s);
            c.watch_trigger_t = t.time;
        }
        // 4) metrics once, only when a rule can use them
        let need_m = c.state != RadarState::Hot && (alarm.is_some() || (t.buy && ((real && !creator) || p.watch)));
        let m = need_m.then(|| metrics(c, t.time, &p));
        let mut out = None;
        // 5) Cold -> Watch / refresh the Watch trigger
        if p.watch && c.state == RadarState::Cold {
            let trig = match k {
                CoinKind::New => true,
                CoinKind::Old => m.as_ref().is_some_and(|m| flat(m, &p) && (alarm.is_some() || watch_trigger(m, &p))),
            };
            if trig {
                transition(c, RadarState::Watch, t.time, &p, &mut self.k);
                out = Some(Signal::Watch);
            }
        } else if c.state == RadarState::Watch && m.as_ref().is_some_and(|m| watch_trigger(m, &p)) {
            c.watch_trigger_t = t.time;
        }
        // 6) Promotion on a real, non-creator buy
        if c.state != RadarState::Hot && real && t.buy && !creator {
            let m = m.as_ref().expect("metrics computed for real non-creator buys");
            let promote = match k {
                CoinKind::New if !p.newborn_auto_hot => new_rule(c, m, t.time, &p),
                CoinKind::New if !c.promoted_once => true,
                _ => old_rules(c, m, t.time, &p, alarm_recent),
            };
            if promote {
                transition(c, RadarState::Hot, t.time, &p, &mut self.k);
                c.promoted_once = true;
                self.sync();
                return Some(Signal::Promote);
            }
        }
        if out.is_some() {
            self.sync();
            return out;
        }
        // 7) Repeated decision rows (V5-2)
        if p.decide_every_s > 0
            && real
            && (c.state == RadarState::Hot || (c.state == RadarState::Watch && p.decide_in_watch))
            && t.time - c.last_decide >= p.decide_every_s
            && t.time <= c.decide_until
        {
            c.last_decide = t.time;
            c.decisions_in_state += 1;
            return Some(Signal::Decide);
        }
        None
    }

    pub fn is_newborn(&self, coin: u32) -> Option<bool> {
        self.coins.get(&coin).map(|c| c.birth.is_new())
    }

    pub fn birth(&self, coin: u32) -> Option<Birth> {
        self.coins.get(&coin).map(|c| c.birth)
    }

    pub fn state(&self, coin: u32) -> RadarState {
        self.coins.get(&coin).map_or(RadarState::Cold, |c| c.state)
    }

    pub fn n_alarms(&self, coin: u32) -> u32 {
        self.coins.get(&coin).map_or(0, |c| c.n_alarms)
    }

    pub fn view(&self, coin: u32, now: i64) -> Option<CoinView> {
        let c = self.coins.get(&coin)?;
        let chs = [Channel::Clusters, Channel::BuySol, Channel::NetFlowLiq];
        Some(CoinView {
            state: c.state,
            kind: kind(c, now, &self.p),
            birth: c.birth,
            state_since: c.state_since,
            decisions_in_state: c.decisions_in_state,
            cusum_s: chs.map(|ch| c.cusum.s(ch)),
            cusum_pop: chs.map(|ch| c.cusum.pop_used(ch)),
            last_alarm: c.last_alarm,
            n_alarms: c.n_alarms,
            metrics: metrics(c, now, &self.p),
        })
    }

    pub fn params(&self) -> &Params {
        &self.p
    }
}

/// Naive baseline for G1H.3: HOT when raw trade count in the last 60 s >= n (no quality filters).
pub struct NaiveRadar {
    n: usize,
    recent: FxHashMap<u32, (VecDeque<i64>, bool)>,
}

impl NaiveRadar {
    pub fn new(n: usize) -> Self {
        Self { n, recent: FxHashMap::default() }
    }
    pub fn on_trade(&mut self, t: &Trade) -> bool {
        let e = self.recent.entry(t.coin).or_default();
        if e.1 {
            return false;
        }
        e.0.push_back(t.time);
        while e.0.front().is_some_and(|x| *x <= t.time - 60) {
            e.0.pop_front();
        }
        if e.0.len() >= self.n {
            e.1 = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests;
