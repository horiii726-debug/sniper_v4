//! Stage 1 rule-based radar (V4 §7, §8, §11 subset). Causal: each trade updates only that coin's
//! state and the decision uses nothing after the trade. No ML, no trading.
//!
//! "Real" buy = buy of at least `real_min_sol` (dust buys inflate maker counts, §12.1).
//! Promotion to HOT when, over the last `win_s` seconds:
//!   unique real buyers >= min_buyers, real buy SOL >= min_buy_sol, net flow > 0,
//!   buy/sell SOL ratio <= max_buy_sell_ratio, median real buy <= max_median_buy_sol,
//!   and fast/slow EWMA of real-buy arrivals >= min_accel (or the coin is younger than slow tau).
//! Demotion after `demote_idle_s` without a real buy.

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
    pub buy: bool,
    pub sol: f64,
    pub price: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Params {
    pub win_s: i64,
    pub real_min_sol: f64,
    pub min_buyers: usize,
    pub min_buy_sol: f64,
    pub max_buy_sell_ratio: f64,
    pub max_median_buy_sol: f64,
    pub tau_fast_s: f64,
    pub tau_slow_s: f64,
    pub min_accel: f64,
    pub demote_idle_s: i64,
    /// Slow-climb rule (Radar 3): over `slow_win_s`, unique real buyers >= slow_min_buyers,
    /// real buy SOL >= slow_min_buy_sol and positive net flow.
    pub slow_win_s: i64,
    pub slow_min_buyers: usize,
    pub slow_min_buy_sol: f64,
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
            slow_min_buy_sol: f64::INFINITY,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    Promote,
    Demote,
}

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

struct Coin {
    first_time: i64,
    /// (time, trader, buy, sol) of real trades inside the window.
    win: VecDeque<(i64, u32, bool, f64)>,
    slow_win: VecDeque<(i64, u32, bool, f64)>,
    fast: Ewma,
    slow: Ewma,
    last_real_buy: i64,
    hot: bool,
    newborn: bool,
    promoted_once: bool,
}

pub struct Radar {
    p: Params,
    /// Coins first seen after this time are newborns (Radar 1): HOT from their first real buy.
    /// Live, newborns come from create/pool-create events; on a backfill window, first appearance after
    /// a warm-up is the proxy.
    newborn_after: i64,
    coins: FxHashMap<u32, Coin>,
    pub hot_now: usize,
    pub hot_max: usize,
}

impl Radar {
    pub fn new(p: Params) -> Self {
        Self { p, newborn_after: i64::MAX, coins: FxHashMap::default(), hot_now: 0, hot_max: 0 }
    }

    pub fn with_newborn_after(mut self, t: i64) -> Self {
        self.newborn_after = t;
        self
    }

    /// Demote every HOT coin idle for too long as of `now` (call before each trade, in time order).
    pub fn sweep(&mut self, now: i64, out: &mut Vec<(u32, Signal)>) {
        let idle = self.p.demote_idle_s;
        for (id, c) in self.coins.iter_mut() {
            if c.hot && now - c.last_real_buy > idle {
                c.hot = false;
                self.hot_now -= 1;
                out.push((*id, Signal::Demote));
            }
        }
    }

