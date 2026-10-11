//! Frozen copy of the V4 radar (commit e75a8c5 / b70c27c), test-only: reference for the legacy-mode equivalence test.
#![allow(dead_code)]

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
    /// Price-lift rule (Radar 3, thin coins): over `slow_win_s`, last real-trade price >= lift x the
    /// window's minimum price, with >= lift_min_buyers distinct real buyers since that minimum and
    /// positive net flow since it. 0 disables.
    pub lift: f64,
    pub lift_min_buyers: usize,
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
            lift: 0.0,
            lift_min_buyers: 2,
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
    /// (time, price) of real trades inside the slow window, parallel to `slow_win`.
    prices: VecDeque<f64>,
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
            prices: VecDeque::new(),
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
        c.prices.push_back(t.price);
        while c.slow_win.front().is_some_and(|w| w.0 <= t.time - p.slow_win_s) {
            c.slow_win.pop_front();
            c.prices.pop_front();
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
        if Self::slow_rule(&c.slow_win, &p) || Self::lift_rule(&c.slow_win, &c.prices, &p) {
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

    fn lift_rule(w: &VecDeque<(i64, u32, bool, f64)>, prices: &VecDeque<f64>, p: &Params) -> bool {
        if p.lift <= 0.0 || prices.len() < 2 {
            return false;
        }
        let last = *prices.back().expect("non-empty");
        let (imin, min) = prices.iter().enumerate().fold((0, f64::INFINITY), |a, (i, &x)| if x.is_finite() && x < a.1 { (i, x) } else { a });
        if !(min > 0.0) || last < p.lift * min {
            return false;
        }
        let (mut b, mut s) = (0.0, 0.0);
        let mut buyers: Vec<u32> = Vec::new();
        for &(_, tr, buy, sol) in w.iter().skip(imin + 1) {
            if buy {
                b += sol;
                buyers.push(tr);
            } else {
                s += sol;
            }
        }
        buyers.sort_unstable();
        buyers.dedup();
        buyers.len() >= p.lift_min_buyers && b > s
    }

    pub fn is_newborn(&self, coin: u32) -> Option<bool> {
        self.coins.get(&coin).map(|c| c.newborn)
    }
}

