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

const REAL_MIN_SOL: f64 = 0.01;

/// Minimum real exit liquidity (SOL) to enter (curve: real quote reserve, V5-13).
const MIN_EXIT_LIQ_SOL: f64 = 0.5;

/// Normal exit policy (always-on exits apply to every mode). User decision: sell 100%, on crowd momentum.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExitMode {
    /// A: Momentum Fade (prediction spec Bagian 16): sellers dominate AND (quiet OR smaller sizes OR crowd shrinking)
    /// for fade_persist_s, or no real buy for idle_s; sells 100%.
    #[default]
    MomentumFade,
    /// B: Momentum Fade AND drawdown from the post-entry peak >= adaptive d_t = clamp(1 - exp(-trail_k sigma_60), d_min, d_max).
    FadeAndDrawdown,
    /// C: fixed trailing stop from the post-entry peak (trail_fixed), sells 100% (U11 ATH / Peak Trailing Stop).
    Trail,
    /// C2: U12 adaptive volatility trailing from the post-entry peak, d_t = clamp(1 - exp(-trail_k sigma_60), d_min,
    /// d_max) with sigma_60 = vol_60s x sqrt(60) (1-s log-return std scaled to 60 s), sells 100%.
    AdaptiveTrail,
    /// D: V5 TP1 / TP2 partial ladder + bag trail (comparison baseline only).
    Ladder,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
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
    /// V5-1: entry candidates are collected per decision tick of this many seconds, ranked by EV, and only then
    /// checked against exposure / slots / cash (0 = batch per slot).
    pub decision_tick_s: i64,
    /// V5-2: evaluate entries at every radar decision row while the coin is HOT (false = Promote rows only, V5).
    pub decision_rows: bool,
    /// No re-entry into a coin within this many seconds of closing a position on it.
    pub reentry_cooldown_s: i64,
    pub exit_mode: ExitMode,
    /// P5 price guard: enter only while price <= 5-minute low x (1 + this) ("buy before the rise"); None = off.
    pub max_entry_ret: Option<f64>,
    /// Momentum Fade: buy-arrival EWMA time constant, s.
    pub fade_tau_s: f64,
    /// Quiet: buy-arrival EWMA < this x its peak since entry.
    pub fade_rate_frac: f64,
    /// Smaller sizes: mean real buy over 30 s < this x its peak since entry.
    pub fade_size_frac: f64,
    /// The fade condition must hold this long.
    pub fade_persist_s: i64,
    /// No fade exit in the first seconds after the fill.
    pub fade_min_hold_s: i64,
    /// Dead: no real buy for this long.
    pub idle_s: i64,
    /// Time stop: no gain of time_stop_gain within time_stop_s after entry (0 = off).
    pub time_stop_s: i64,
    pub time_stop_gain: f64,
    /// Flow-confirmed time stop: the stalled position is sold only while the coin has at most this many distinct
    /// buyers in the last 60 s (stalled coins with active buyers reach 2x 3-4x more often; INFINITY = classic stop).
    pub time_stop_max_buyers60: f64,
    /// B: adaptive drawdown bounds.
    pub d_min: f64,
    pub d_max: f64,
    /// C: fixed trailing distance.
    pub trail_fixed: f64,
    /// Optional wide disaster stop for non-ladder modes (value <= (1 - this) x cost after stop_grace_s).
    pub disaster_stop: Option<f64>,
    /// Block entries into mints with a honeypot-risk flag (freeze authority, transfer hook, ...; mint_safety.rs).
    pub mint_gate: bool,
    /// E9 account risk: no new entries once equity <= start x (1 - this) (None = off).
    pub max_loss_frac: Option<f64>,
    /// E9 account risk: after this many consecutive losing trades, no new entries for `loss_streak_pause_s` (0 = off).
    pub loss_streak: u32,
    pub loss_streak_pause_s: i64,
    /// M1 meta-labeling: also evaluate entries at WATCH decision rows (radar = high recall, the model takes / skips).
    pub entry_on_watch: bool,
    /// Trade the venues that passed gate C18 (fill accuracy) instead of pump.fun / PumpSwap only.
    pub c18_venues: bool,
    /// Compounding: each new position = this fraction of the current equity (None = fixed bet_sol).
    pub bet_equity_frac: Option<f64>,
    /// Fractional Kelly sizing on the current equity (compounding): bet = kelly_frac x f* x equity, f* = growth-optimal
    /// fraction for the candidate's bin probabilities and the training trades' returns per true bin (`KellyTable`).
    /// f* = 0 (no edge) -> no trade. None = off (fixed bet / bet_equity_frac).
    pub kelly_frac: Option<f64>,
    /// Kelly bets below this are skipped: fixed tx costs (~0.001 SOL per round trip) would dominate.
    pub kelly_min_bet_sol: f64,
}