    pub fn on_trade(&mut self, t: &Trade) -> Option<Signal> {
        let p = self.p;
        let c = self.coins.entry(t.coin).or_insert_with(|| Coin {
            first_time: t.time,
            win: VecDeque::new(),
            slow_win: VecDeque::new(),
            fast: Ewma { r: 0.0, t: t.time as f64 },
            slow: Ewma { r: 0.0, t: t.time as f64 },
            last_real_buy: t.time,
            hot: false,
            newborn: t.time > self.newborn_after,
            promoted_once: false,
        });
        if t.sol < p.real_min_sol {
            return None;
        }
        c.win.push_back((t.time, t.trader, t.buy, t.sol));
        while c.win.front().is_some_and(|w| w.0 <= t.time - p.win_s) {
            c.win.pop_front();
        }
        c.slow_win.push_back((t.time, t.trader, t.buy, t.sol));
        while c.slow_win.front().is_some_and(|w| w.0 <= t.time - p.slow_win_s) {
            c.slow_win.pop_front();
        }
        if t.buy {
            c.fast.add(t.time as f64, 1.0, p.tau_fast_s);
            c.slow.add(t.time as f64, 1.0, p.tau_slow_s);
            c.last_real_buy = t.time;
        }
        if c.hot || !t.buy {
            return None;
        }
        // Radar 1: a newborn is HOT from its first real buy (re-promotion after demotion uses the rules).
        if c.newborn && !c.promoted_once {
            c.promoted_once = true;
            c.hot = true;
            self.hot_now += 1;
            self.hot_max = self.hot_max.max(self.hot_now);
            return Some(Signal::Promote);
        }
        if Self::slow_rule(&c.slow_win, &p) {
            c.hot = true;
            c.promoted_once = true;
            self.hot_now += 1;
            self.hot_max = self.hot_max.max(self.hot_now);
            return Some(Signal::Promote);
        }
        let mut buyers: Vec<u32> = Vec::with_capacity(c.win.len());
        let (mut bsol, mut ssol) = (0.0, 0.0);
        let mut bsizes: Vec<f64> = Vec::new();
        for &(_, tr, b, s) in &c.win {
            if b {
                buyers.push(tr);
                bsol += s;
                bsizes.push(s);
            } else {
                ssol += s;
            }
        }
        buyers.sort_unstable();
        buyers.dedup();
        if buyers.len() < p.min_buyers || bsol < p.min_buy_sol || bsol <= ssol {
            return None;
        }
        if ssol > 0.0 && bsol / ssol > p.max_buy_sell_ratio {
            return None;
        }
        bsizes.sort_by(|a, b| a.total_cmp(b));
        if bsizes[bsizes.len() / 2] > p.max_median_buy_sol {
            return None;
        }
        let now = t.time as f64;
        let young = ((t.time - c.first_time) as f64) < p.tau_slow_s;
        let accel = c.fast.at(now, p.tau_fast_s) / (c.slow.at(now, p.tau_slow_s) + 1e-9);
        if !young && accel < p.min_accel {
            return None;
        }
        c.hot = true;
        c.promoted_once = true;
        self.hot_now += 1;
        self.hot_max = self.hot_max.max(self.hot_now);
        Some(Signal::Promote)
    }

    fn slow_rule(w: &VecDeque<(i64, u32, bool, f64)>, p: &Params) -> bool {
        if p.slow_min_buyers == usize::MAX {
            return false;
        }
        let (mut b, mut s) = (0.0, 0.0);
        let mut buyers: Vec<u32> = Vec::new();
        for &(_, tr, buy, sol) in w {
            if buy {
                b += sol;
                buyers.push(tr);
            } else {
                s += sol;
            }
        }
        buyers.sort_unstable();
        buyers.dedup();
        buyers.len() >= p.slow_min_buyers && b >= p.slow_min_buy_sol && b > s
    }

    pub fn is_newborn(&self, coin: u32) -> Option<bool> {
        self.coins.get(&coin).map(|c| c.newborn)
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
mod tests {
    use super::*;

    #[test]
    fn ewma_constant_rate_converges_and_decays() {
        // V4 §33.4: constant rate r for >> tau -> R converges to r; no events -> exact exp decay.
        let mut e = Ewma::default();
        let tau = 10.0;
        for i in 1..=10_000 {
            e.add(i as f64 * 0.5, 1.0, tau); // 2 events / s
        }
        let r = e.at(5000.0, tau);
        assert!((r - 2.0).abs() / 2.0 < 0.03, "{r}");
        let later = e.at(5010.0, tau);
        assert!((later / r - (-1.0f64).exp()).abs() < 1e-12);
    }

    fn tr(time: i64, trader: u32, buy: bool, sol: f64) -> Trade {
        Trade { time, order: time as u64, coin: 1, trader, buy, sol, price: 1.0 }
    }

    #[test]
    fn promotes_on_broad_real_buying_and_ignores_dust() {
        let p = Params { min_buyers: 5, min_buy_sol: 0.5, ..Default::default() };
        let mut r = Radar::new(p);
        // 50 dust buys from 50 wallets: never HOT.
        for i in 0..50 {
            assert_eq!(r.on_trade(&tr(i, i as u32, true, 0.001)), None);
        }
        // 5 distinct real buyers of 0.2 SOL within 60 s -> HOT on the 5th.
        let mut sig = None;
        for i in 0..5 {
            sig = r.on_trade(&tr(60 + i, 100 + i as u32, true, 0.2));
        }
        assert_eq!(sig, Some(Signal::Promote));
        let mut out = Vec::new();
        r.sweep(60 + 4 + 301, &mut out);
        assert_eq!(out, vec![(1, Signal::Demote)]);
    }

    #[test]
    fn one_whale_does_not_promote() {
        let p = Params { min_buyers: 5, min_buy_sol: 0.5, ..Default::default() };
        let mut r = Radar::new(p);
        for i in 0..20 {
            assert_eq!(r.on_trade(&tr(i, 7, true, 5.0)), None);
        }
    }
}