impl Default for StratV5 {
    /// Starting values of V5 plan §7.5.
    fn default() -> Self {
        Self {
            ev_min: 0.15, rug_veto: 0.5, max_ret_60s_entry: 0.5, max_top10_share: 0.5, max_bundle_share: 0.3,
            max_positions: 18, replace_margin: 0.15, max_entries_per_min: 3, bet_sol: 0.05, max_total_exposure: 0.6,
            pre_tp_stop: 0.35, stop_grace_s: 20, tp1_mult: 2.0, tp1_sell_frac: 0.5, tp2_mult: 5.0, tp2_sell_frac: 0.25,
            bag_trail: 0.55, trail_k: 3.0, hold_min: 0.4, dev_kill_min_frac: 0.5, lp_drop_frac: 0.6, dead_exit: 0.7, max_hold_s: 36_000,
            decision_tick_s: 2,
            decision_rows: true,
            reentry_cooldown_s: 300,
            exit_mode: ExitMode::MomentumFade,
            max_entry_ret: Some(0.10),
            fade_tau_s: 10.0,
            fade_rate_frac: 0.25,
            fade_size_frac: 0.5,
            fade_persist_s: 10,
            fade_min_hold_s: 10,
            idle_s: 120,
            time_stop_s: 300,
            time_stop_gain: 0.10,
            time_stop_max_buyers60: 1.0,
            mint_gate: true,
            max_loss_frac: None,
            loss_streak: 0,
            loss_streak_pause_s: 900,
            entry_on_watch: false,
            c18_venues: false,
            bet_equity_frac: None,
            kelly_frac: None,
            kelly_min_bet_sol: 0.02,
            d_min: 0.15,
            d_max: 0.60,
            trail_fixed: 0.30,
            disaster_stop: None,
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
    /// Wallet execution: a buy carries min tokens out = quote at decision x (1 - this); a fill below it fails (fee paid).
    pub buy_slippage: f64,
    /// Wallet execution: a sell carries min SOL out = value at decision x (1 - this); below it the tx fails (fee paid)
    /// and is retried after `latency_slots`; the last try (`sell_max_tries`) has no limit.
    pub sell_slippage: f64,
    /// Wallet execution: probability a sell tx fails for other reasons (congestion, expired blockhash); retried.
    pub sell_fail_prob: f64,
    pub sell_max_tries: u8,
    /// Token account rent locked per open position (refunded when the account is closed with the final sell).
    pub ata_rent_lamports: u64,
}

impl Default for CostsV5 {
    fn default() -> Self {
        // 500,000 lamports per transaction in total, as in V4 (base 5,000 + priority 195,000 + tip 300,000).
        Self { latency_slots: 2, tx_base_lamports: 5_000, priority_lamports: 195_000, tip_lamports: 300_000, buy_fail_prob: 0.15, default_fee_bps: 125, fail_seed: 0, start_sol: crate::units::start_sol(),
            buy_slippage: 0.20, sell_slippage: 0.30, sell_fail_prob: 0.05, sell_max_tries: 3, ata_rent_lamports: 2_074_080 }
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
    /// Distinct buyers in the last 60 s at the coin's latest snapshot since entry (0 before the first one).
    last_buyers60: f64,
    entry_ev: f64,
    entry_label: Option<Labels>,
    /// (time, quote reserve) within 60 s, AMM pools only.
    q_hist: VecDeque<(i64, f64)>,
    /// V5-7: latest P(hold) prediction while held (None until the model scored a held snapshot).
    last_hold: Option<f32>,
    /// V5-7 (oracle runs only): (P(hold) bucket, liquidation value, legs sold so far) at held snapshots.
    marks: Vec<(u8, f64, u32)>,
    fade: Fade,
}

/// Crowd-momentum state of a held position, from real trades after the fill.
#[derive(Debug, Clone, Default)]
struct Fade {
    rate: f64,
    rate_t: i64,
    peak_rate: f64,
    /// (time, buy, SOL) of real trades within 30 s
    w30: VecDeque<(i64, bool, f64)>,
    peak_size: f64,
    since: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct FadeNow {
    quiet: bool,
    smaller: bool,
    sellers: bool,
}

impl Fade {
    fn rate_at(&self, t: i64, tau: f64) -> f64 {
        self.rate * (-((t - self.rate_t) as f64) / tau).exp()
    }

    /// Feed a real trade of the coin; returns the instantaneous signals.
    fn on_trade(&mut self, t: i64, buy: bool, sol: f64, tau: f64, st: &StratV5) -> FadeNow {
        self.rate = self.rate_at(t, tau) + if buy { 1.0 / tau } else { 0.0 };
        self.rate_t = t;
        self.peak_rate = self.peak_rate.max(self.rate);
        self.w30.push_back((t, buy, sol));
        while self.w30.front().is_some_and(|x| x.0 <= t - 30) {
            self.w30.pop_front();
        }
        let (mut b, mut s, mut nb) = (0.0, 0.0, 0usize);
        for &(_, by, v) in &self.w30 {
            if by {
                b += v;
                nb += 1;
            } else {
                s += v;
            }
        }
        let size = if nb > 0 { b / nb as f64 } else { 0.0 };
        self.peak_size = self.peak_size.max(size);
        FadeNow { quiet: self.rate < st.fade_rate_frac * self.peak_rate, smaller: nb == 0 || size < st.fade_size_frac * self.peak_size, sellers: s > b }
    }
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
    /// V5-7 (oracle runs): forward net return observations (bucket, return, weight) for the hold table.
    #[serde(skip)]
    pub hold_obs: Vec<(u8, f64, f64)>,
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
    /// Buys that failed on the slippage limit (subset of failed_buys).
    pub slippage_failed_buys: usize,
    /// Sell transactions that failed (slippage limit or random) and were retried.
    pub sell_retries: usize,
    pub exits_by_reason: std::collections::BTreeMap<&'static str, usize>,
    pub final_reasons: std::collections::BTreeMap<&'static str, usize>,
    pub runners_10x: usize,
    pub runners_10x_captured_5x: usize,
    pub moonshot_capture: f64,
    pub signals_in_window: usize,
    pub blocked: std::collections::BTreeMap<&'static str, usize>,
    /// V5-1: open positions replaced by a better candidate.
    pub replacements: usize,
    /// V5-1: signal rows merged into a later row of the same coin in the same decision tick.
    pub merged_in_tick: usize,
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

/// V5-7: weight of the pooled prior when shrinking hold-table buckets.
pub const HOLD_PRIOR_W: f64 = 5.0;

/// V5-7: expected NET forward return of continuing to hold under this exit policy, per SOL of current liquidation
/// value, by P(hold) decile (index 10 = no prediction yet). Measured on training windows' oracle runs, like R_k.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct HoldTable {
    pub r: [f64; 11],
    pub w: [f64; 11],
}

impl HoldTable {
    pub const NO_PRED: usize = 10;

    pub fn bucket(p: Option<f32>) -> usize {
        match p {
            Some(x) if x.is_finite() => ((x.clamp(0.0, 1.0) * 10.0) as usize).min(9),
            _ => Self::NO_PRED,
        }
    }

    /// obs = (bucket, forward net return, weight); returns clipped to [-1, 1000] and each bucket shrunk toward the
    /// pooled mean with weight HOLD_PRIOR_W. None without observations (replacement then stays disabled).
    pub fn fit(obs: &[(u8, f64, f64)]) -> Option<Self> {
        let (mut sw, mut swr) = ([0f64; 11], [0f64; 11]);
        for &(b, r, w) in obs {
            if !r.is_finite() || !(w > 0.0) {
                continue;
            }
            let b = (b as usize).min(10);
            sw[b] += w;
            swr[b] += w * r.clamp(-1.0, 1000.0);
        }
        let tw: f64 = sw.iter().sum();
        if tw <= 0.0 {
            return None;
        }
        let prior = swr.iter().sum::<f64>() / tw;
        let mut r = [0f64; 11];
        for k in 0..11 {
            r[k] = (swr[k] + HOLD_PRIOR_W * prior) / (sw[k] + HOLD_PRIOR_W);
        }
        Some(Self { r, w: sw })
    }

    pub fn value(&self, p: Option<f32>) -> f64 {
        self.r[Self::bucket(p)]
    }
}

/// V5-7: forward net return from each hold mark (bucket, value at the mark, legs already sold at the mark):
/// sum over the later legs of (proceeds - txc) / value - 1, each mark weighted 1 / marks (each trade weighs 1).
pub fn forward_returns(marks: &[(u8, f64, u32)], legs: &[Leg], txc: f64) -> Vec<(u8, f64, f64)> {
    let w = 1.0 / marks.len().max(1) as f64;
    marks
        .iter()
        .filter(|m| m.1 > 0.0)
        .map(|&(b, v, k)| {
            let got: f64 = legs.iter().skip(k as usize).map(|l| l.proceeds_sol - txc).sum();
            (b, got / v - 1.0, w)
        })
        .collect()
}

/// V5-1: one entry candidate of the current decision tick.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cand {
    pub coin: u32,
    pub ev: f64,
    pub order: u64,
    pub label: Option<Labels>,
    /// Growth-optimal fraction of equity f* (NaN = no Kelly table).
    pub kelly: f64,
}

/// Bet of a candidate in SOL: fractional Kelly on the equity when enabled and fitted, else the tick's fixed bet.
pub(crate) fn cand_bet(st: &StratV5, c: &Cand, equity: f64) -> f64 {
    match st.kelly_frac {
        Some(k) if c.kelly.is_finite() => k * c.kelly * equity.max(0.0),
        _ => st.bet_sol,
    }
}

/// Fractional Kelly input: g[k][j] = mean ln(1 + f_j r) over the training trades whose true bin is k (bins with few
/// trades shrunk toward the pooled mean, prior weight 5 trades, like R_k), on the grid f_j = 0.0025 .. 1.0.
#[derive(Debug, Clone, Serialize)]
pub struct KellyTable {
    pub f: Vec<f64>,
    pub g: Vec<Vec<f64>>,
}

impl KellyTable {
    pub fn fit(rets: &[(u8, f64)]) -> Option<Self> {
        if rets.is_empty() {
            return None;
        }
        let f: Vec<f64> = (1..=400).map(|j| j as f64 * 0.0025).collect();
        let lg = |r: f64, fj: f64| (1.0 + fj * r).max(1e-9).ln();
        let pooled: Vec<f64> = f.iter().map(|&fj| rets.iter().map(|(_, r)| lg(*r, fj)).sum::<f64>() / rets.len() as f64).collect();
        let g = (0..7u8).map(|k| {
            let rk: Vec<f64> = rets.iter().filter(|(b, _)| *b == k).map(|(_, r)| *r).collect();
            f.iter().zip(&pooled).map(|(&fj, &pj)| (rk.iter().map(|r| lg(*r, fj)).sum::<f64>() + 5.0 * pj) / (rk.len() as f64 + 5.0)).collect()
        }).collect();
        Some(Self { f, g })
    }

    /// Growth-optimal fraction for bin probabilities `p` (0 = expected log growth <= 0 at every fraction).
    pub fn f_star(&self, p: &[f32; 7]) -> f64 {
        let mut best = (0.0, 0.0);
        for j in 0..self.f.len() {
            let v: f64 = (0..7).map(|k| p[k] as f64 * self.g[k][j]).sum();
            if v > best.1 {
                best = (self.f[j], v);
            }
        }
        best.0
    }
}

/// V5-1: an open position that may be replaced. `hold_sol` = value x H(P(hold)) (expected net SOL of holding).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Held {
    pub coin: u32,
    pub hold_sol: f64,
    pub cost_share: f64,
    pub value: f64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Capacity {
    pub open_cost: f64,
    pub equity: f64,
    pub cash_free: f64,
    pub slots_used: usize,
    pub entries_60s: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Decision {
    Enter(usize),
    /// (candidate index, coin of the position sold to make room)
    Replace(usize, u32),
    Block(usize, &'static str),
    Merged(usize),
}

/// V5-1: rank the tick's candidates by EV (one row per coin: the latest), then apply the per-minute cap, exposure,
/// slots and cash in rank order. When capacity binds, the weakest held position (lowest expected net SOL of holding)
/// is replaced iff `bet * EV - txc > hold_sol + replace_margin * bet` (V5-7: both sides in expected net SOL).
pub(crate) fn allocate(cands: &[Cand], held: &[Held], mut cap: Capacity, st: &StratV5, txc: f64, can_replace: bool, oracle: bool) -> Vec<Decision> {
    let mut out = Vec::with_capacity(cands.len());
    let mut latest: FxHashMap<u32, usize> = FxHashMap::default();
    for (i, c) in cands.iter().enumerate() {
        let e = latest.entry(c.coin).or_insert(i);
        if cands[*e].order < c.order {
            *e = i;
        }
    }
    let mut keep = Vec::with_capacity(latest.len());
    for (i, c) in cands.iter().enumerate() {
        if latest[&c.coin] == i {
            keep.push(i);
        } else {
            out.push(Decision::Merged(i));
        }
    }
    keep.sort_by(|&a, &b| cands[b].ev.total_cmp(&cands[a].ev).then(cands[a].order.cmp(&cands[b].order)));
    if oracle {
        out.extend(keep.into_iter().map(Decision::Enter));
        return out;
    }
    let mut victims: Vec<Held> = held.to_vec();
    victims.sort_by(|a, b| a.hold_sol.total_cmp(&b.hold_sol).then(a.coin.cmp(&b.coin)));
    let mut vi = 0;
    for i in keep {
        let c = &cands[i];
        let b = cand_bet(st, c, cap.equity);
        if st.kelly_frac.is_some() && b < st.kelly_min_bet_sol {
            out.push(Decision::Block(i, if c.kelly > 0.0 { "kelly_bet_too_small" } else { "kelly_no_edge" }));
            continue;
        }
        if cap.entries_60s >= st.max_entries_per_min {
            out.push(Decision::Block(i, "max_entries_per_min"));
            continue;
        }
        let exp_full = cap.open_cost + b > st.max_total_exposure * cap.equity;
        let slots_full = cap.slots_used >= st.max_positions;
        let cash_short = cap.cash_free < b + 3.0 * txc;
        if !exp_full && !slots_full && !cash_short {
            out.push(Decision::Enter(i));
            cap.open_cost += b;
            cap.slots_used += 1;
            cap.cash_free -= b + txc;
            cap.entries_60s += 1;
            continue;
        }
        if can_replace && vi < victims.len() {
            let w = victims[vi];
            let better = b * c.ev - txc > w.hold_sol + st.replace_margin * b;
            let exp_ok = cap.open_cost - w.cost_share + b <= st.max_total_exposure * cap.equity;
            let cash_ok = cap.cash_free + w.value - txc >= b + 3.0 * txc;
            if better && exp_ok && cash_ok {
                out.push(Decision::Replace(i, w.coin));
                vi += 1;
                cap.open_cost += b - w.cost_share;
                cap.cash_free += w.value - 2.0 * txc - b;
                cap.entries_60s += 1;
                continue;
            }
        }
        out.push(Decision::Block(i, if exp_full { "exposure_cap" } else if slots_full { "slots_full" } else { "no_cash" }));
    }
    out
}

enum Order {
    Buy { coin: u32, lamports: u64, ev: f64, label: Option<Labels>, min_tokens: u128 },
    Sell { coin: u32, tokens: u64, reason: &'static str, close: bool, min_sol: f64, tries: u8 },
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
    low300: usize,
    buyers60: usize,
}

fn idx() -> Idx {
    let f = |n: &str| FEATURES.iter().position(|x| *x == n).unwrap_or_else(|| panic!("feature {n}"));
    Idx { ret_60s: f("ret_60s"), top10: f("top10_supply_share"), bundle: f("f2_bundle_buy_share"), vol: f("vol_60s"), low300: f("price_vs_low300"), buyers60: f("buyers_60s") }
}

/// Simulate one window. `rk` = net return per entry bin (from `estimate_rk`). `oracle` = enter every
/// in-window signal with a fixed bet, no gates, unlimited cash and slots (used to measure R_k).
#[allow(clippy::too_many_arguments)]
pub fn simulate(w: &WindowData, rp: RadarParams, scorer: &mut dyn Scorer, st: &StratV5, c: &CostsV5, rk: &[f64; 7], hold: Option<&HoldTable>, oracle: bool) -> ReportV5 {
    simulate_k(w, rp, scorer, st, c, rk, hold, None, oracle)
}

/// `simulate` with an optional Kelly table (used when `st.kelly_frac` is set).
#[allow(clippy::too_many_arguments)]
pub fn simulate_k(w: &WindowData, rp: RadarParams, scorer: &mut dyn Scorer, st: &StratV5, c: &CostsV5, rk: &[f64; 7], hold: Option<&HoldTable>,
                  kelly: Option<&KellyTable>, oracle: bool) -> ReportV5 {
    let ev = w.ev;
    let ix = idx();
    let mut eng = Engine::new(ev, rp).with_ctx(w.ctx.clone()).with_c18_venues(st.c18_venues);
    let start = c.start_sol;
    let mut cash = start;
    let txc = c.tx_sol();
    let mut pos: FxHashMap<u32, Pos> = FxHashMap::default();
    let mut pending: Vec<(u64, Order)> = Vec::new();
    let mut busy: FxHashSet<u32> = FxHashSet::default();
    let mut pending_buy_cost = 0.0;
    let mut blacklist: FxHashSet<u32> = FxHashSet::default();
    let mut last_exit: FxHashMap<u32, i64> = FxHashMap::default();
    let mut entries: VecDeque<i64> = VecDeque::new();
    let mut trades: Vec<TradeV5> = Vec::new();
    let (mut failed, mut costs) = (0usize, 0.0);
    let (mut slippage_failed, mut sell_retries) = (0usize, 0usize);
    // E9 account risk state: equity at the last 1-s mark, consecutive losing trades, entry pause end
    let (mut last_eq, mut losing_run, mut paused_until) = (start, 0u32, None::<i64>);
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
    let (mut replacements, mut merged_in_tick) = (0usize, 0usize);
    // V5-1: candidates of the open decision tick (decided when the tick closes, causally).
    let mut cands: Vec<Cand> = Vec::new();
    let mut cand_tick = i64::MIN;
    let (mut last_slot, mut last_t) = (0u64, i64::MIN);
    let tick_of = |slot: u64, t: i64| -> i64 { if st.decision_tick_s > 0 { t.div_euclid(st.decision_tick_s) } else { slot as i64 } };
    let evs = &ev.evs;
    let mut i = 0usize;
    loop {
        let (cur_slot, cur_t) = if i < evs.len() { (evs[i].slot, evs[i].time) } else { (u64::MAX, ev.data_end() + 1) };
        // 0) Close the decision tick: rank its candidates, then apply caps (only events of the closed tick were applied).
        if !cands.is_empty() && (i >= evs.len() || tick_of(cur_slot, cur_t) != cand_tick) {
            let tick = std::mem::take(&mut cands);
            if i >= evs.len() {
                for cd in &tick {
                    *blocked.entry("data_end").or_default() += 1;
                    outcomes.push((cd.coin, "data_end"));
                }
            } else {
                let mut live = Vec::with_capacity(tick.len());
                for cd in tick {
                    if let Some(r) = eng.untradable(cd.coin, MIN_EXIT_LIQ_SOL) {
                        *blocked.entry(r).or_default() += 1;
                        outcomes.push((cd.coin, r));
                    } else {
                        live.push(cd);
                    }
                }
                while entries.front().is_some_and(|t| *t <= last_t - 60) {
                    entries.pop_front();
                }
                let n_pending_buys = busy.iter().filter(|b| !pos.contains_key(b)).count();
                let open_cost: f64 = pos.values().map(|p| p.cost * p.tokens as f64 / p.tokens0.max(1) as f64).sum::<f64>() + pending_buy_cost;
                let equity = cash + pos.iter().map(|(cid, p)| eng.pool_value(*cid, p.tokens, c.default_fee_bps)).sum::<f64>();
                let cap = Capacity {
                    open_cost, equity, cash_free: cash - pending_buy_cost - n_pending_buys as f64 * txc, slots_used: pos.len() + n_pending_buys, entries_60s: entries.len(),
                };
                let held: Vec<Held> = match hold {
                    Some(h) => pos.iter().filter(|(cid, _)| !busy.contains(cid)).map(|(cid, p)| {
                        let value = eng.pool_value(*cid, p.tokens, c.default_fee_bps);
                        Held { coin: *cid, hold_sol: value * h.value(p.last_hold), cost_share: p.cost * p.tokens as f64 / p.tokens0.max(1) as f64, value }
                    }).collect(),
                    None => Vec::new(),
                };
                // compounding: the bet of this tick is a fraction of the current equity (else the fixed bet_sol)
                let st_tick = StratV5 { bet_sol: st.bet_equity_frac.map_or(st.bet_sol, |f| (f * equity).max(0.0)), ..*st };
                let st = &st_tick;
                for d in allocate(&live, &held, cap, st, txc, hold.is_some(), oracle) {
                    let (ci, victim) = match d {
                        Decision::Merged(k) => {
                            merged_in_tick += 1;
                            outcomes.push((live[k].coin, "merged_in_tick"));
                            continue;
                        }
                        Decision::Block(k, r) => {
                            *blocked.entry(r).or_default() += 1;
                            outcomes.push((live[k].coin, r));
                            continue;
                        }
                        Decision::Enter(k) => (k, None),
                        Decision::Replace(k, wc) => (k, Some(wc)),
                    };
                    if let Some(wc) = victim {
                        busy.insert(wc);
                        let t = pos[&wc].tokens;
                        let min_sol = eng.pool_value(wc, t, c.default_fee_bps) * (1.0 - c.sell_slippage);
                        pending.push((last_slot + c.latency_slots, Order::Sell { coin: wc, tokens: t, reason: "Replaced", close: true, min_sol, tries: 1 }));
                        replacements += 1;
                    }
                    let cd = live[ci];
                    outcomes.push((cd.coin, "entered"));
                    busy.insert(cd.coin);
                    let bet = if oracle { st.bet_sol } else { cand_bet(st, &cd, equity) };
                    pending_buy_cost += bet;
                    if !oracle {
                        entries.push_back(last_t);
                    }
                    let lamports = (bet * 1e9) as u64;
                    let min_tokens = (eng.fill_buy(cd.coin, lamports, c.default_fee_bps) as f64 * (1.0 - c.buy_slippage)) as u128;
                    pending.push((last_slot + c.latency_slots, Order::Buy { coin: cd.coin, lamports, ev: cd.ev, label: cd.label, min_tokens }));
                }
            }
        }
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
                Order::Sell { coin, tokens, reason, close, min_sol, tries } => {
                    let Some(p) = pos.get_mut(&coin) else { continue };
                    let n = tokens.min(p.tokens);
                    let got = eng.pool_value(coin, n, c.default_fee_bps);
                    let random_fail = hash01(coin as u64 ^ 0x5e11_5e11, cur_slot, c.fail_seed) < c.sell_fail_prob;
                    if tries < c.sell_max_tries && (random_fail || got < min_sol) {
                        // failed sell tx: fee paid, retried; the last try has no limit
                        cash -= txc;
                        costs += txc;
                        p.tx_paid += txc;
                        sell_retries += 1;
                        let next = tries + 1;
                        let min_sol = if next >= c.sell_max_tries { 0.0 } else { got * (1.0 - c.sell_slippage) };
                        pending.push((cur_slot + c.latency_slots, Order::Sell { coin, tokens, reason, close, min_sol, tries: next }));
                        continue;
                    }
                    cash += got - txc;
                    costs += txc;
                    p.tx_paid += txc;
                    p.tokens -= n;
                    p.proceeds += got;
                    p.legs.push(Leg { t: cur_t - ev.window.0, reason, tokens_frac: n as f64 / p.tokens0.max(1) as f64, proceeds_sol: got });
                    *exits.entry(reason).or_default() += 1;
                    if close || p.tokens == 0 {
                        busy.remove(&coin);
                        last_exit.insert(coin, cur_t);
                        // token account closed with the final sell: rent back
                        cash += c.ata_rent_lamports as f64 / 1e9;
                        let p = pos.remove(&coin).expect("present");
                        if reason == "DevSell" || reason == "RugModel" {
                            blacklist.insert(coin);
                        }
                        let pnl = p.proceeds - p.cost - p.tx_paid;
                        losing_run = if pnl < 0.0 { losing_run + 1 } else { 0 };
                        if st.loss_streak > 0 && losing_run >= st.loss_streak {
                            paused_until = Some(cur_t + st.loss_streak_pause_s);
                            losing_run = 0;
                        }
                        let hold_obs = forward_returns(&p.marks, &p.legs, txc);
                        trades.push(TradeV5 {
                            mint: ev.mints[coin as usize].clone(), entry_t: p.entry_t - ev.window.0, exit_t: cur_t - ev.window.0, cost_sol: p.cost,
                            proceeds_sol: p.proceeds, tx_sol: p.tx_paid, pnl_sol: pnl, ret: pnl / p.cost, max_mult: p.max_mult, final_reason: reason,
                            entry_ev: p.entry_ev, true_bin: p.entry_label.map(|l| l.bin), true_logmax: p.entry_label.map(|l| l.logmax), legs: p.legs, hold_obs,
                        });
                    } else {
                        busy.remove(&coin);
                    }
                }
                Order::Buy { coin, lamports, ev: e_ev, label, min_tokens } => {
                    busy.remove(&coin);
                    let size = lamports as f64 / 1e9;
                    pending_buy_cost -= size;
                    cash -= txc;
                    costs += txc;
                    let toks = eng.fill_buy(coin, lamports, c.default_fee_bps);
                    let rent = c.ata_rent_lamports as f64 / 1e9;
                    if hash01(coin as u64, cur_slot, c.fail_seed) < c.buy_fail_prob || toks == 0 || (!oracle && cash < size + rent) {
                        failed += 1;
                        continue;
                    }
                    if toks < min_tokens {
                        failed += 1;
                        slippage_failed += 1;
                        continue;
                    }
                    cash -= size + rent;
                    let px = eng.price(coin);
                    pos.insert(coin, Pos {
                        tokens0: toks as u64, tokens: toks as u64, cost: size, entry_t: cur_t, entry_px: lamports as f64 / toks as f64, peak_px: px, max_mult: 1.0,
                        stage: Stage::PreTp, proceeds: 0.0, tx_paid: txc, legs: Vec::new(), last: Pred::default(), last_vol: 0.0, last_buyers60: 0.0, entry_ev: e_ev,
                        entry_label: label, q_hist: VecDeque::new(), last_hold: None, marks: Vec::new(),
                        fade: Fade { rate_t: cur_t, ..Default::default() },
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
                pending.push((0, Order::Sell { coin, tokens: t, reason: "DataEnd", close: true, min_sol: 0.0, tries: c.sell_max_tries }));
            }
            continue;
        }
        let e = &evs[i];
        i += 1;
        (last_slot, last_t) = (e.slot, e.time);
        let step = eng.apply(e);
        // 2) Once per second: max hold, data end of a coin, equity.
        if e.time > last_sec {
            last_sec = e.time;
            let ids: Vec<u32> = pos.keys().copied().filter(|c| !busy.contains(c)).collect();
            for coin in ids {
                let p = &pos[&coin];
                let normal = st.exit_mode != ExitMode::Ladder;
                let reason = if e.time - p.entry_t >= st.max_hold_s {
                    Some("MaxHold")
                } else if e.time > ev.cover_end[coin as usize] {
                    Some("DataEnd")
                } else if normal && matches!(st.exit_mode, ExitMode::MomentumFade | ExitMode::FadeAndDrawdown) && eng.secs_since_real_buy(coin, e.time) > st.idle_s {
                    Some("Dead")
                } else if normal && st.time_stop_s > 0 && e.time - p.entry_t >= st.time_stop_s && p.max_mult < 1.0 + st.time_stop_gain
                    && p.last_buyers60 <= st.time_stop_max_buyers60 {
                    Some("TimeStop")
                } else {
                    None
                };
                if let Some(r) = reason {
                    busy.insert(coin);
                    let min_sol = eng.pool_value(coin, p.tokens, c.default_fee_bps) * (1.0 - c.sell_slippage);
                    pending.push((e.slot + c.latency_slots, Order::Sell { coin, tokens: p.tokens, reason: r, close: true, min_sol, tries: 1 }));
                }
            }
            let eq = cash + pos.iter().map(|(cid, p)| eng.pool_value(*cid, p.tokens, c.default_fee_bps)).sum::<f64>();
            last_eq = eq;
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
                p.last_buyers60 = x[ix.buyers60] as f64;
                p.last_hold = Some(pr.hold);
            }
            if oracle && step.snapshot.is_some() {
                p.marks.push((HoldTable::bucket(pred.map(|q| q.hold)) as u8, val, p.legs.len() as u32));
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
            } else if st.exit_mode != ExitMode::Ladder {
                let held_s = e.time - p.entry_t;
                let fnow = (e.sol >= (REAL_MIN_SOL * 1e9) as u64).then(|| p.fade.on_trade(e.time, e.buy, e.sol as f64 / 1e9, st.fade_tau_s, st));
                if st.disaster_stop.is_some_and(|d| held_s >= st.stop_grace_s && val <= (1.0 - d) * p.cost) {
                    sell = Some((p.tokens, "DisasterStop", true));
                } else if st.exit_mode == ExitMode::Trail {
                    if price <= p.peak_px * (1.0 - st.trail_fixed.min(0.99)) {
                        sell = Some((p.tokens, "Trail", true));
                    }
                } else if st.exit_mode == ExitMode::AdaptiveTrail {
                    let d = (1.0 - (-st.trail_k * p.last_vol * 60f64.sqrt()).exp()).clamp(st.d_min, st.d_max);
                    if price <= p.peak_px * (1.0 - d) {
                        sell = Some((p.tokens, "AdaptiveTrail", true));
                    }
                } else if let Some(f) = fnow {
                    let crowd_down = f.sellers && !(f.quiet || f.smaller) && eng.radar().view(e.coin, e.time).is_some_and(|v| v.metrics.growth < 1.0);
                    let mut cond = f.sellers && (f.quiet || f.smaller || crowd_down) && held_s >= st.fade_min_hold_s;
                    if cond && st.exit_mode == ExitMode::FadeAndDrawdown {
                        let sigma_h = p.last_vol * 60f64.sqrt();
                        let d = (1.0 - (-st.trail_k * sigma_h).exp()).clamp(st.d_min, st.d_max);
                        cond = p.peak_px > 0.0 && price <= p.peak_px * (1.0 - d);
                    }
                    if cond {
                        let since = *p.fade.since.get_or_insert(e.time);
                        if e.time - since >= st.fade_persist_s {
                            sell = Some((p.tokens, "MomentumFade", true));
                        }
                    } else {
                        p.fade.since = None;
                    }
                }
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
                    let min_sol = eng.pool_value(e.coin, n, c.default_fee_bps) * (1.0 - c.sell_slippage);
                    pending.push((e.slot + lat, Order::Sell { coin: e.coin, tokens: n, reason: r, close, min_sol, tries: 1 }));
                }
            }
            continue;
        }
        // 4) Entry: radar decision rows of HOT coins (or Promote rows only) inside the decision window.
        let state_ok = step.radar_state == radar::RadarState::Hot || (st.entry_on_watch && step.radar_state == radar::RadarState::Watch);
        let entry_row = if st.decision_rows { step.decision && state_ok } else { step.signal };
        if !entry_row || e.time > ev.window.1 {
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
        if last_exit.get(&e.coin).is_some_and(|t| e.time - t < st.reentry_cooldown_s) {
            block("reentry_cooldown");
            continue;
        }
        if let Some(r) = eng.untradable(e.coin, MIN_EXIT_LIQ_SOL) {
            block(r);
            continue;
        }
        if st.mint_gate && eng.mint_unsafe(e.coin) {
            block("mint_unsafe");
            continue;
        }
        if st.max_loss_frac.is_some_and(|x| last_eq <= start * (1.0 - x)) {
            block("risk_loss_limit");
            continue;
        }
        if paused_until.is_some_and(|t| e.time < t) {
            block("risk_loss_streak");
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
            } else if st.max_entry_ret.is_some_and(|r| x[ix.low300] as f64 > 1.0 + r) {
                Some("price_guard")
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
        }
        if cands.is_empty() {
            cand_tick = tick_of(e.slot, e.time);
        }
        let kf = kelly.map_or(f64::NAN, |k| k.f_star(&pr.bin));
        cands.push(Cand { coin: e.coin, ev: e_ev, order: e.order, label, kelly: kf });
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
        slippage_failed_buys: slippage_failed,
        sell_retries,
        exits_by_reason: exits,
        final_reasons,
        runners_10x: runner.len(),
        runners_10x_captured_5x: caught,
        moonshot_capture: if runner.is_empty() { 0.0 } else { caught as f64 / runner.len() as f64 },
        signals_in_window: signals,
        blocked,
        replacements,
        merged_in_tick,
        trades_list: trades,
        signal_outcomes: outcomes,
    }
}

/// R_k (§7.2): mean net return of this exit policy per true entry bin, from oracle runs (every in-window signal
/// entered, fixed bet) on TRAINING windows only. Each training window uses its own out-of-sample scores when
/// given (so model exits act as in the test), else no model (price exits only). Bins with few trades are shrunk
/// toward the pooled mean (prior weight 5 trades). Returns (R, trades per bin, hold table from the same runs).
pub fn estimate_rk(train: &[(&WindowData, Option<&FileScores>)], rp: RadarParams, st: &StratV5, c: &CostsV5) -> ([f64; 7], [usize; 7], Option<HoldTable>) {
    let (rk, cnt, hold, _) = estimate_rk_kelly(train, rp, st, c);
    (rk, cnt, hold)
}

/// `estimate_rk` plus the Kelly table fitted on the same training trades.
pub fn estimate_rk_kelly(train: &[(&WindowData, Option<&FileScores>)], rp: RadarParams, st: &StratV5, c: &CostsV5) -> ([f64; 7], [usize; 7], Option<HoldTable>, Option<KellyTable>) {
    use rayon::prelude::*;
    let reps: Vec<ReportV5> = train.par_iter().map(|(w, sc)| match sc {
        Some(sc) => simulate(w, rp, &mut FileScoresRef(sc), st, c, &[0.0; 7], None, true),
        None => simulate(w, rp, &mut NoScores, st, c, &[0.0; 7], None, true),
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
    let obs: Vec<(u8, f64, f64)> = reps.iter().flat_map(|r| r.trades_list.iter().flat_map(|t| t.hold_obs.iter().copied())).collect();
    let rets: Vec<(u8, f64)> = reps.iter().flat_map(|r| r.trades_list.iter().filter_map(|t| t.true_bin.map(|b| (b, t.ret)))).collect();
    (rk, cnt, HoldTable::fit(&obs), KellyTable::fit(&rets))
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
             quote_res: q, token_res: tk, real_quote: 0, real_token: 0, fee_bps: 100, complete: false, venue: 1, pool: 0}
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
        let st = StratV5 { max_ret_60s_entry: 10.0, max_top10_share: 1.0, max_bundle_share: 1.0, exit_mode: ExitMode::Ladder, max_entry_ret: None, ..Default::default() };
        let r = simulate(&w, rp, &mut Fixed(pr), &st, &c, &rk, None, false);
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

    /// Wallet execution: a zero buy-slippage limit fails on a rising price (fee paid, no position); failing sells are
    /// retried (each try pays its fee) and the last try has no limit; ATA rent is locked and refunded.
    /// Kelly: no edge -> 0; a bin with sure +100 % and one with -100 % at 50/50 -> f* = 0 (log growth <= 0);
    /// 60 % +100 % / 40 % -50 % -> f* = p/b_loss - q/b_win = 0.6/0.5 - 0.4/1 = 0.8; quarter Kelly sizes on equity.
    #[test]
    fn kelly_fraction_and_bet() {
        let rets: Vec<(u8, f64)> = (0..1000).map(|i| if i % 2 == 0 { (6u8, 1.0) } else { (0u8, -0.5) }).collect();
        let k = KellyTable::fit(&rets).unwrap();
        let p = |a: f32| { let mut x = [0f32; 7]; x[6] = a; x[0] = 1.0 - a; x };
        assert_eq!(k.f_star(&p(0.0)), 0.0, "only losers: no bet");
        assert!((k.f_star(&p(0.6)) - 0.8).abs() < 0.02, "{}", k.f_star(&p(0.6)));
        assert!(k.f_star(&p(0.3)) == 0.0, "negative edge");
        let st = StratV5 { kelly_frac: Some(0.25), ..Default::default() };
        let c = Cand { coin: 0, ev: 0.0, order: 0, label: None, kelly: 0.8 };
        assert!((cand_bet(&st, &c, 2.0) - 0.4).abs() < 1e-12);
        assert_eq!(cand_bet(&st, &Cand { kelly: f64::NAN, ..c }, 2.0), st.bet_sol, "no table: fixed bet");
    }

    #[test]
    fn wallet_execution_slippage_retries_and_rent() {
        let mut ev = Events::new(runner_events(), vec!["M".into()], (0, 120), vec![]);
        ev.cover_end = vec![100_000];
        let rp = RadarParams { min_buyers: 5, min_buy_sol: 1.0, max_median_buy_sol: 1.0, ..RadarParams::default() };
        let w = WindowData::new("t".into(), &ev, Context::default(), 200.0);
        let pr = Pred { bin: [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], tp1: 0.9, rug: 0.0, hold: 0.0, dead: 0.0 };
        let rk = [0.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0];
        let st = StratV5 { max_ret_60s_entry: 10.0, max_top10_share: 1.0, max_bundle_share: 1.0, exit_mode: ExitMode::Ladder, max_entry_ret: None, ..Default::default() };
        // 1) no slippage allowed: the price rises during the 2-slot latency -> every buy fails on the limit
        let c = CostsV5 { buy_fail_prob: 0.0, buy_slippage: 0.0, sell_fail_prob: 0.0, ..Default::default() };
        let r = simulate(&w, rp, &mut Fixed(pr), &st, &c, &rk, None, false);
        assert_eq!(r.trades, 0, "{r:?}");
        assert!(r.slippage_failed_buys >= 1 && r.slippage_failed_buys == r.failed_buys, "{r:?}");
        assert!((r.final_sol - (r.start_sol - r.failed_buys as f64 * c.tx_sol())).abs() < 1e-9, "failed buys pay only the fee");
        // 2) every sell try fails at random until the last (no-limit) try: 3 tries per sell leg
        let c = CostsV5 { buy_fail_prob: 0.0, sell_fail_prob: 1.0, sell_max_tries: 3, ..Default::default() };
        let r = simulate(&w, rp, &mut Fixed(pr), &st, &c, &rk, None, false);
        assert_eq!(r.trades, 1, "{:?}", r.blocked);
        let t = &r.trades_list[0];
        // DataEnd closes at the end of the data in one (no-limit) try
        let legs = t.legs.iter().filter(|l| l.reason != "DataEnd").count();
        let end_legs = t.legs.len() - legs;
        assert!(legs >= 1);
        assert_eq!(r.sell_retries, 2 * legs, "{r:?}");
        assert!((t.tx_sol - (1 + 3 * legs + end_legs) as f64 * c.tx_sol()).abs() < 1e-12);
        // rent locked at the buy is refunded at the close: cash accounting closes on pnl alone
        assert!((r.final_sol - (r.start_sol + t.pnl_sol)).abs() < 1e-9, "cash accounting with rent");
    }

    /// E9: a loss limit already hit (0 % allowed loss: equity <= start) blocks every entry; a loose limit changes nothing.
    #[test]
    fn account_loss_limit_blocks_entries() {
        let mut ev = Events::new(runner_events(), vec!["M".into()], (0, 120), vec![]);
        ev.cover_end = vec![100_000];
        let rp = RadarParams { min_buyers: 5, min_buy_sol: 1.0, max_median_buy_sol: 1.0, ..RadarParams::default() };
        let w = WindowData::new("t".into(), &ev, Context::default(), 200.0);
        let pr = Pred { bin: [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], tp1: 0.9, rug: 0.0, hold: 0.0, dead: 0.0 };
        let rk = [0.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0];
        let c = CostsV5 { buy_fail_prob: 0.0, sell_fail_prob: 0.0, ..Default::default() };
        let base = StratV5 { max_ret_60s_entry: 10.0, max_top10_share: 1.0, max_bundle_share: 1.0, exit_mode: ExitMode::Ladder, max_entry_ret: None, ..Default::default() };
        let r0 = simulate(&w, rp, &mut Fixed(pr), &base, &c, &rk, None, false);
        assert_eq!(r0.trades, 1);
        let r = simulate(&w, rp, &mut Fixed(pr), &StratV5 { max_loss_frac: Some(0.0), ..base }, &c, &rk, None, false);
        assert_eq!(r.trades, 0);
        assert!(r.blocked.get("risk_loss_limit").copied().unwrap_or(0) >= 1, "{:?}", r.blocked);
        let r = simulate(&w, rp, &mut Fixed(pr), &StratV5 { max_loss_frac: Some(0.9), loss_streak: 3, ..base }, &c, &rk, None, false);
        assert_eq!(r.trades, r0.trades);
        assert!((r.final_sol - r0.final_sol).abs() < 1e-12);
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
        let r = simulate(&w, rp, &mut Fixed(pr), &st, &c, &[0.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0], None, false);
        assert_eq!(r.trades, 1, "{:?}", r.blocked);
        let t = &r.trades_list[0];
        assert_eq!(t.final_reason, "DevSell", "{t:?}");
        assert!(t.exit_t >= 70 && t.exit_t < 90, "{t:?}");
    }

    fn cand(coin: u32, ev: f64, order: u64) -> Cand {
        Cand { coin, ev, order, label: None, kelly: f64::NAN }
    }

    fn cap_for(n_bets: f64, st: &StratV5) -> Capacity {
        // equity such that exactly `n_bets` bets fit under max_total_exposure
        Capacity { open_cost: 0.0, equity: (n_bets + 0.5) * st.bet_sol / st.max_total_exposure, cash_free: 10.0, slots_used: 0, entries_60s: 0 }
    }

    #[test]
    fn allocate_ranks_by_ev_not_arrival() {
        let st = StratV5::default();
        let d = allocate(&[cand(1, 0.2, 1), cand(2, 0.9, 2)], &[], cap_for(1.0, &st), &st, 0.0005, false, false);
        assert_eq!(d, vec![Decision::Enter(1), Decision::Block(0, "exposure_cap")]);
    }

    /// The V5-1 scenario: slots are free (10 < 18) but exposure binds; the weakest held position is replaced.
    #[test]
    fn allocate_replaces_when_exposure_is_binding() {
        let st = StratV5::default();
        let cap = Capacity { open_cost: 0.5, equity: 0.9, cash_free: 0.4, slots_used: 10, entries_60s: 0 };
        let held = [Held { coin: 7, hold_sol: 0.004, cost_share: 0.05, value: 0.05 }, Held { coin: 8, hold_sol: -0.02, cost_share: 0.05, value: 0.03 }];
        let d = allocate(&[cand(1, 0.6, 1)], &held, cap, &st, 0.0005, true, false);
        assert_eq!(d, vec![Decision::Replace(0, 8)]);
        let d = allocate(&[cand(1, 0.6, 1)], &held, cap, &st, 0.0005, false, false);
        assert_eq!(d, vec![Decision::Block(0, "exposure_cap")], "no hold table -> never replaces");
    }

    #[test]
    fn allocate_replacement_compares_expected_sol() {
        let st = StratV5 { bet_sol: 0.05, replace_margin: 0.15, ..Default::default() };
        // exposure binds (0.55 > 0.6 x 0.85 = 0.51) but the swap keeps it at 0.5 <= 0.51
        let cap = Capacity { open_cost: 0.5, equity: 0.85, cash_free: 0.4, slots_used: 10, entries_60s: 0 };
        // b * ev - txc = 0.0145; threshold = hold_sol + 0.0075
        for (hs, want) in [(0.005, true), (0.008, false)] {
            let held = [Held { coin: 9, hold_sol: hs, cost_share: 0.05, value: 0.05 }];
            let d = allocate(&[cand(1, 0.3, 1)], &held, cap, &st, 0.0005, true, false);
            assert_eq!(d[0] == Decision::Replace(0, 9), want, "{hs}: {d:?}");
        }
    }

    #[test]
    fn allocate_dedupes_coin_keeps_latest_row() {
        let st = StratV5::default();
        let d = allocate(&[cand(1, 0.9, 5), cand(1, 0.3, 9), cand(2, 0.5, 6)], &[], cap_for(5.0, &st), &st, 0.0005, false, false);
        assert_eq!(d, vec![Decision::Merged(0), Decision::Enter(2), Decision::Enter(1)]);
    }

    #[test]
    fn allocate_respects_entries_per_minute_in_rank_order() {
        let st = StratV5 { max_entries_per_min: 1, ..Default::default() };
        let d = allocate(&[cand(1, 0.2, 1), cand(2, 0.9, 2), cand(3, 0.5, 3)], &[], cap_for(5.0, &st), &st, 0.0005, false, false);
        assert_eq!(d, vec![Decision::Enter(1), Decision::Block(2, "max_entries_per_min"), Decision::Block(0, "max_entries_per_min")]);
    }

    #[test]
    fn allocate_cash_reserved_by_pending_buys() {
        let st = StratV5::default();
        let cap = Capacity { open_cost: 0.0, equity: 10.0, cash_free: 0.06, slots_used: 0, entries_60s: 0 };
        let d = allocate(&[cand(1, 0.9, 1), cand(2, 0.5, 2)], &[], cap, &st, 0.0005, false, false);
        assert_eq!(d, vec![Decision::Enter(0), Decision::Block(1, "no_cash")]);
    }

    #[test]
    fn hold_table_fit_and_lookup() {
        let h = HoldTable::fit(&[(0, -0.5, 1.0), (9, 1.0, 1.0)]).unwrap();
        assert!((h.r[0] - 0.125).abs() < 1e-12 && (h.r[9] - 0.375).abs() < 1e-12 && (h.r[5] - 0.25).abs() < 1e-12);
        assert!((h.value(None) - 0.25).abs() < 1e-12 && (h.value(Some(f32::NAN)) - 0.25).abs() < 1e-12);
        assert!((h.value(Some(1.0)) - 0.375).abs() < 1e-12, "p = 1 falls in the top decile");
        assert!(HoldTable::fit(&[]).is_none());
    }

    #[test]
    fn forward_returns_from_marks() {
        let leg = |p: f64| Leg { t: 0, reason: "x", tokens_frac: 0.5, proceeds_sol: p };
        let r = forward_returns(&[(3, 0.10, 0), (7, 0.20, 1)], &[leg(0.10), leg(0.15)], 0.0005);
        assert_eq!(r.len(), 2);
        assert!(r[0].0 == 3 && (r[0].1 - 1.49).abs() < 1e-12 && (r[0].2 - 0.5).abs() < 1e-12);
        assert!(r[1].0 == 7 && (r[1].1 + 0.2525).abs() < 1e-12);
    }

    fn mkc(slot: u64, time: i64, coin: u32, user: u32, q: u128) -> Ev {
        let mut e = mk(slot, time, user, true, 300_000_000, q, 1_000_000_000_000_000);
        e.coin = coin;
        e
    }

    /// Scores by pool size: >= 70 SOL quote reserve -> bin 5, else bin 1.
    struct ByPool;
    impl Scorer for ByPool {
        fn score(&mut self, _o: u64, x: &[f32]) -> Option<Pred> {
            let mut bin = [0f32; 7];
            bin[if x[3] >= 70.0 { 5 } else { 1 }] = 1.0;
            Some(Pred { bin, ..Default::default() })
        }
    }

    /// Two coins promoted in the same decision tick with room for one bet: the higher-EV coin enters even though
    /// the other was promoted first. With per-slot batching (tick 0) the earlier coin enters (first-come).
    #[test]
    fn two_coins_same_tick_higher_ev_wins() {
        let mut evs = Vec::new();
        let mut slot = 100;
        for s in 0..30i64 {
            for u in 0..3u32 {
                slot += 1;
                evs.push(mkc(slot, s, 1, 1000 + (s as u32) * 3 + u, 50_000_000_000)); // low-EV coin first in each second
                slot += 1;
                evs.push(mkc(slot, s, 0, (s as u32) * 3 + u, 80_000_000_000));
            }
        }
        let mut ev = Events::new(evs, vec!["A".into(), "B".into()], (0, 60), vec![]);
        ev.cover_end = vec![100_000, 100_000];
        let rp = RadarParams { min_buyers: 5, min_buy_sol: 1.0, max_median_buy_sol: 1.0, ..RadarParams::default() };
        let w = WindowData::new("t".into(), &ev, Context::default(), f64::NAN);
        let c = CostsV5 { buy_fail_prob: 0.0, start_sol: 0.1, ..Default::default() };
        let rk = [0.0, 0.2, 0.0, 0.0, 0.0, 3.0, 0.0];
        let outcome = |tick: i64| {
            let st = StratV5 { max_ret_60s_entry: 10.0, max_top10_share: 1.0, max_bundle_share: 1.0, decision_tick_s: tick, ..Default::default() };
            let r = simulate(&w, rp, &mut ByPool, &st, &c, &rk, None, false);
            // first decided outcome per coin (several decision rows of one coin in a tick are merged)
            let get = |coin: u32| r.signal_outcomes.iter().find(|x| x.0 == coin && x.1 != "merged_in_tick").map(|x| x.1);
            (get(0), get(1))
        };
        assert_eq!(outcome(10), (Some("entered"), Some("exposure_cap")));
        assert_eq!(outcome(0), (Some("exposure_cap"), Some("entered")));
    }

    #[test]
    fn estimate_rk_returns_hold_table() {
        let mut ev = Events::new(runner_events(), vec!["M".into()], (0, 120), vec![]);
        ev.cover_end = vec![100_000];
        let rp = RadarParams { min_buyers: 5, min_buy_sol: 1.0, max_median_buy_sol: 1.0, ..RadarParams::default() };
        let w = WindowData::new("t".into(), &ev, Context::default(), f64::NAN);
        let (rk, cnt, hold) = estimate_rk(&[(&w, None)], rp, &StratV5::default(), &CostsV5 { buy_fail_prob: 0.0, ..Default::default() });
        assert_eq!(cnt.iter().sum::<usize>(), 1);
        assert!(rk.iter().all(|x| x.is_finite()));
        let h = hold.expect("hold table from the oracle run");
        // NoScores predicts hold = 0 at every held snapshot -> bucket 0
        assert!(h.r.iter().all(|x| x.is_finite()) && h.w[0] > 0.0 && (h.w.iter().sum::<f64>() - h.w[0]).abs() < 1e-12);
    }

    /// A coin exited by Stop is not re-entered within reentry_cooldown_s, although decision rows keep coming.
    #[test]
    fn decision_rows_reentry_cooldown() {
        let mut evs = Vec::new();
        let tk: u128 = 1_000_000_000_000_000;
        let mut slot = 100;
        let mut q: u128 = 50_000_000_000;
        for s in 0..400i64 {
            for u in 0..3u32 {
                slot += 1;
                if s > 30 && s < 60 {
                    q = (q as f64 * 0.99) as u128; // dump right after entry -> Stop
                }
                evs.push(mk(slot, s, (s as u32) * 3 + u, true, 300_000_000, q, tk));
            }
        }
        let mut ev = Events::new(evs, vec!["M".into()], (0, 400), vec![]);
        ev.cover_end = vec![100_000];
        let rp = RadarParams { min_buyers: 5, min_buy_sol: 1.0, max_median_buy_sol: 1.0, ..RadarParams::default() };
        let w = WindowData::new("t".into(), &ev, Context::default(), f64::NAN);
        let pr = Pred { bin: [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], ..Default::default() };
        let c = CostsV5 { buy_fail_prob: 0.0, ..Default::default() };
        let base = StratV5 { max_ret_60s_entry: 10.0, max_top10_share: 1.0, max_bundle_share: 1.0, stop_grace_s: 5, exit_mode: ExitMode::Ladder, max_entry_ret: None, ..Default::default() };
        let r = simulate(&w, rp, &mut Fixed(pr), &base, &c, &[0.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0], None, false);
        assert!(r.trades >= 1 && r.trades_list[0].final_reason == "Stop", "{:?}", r.trades_list);
        let exit = r.trades_list[0].exit_t;
        assert!(r.blocked.get("reentry_cooldown").copied().unwrap_or(0) > 0, "{:?}", r.blocked);
        assert!(r.trades_list.iter().skip(1).all(|t| t.entry_t >= exit + base.reentry_cooldown_s), "{:?}", r.trades_list);
    }

    fn runner_with_dump() -> Events {
        let mut evs = runner_events();
        let mut q = evs.last().unwrap().quote_res;
        let mut slot = evs.last().unwrap().slot;
        for s in 400..500i64 {
            slot += 1;
            q = (q as f64 * 0.97) as u128;
            evs.push(mk(slot, s, 20_000 + s as u32, false, 300_000_000, q, 1_000_000_000_000_000));
        }
        let mut ev = Events::new(evs, vec!["M".into()], (0, 120), vec![]);
        ev.cover_end = vec![100_000];
        ev
    }

    /// User decision: one 100% exit on crowd momentum. The runner is held through the pump (only buyers) and sold
    /// in one leg once sellers dominate for fade_persist_s.
    #[test]
    fn momentum_fade_holds_runner_then_sells_all() {
        let ev = runner_with_dump();
        let rp = RadarParams { min_buyers: 5, min_buy_sol: 1.0, max_median_buy_sol: 1.0, ..RadarParams::default() };
        let w = WindowData::new("t".into(), &ev, Context::default(), f64::NAN);
        let pr = Pred { bin: [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], ..Default::default() };
        let c = CostsV5 { buy_fail_prob: 0.0, ..Default::default() };
        let rk = [0.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0];
        for mode in [ExitMode::MomentumFade, ExitMode::FadeAndDrawdown, ExitMode::Trail, ExitMode::AdaptiveTrail] {
            let st = StratV5 { max_ret_60s_entry: 10.0, max_top10_share: 1.0, max_bundle_share: 1.0, exit_mode: mode, ..Default::default() };
            let r = simulate(&w, rp, &mut Fixed(pr), &st, &c, &rk, None, false);
            assert_eq!(r.trades, 1, "{mode:?} {:?}", r.blocked);
            let t = &r.trades_list[0];
            assert_eq!(t.legs.len(), 1, "{mode:?}: one 100% exit, no partial sells: {t:?}");
            assert!((t.legs[0].tokens_frac - 1.0).abs() < 1e-12);
            assert!(t.max_mult > 20.0 && t.pnl_sol > 0.0, "{mode:?}: held through the pump: {t:?}");
            assert!(t.exit_t >= 400, "{mode:?}: not sold during the pump: {t:?}");
            let want = match mode { ExitMode::Trail => "Trail", ExitMode::AdaptiveTrail => "AdaptiveTrail", _ => "MomentumFade" };
            assert_eq!(t.final_reason, want, "{t:?}");
        }
    }

    /// P5: no entry once the price is > max_entry_ret above its 5-minute low.
    #[test]
    fn price_guard_blocks_late_entries() {
        let mut evs = Vec::new();
        let tk: u128 = 1_000_000_000_000_000;
        let mut slot = 100;
        for s in 0..60i64 {
            for u in 0..3u32 {
                slot += 1;
                let q: u128 = if s == 0 && u == 0 { 50_000_000_000 } else { 70_000_000_000 }; // +40% at once
                evs.push(mk(slot, s, (s as u32) * 3 + u, true, 300_000_000, q, tk));
            }
        }
        let mut ev = Events::new(evs, vec!["M".into()], (0, 60), vec![]);
        ev.cover_end = vec![100_000];
        let rp = RadarParams { min_buyers: 5, min_buy_sol: 1.0, max_median_buy_sol: 1.0, ..RadarParams::default() };
        let w = WindowData::new("t".into(), &ev, Context::default(), f64::NAN);
        let pr = Pred { bin: [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], ..Default::default() };
        let st = StratV5 { max_ret_60s_entry: 10.0, max_top10_share: 1.0, max_bundle_share: 1.0, ..Default::default() };
        let r = simulate(&w, rp, &mut Fixed(pr), &st, &CostsV5 { buy_fail_prob: 0.0, ..Default::default() }, &[0.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0], None, false);
        assert_eq!(r.trades, 0, "{:?}", r.trades_list);
        assert!(r.blocked.get("price_guard").copied().unwrap_or(0) > 0, "{:?}", r.blocked);
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
        let r = simulate(&w, rp, &mut Fixed(pr), &StratV5::default(), &CostsV5::default(), &[0.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0], None, false);
        assert_eq!(r.trades, 0);
        assert!(r.blocked.get("ev_below_min").copied().unwrap_or(0) >= 1, "{:?}", r.blocked);
    }
}
