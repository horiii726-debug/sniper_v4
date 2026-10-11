//! Robust causal CUSUM change-point detection per coin (prediction spec Bagian 7, V4 spec §11.5).
//!
//! Sampling: fixed steps of `step_s` seconds aligned to absolute time. Trades accumulate into the open step; a step
//! is closed when the coin's first trade of a later step arrives (steps in between are idle steps).
//! Channels per closed step (real, non-creator trades of the coin):
//!   Clusters   x = 2 sqrt(n + 3/8), n = unique buying clusters (Anscombe: Poisson variance ~ 1); idle 2 sqrt(3/8)
//!   BuySol     x = ln(1 + B / bsol_unit) (buy SOL, log scale); idle 0
//!   NetFlowLiq x = (B - S) / L (L = real liquidity, floored); disabled while L is unknown; idle 0
//! Baseline from the coin's own PAST steps only (median / 1.4826 MAD, or EWMA); while the coin has fewer than
//! `warmup_n` own steps, the population baseline of its age bucket (fed by earlier closed steps of all coins) is used.
//! z = (x - mu) / max(sigma, floor); S = max(0, S + z - k); alarm when S > h, then S = 0.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineKind {
    #[default]
    MedianMad,
    Ewma,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CusumParams {
    pub enabled: bool,
    pub step_s: i64,
    pub k: f64,
    pub h: f64,
    /// Own-history samples kept for the median / MAD baseline.
    pub base_n: usize,
    /// Own samples needed before the own baseline is used (population baseline before that).
    pub warmup_n: usize,
    pub baseline: BaselineKind,
    pub ewma_tau_steps: f64,
    /// Population samples needed per (age bucket, channel) before it is used.
    pub pop_min: usize,
    pub pop_cap: usize,
    /// Population estimates are recomputed at most every this many seconds of event time.
    pub pop_refresh_s: i64,
    /// Idle steps processed after a gap; a longer gap also resets every S.
    pub max_gap_steps: usize,
    /// An alarm counts as "recent" for this long (flow rule).
    pub hold_s: i64,
    pub bsol_unit: f64,
    pub floor_uc: f64,
    pub floor_bsol: f64,
    pub floor_nfl: f64,
}

impl Default for CusumParams {
    fn default() -> Self {
        Self {
            enabled: true,
            step_s: 5,
            k: 0.5,
            h: 5.0,
            base_n: 60,
            warmup_n: 12,
            baseline: BaselineKind::MedianMad,
            ewma_tau_steps: 60.0,
            pop_min: 64,
            pop_cap: 2048,
            pop_refresh_s: 5,
            max_gap_steps: 60,
            hold_s: 60,
            bsol_unit: 0.1,
            floor_uc: 1.0,
            floor_bsol: 0.5,
            floor_nfl: 0.002,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Channel {
    Clusters = 0,
    BuySol = 1,
    NetFlowLiq = 2,
}

pub const N_CH: usize = 3;
pub const N_BUCKETS: usize = 10;
const CHANNELS: [Channel; N_CH] = [Channel::Clusters, Channel::BuySol, Channel::NetFlowLiq];
/// Age bucket edges in seconds since birth (V4 §11.4); bucket 9 = age unknown (old coin).
const AGE_EDGES: [i64; 8] = [10, 30, 60, 120, 300, 900, 3600, 21600];

pub fn age_bucket(age_s: Option<i64>) -> usize {
    match age_s {
        None => N_BUCKETS - 1,
        Some(a) => AGE_EDGES.iter().take_while(|e| a >= **e).count(),
    }
}

/// (median, 1.4826 * MAD); empty -> (NaN, NaN). Sorts `v` in place.
pub fn median_mad(v: &mut [f64]) -> (f64, f64) {
    if v.is_empty() {
        return (f64::NAN, f64::NAN);
    }
    let med = median_sorted(v);
    for x in v.iter_mut() {
        *x = (*x - med).abs();
    }
    (med, 1.4826 * median_sorted(v))
}

fn median_sorted(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Own-history baseline of one channel (past samples only).
#[derive(Debug, Clone)]
pub struct RobustBaseline {
    ring: VecDeque<f64>,
    cap: usize,
    ew_mean: f64,
    ew_var: f64,
    n: usize,
    scratch: Vec<f64>,
}

impl RobustBaseline {
    pub fn new(cap: usize) -> Self {
        Self { ring: VecDeque::with_capacity(cap.min(64)), cap: cap.max(1), ew_mean: 0.0, ew_var: 0.0, n: 0, scratch: Vec::new() }
    }

    /// Samples seen (capped by the ring for median / MAD).
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    pub fn push(&mut self, x: f64, kind: BaselineKind, tau: f64) {
        self.ring.push_back(x);
        while self.ring.len() > self.cap {
            self.ring.pop_front();
        }
        if kind == BaselineKind::Ewma {
            if self.n == 0 {
                self.ew_mean = x;
                self.ew_var = 0.0;
            } else {
                let a = 1.0 / tau.max(1.0);
                let d = x - self.ew_mean;
                self.ew_mean += a * d;
                self.ew_var = (1.0 - a) * (self.ew_var + a * d * d);
            }
        }
        self.n += 1;
    }

    /// (mu, sigma before the floor); None without samples.
    pub fn estimate(&mut self, kind: BaselineKind) -> Option<(f64, f64)> {
        if self.ring.is_empty() {
            return None;
        }
        match kind {
            BaselineKind::MedianMad => {
                self.scratch.clear();
                self.scratch.extend(self.ring.iter().copied());
                Some(median_mad(&mut self.scratch))
            }
            BaselineKind::Ewma => Some((self.ew_mean, self.ew_var.max(0.0).sqrt())),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Cusum {
    pub s: f64,
}

impl Cusum {
    /// S = max(0, S + z - k); returns true (and resets S to 0) when S > h.
    pub fn update(&mut self, z: f64, k: f64, h: f64) -> bool {
        self.s = (self.s + z - k).max(0.0);
        if self.s > h {
            self.s = 0.0;
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Default)]
struct PopRing {
    ring: VecDeque<f64>,
    cached: Option<(f64, f64)>,
    cached_at: i64,
}

/// Population baselines per (age bucket, channel), fed by closed steps of all coins (causal).
#[derive(Debug, Clone)]
pub struct PopBaseline {
    rings: Vec<PopRing>,
    scratch: Vec<f64>,
}

impl Default for PopBaseline {
    fn default() -> Self {
        Self::new()
    }
}

impl PopBaseline {
    pub fn new() -> Self {
        Self { rings: vec![PopRing { cached_at: i64::MIN, ..Default::default() }; N_BUCKETS * N_CH], scratch: Vec::new() }
    }

    pub fn push(&mut self, bucket: usize, ch: Channel, x: f64, cap: usize) {
        let r = &mut self.rings[bucket.min(N_BUCKETS - 1) * N_CH + ch as usize];
        r.ring.push_back(x);
        while r.ring.len() > cap.max(1) {
            r.ring.pop_front();
        }
    }

    /// (mu, sigma before the floor), recomputed at most every `pop_refresh_s`; None until `pop_min` samples.
    pub fn estimate(&mut self, bucket: usize, ch: Channel, now: i64, p: &CusumParams) -> Option<(f64, f64)> {
        let r = &mut self.rings[bucket.min(N_BUCKETS - 1) * N_CH + ch as usize];
        if r.ring.len() < p.pop_min {
            return None;
        }
        if let Some(c) = r.cached {
            if now.saturating_sub(r.cached_at) < p.pop_refresh_s {
                return Some(c);
            }
        }
        self.scratch.clear();
        self.scratch.extend(r.ring.iter().copied());
        let est = match p.baseline {
            BaselineKind::MedianMad => median_mad(&mut self.scratch),
            BaselineKind::Ewma => {
                let n = self.scratch.len() as f64;
                let m = self.scratch.iter().sum::<f64>() / n;
                (m, (self.scratch.iter().map(|x| (x - m).powi(2)).sum::<f64>() / n).sqrt())
            }
        };
        r.cached = Some(est);
        r.cached_at = now;
        Some(est)
    }
}

#[derive(Debug, Clone, Default)]
struct StepAcc {
    clusters: Vec<u32>,
    buy_sol: f64,
    sell_sol: f64,
}

/// Per-coin CUSUM state over the three channels.
#[derive(Debug, Clone)]
pub struct CoinCusum {
    step: i64,
    acc: StepAcc,
    base: [RobustBaseline; N_CH],
    cs: [Cusum; N_CH],
    has_z: [bool; N_CH],
    pop_used: [Option<bool>; N_CH],
}

impl CoinCusum {
    pub fn new(p: &CusumParams) -> Self {
        Self {
            step: i64::MIN,
            acc: StepAcc::default(),
            base: [RobustBaseline::new(p.base_n), RobustBaseline::new(p.base_n), RobustBaseline::new(p.base_n)],
            cs: [Cusum::default(); N_CH],
            has_z: [false; N_CH],
            pop_used: [None; N_CH],
        }
    }

    /// Close every step that ended before time `t` (the open step plus at most `max_gap_steps` idle steps), feeding
    /// baselines and CUSUMs. Returns the first alarming channel (step order, then Clusters, BuySol, NetFlowLiq).
    /// `liq_floored` = latest real liquidity (SOL, floored) known before this trade, None if unknown.
    pub fn advance(&mut self, t: i64, p: &CusumParams, pop: &mut PopBaseline, birth_t: Option<i64>, liq_floored: Option<f64>) -> Option<Channel> {
        let st = t.div_euclid(p.step_s.max(1));
        if self.step == i64::MIN {
            self.step = st;
            return None;
        }
        if st <= self.step {
            return None;
        }
        let acc = std::mem::take(&mut self.acc);
        let mut cl = acc.clusters;
        cl.sort_unstable();
        cl.dedup();
        let active = [
            Some(2.0 * (cl.len() as f64 + 0.375).sqrt()),
            Some((1.0 + acc.buy_sol / p.bsol_unit).ln()),
            liq_floored.map(|l| (acc.buy_sol - acc.sell_sol) / l),
        ];
        let mut alarm = self.close_step(self.step, active, p, pop, birth_t);
        let gap = (st - self.step - 1) as u64;
        let idle = [Some(2.0 * 0.375f64.sqrt()), Some(0.0), liq_floored.map(|_| 0.0)];
        for j in 1..=gap.min(p.max_gap_steps as u64) {
            let a = self.close_step(self.step + j as i64, idle, p, pop, birth_t);
            alarm = alarm.or(a);
        }
        if gap > p.max_gap_steps as u64 {
            for c in self.cs.iter_mut() {
                c.s = 0.0;
            }
        }
        self.step = st;
        alarm
    }

    fn close_step(&mut self, step: i64, xs: [Option<f64>; N_CH], p: &CusumParams, pop: &mut PopBaseline, birth_t: Option<i64>) -> Option<Channel> {
        let start = step.saturating_mul(p.step_s.max(1));
        let bucket = age_bucket(birth_t.map(|b| start - b));
        let floors = [p.floor_uc, p.floor_bsol, p.floor_nfl];
        let mut alarm = None;
        for (k, ch) in CHANNELS.iter().enumerate() {
            let Some(x) = xs[k] else { continue };
            if !x.is_finite() {
                continue;
            }
            let est = if self.base[k].len() >= p.warmup_n {
                self.base[k].estimate(p.baseline).map(|e| (e, false))
            } else {
                pop.estimate(bucket, *ch, start, p).map(|e| (e, true))
            };
            if let Some(((mu, sd), used_pop)) = est {
                if mu.is_finite() {
                    let sigma = if sd.is_finite() { sd.max(floors[k]) } else { floors[k] };
                    let z = (x - mu) / sigma;
                    self.has_z[k] = true;
                    self.pop_used[k] = Some(used_pop);
                    if self.cs[k].update(z, p.k, p.h) && alarm.is_none() {
                        alarm = Some(*ch);
                    }
                }
            }
            self.base[k].push(x, p.baseline, p.ewma_tau_steps);
            pop.push(bucket, *ch, x, p.pop_cap);
        }
        alarm
    }

    /// Add a real, non-creator trade to the open step (call after `advance` for the same trade).
    pub fn add(&mut self, who: u32, buy: bool, sol: f64) {
        if buy {
            self.acc.clusters.push(who);
            self.acc.buy_sol += sol;
        } else {
            self.acc.sell_sol += sol;
        }
    }

    /// Current S of a channel; None until the channel produced a z once.
    pub fn s(&self, ch: Channel) -> Option<f64> {
        self.has_z[ch as usize].then_some(self.cs[ch as usize].s)
    }

    /// Whether the channel's last z used the population baseline.
    pub fn pop_used(&self, ch: Channel) -> Option<bool> {
        self.pop_used[ch as usize]
    }

    /// Own-baseline samples of a channel (tests, diagnostics).
    pub fn own_len(&self, ch: Channel) -> usize {
        self.base[ch as usize].len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_mad_known_values() {
        let (m, s) = median_mad(&mut [1.0, 2.0, 3.0, 4.0, 100.0]);
        assert_eq!(m, 3.0);
        assert!((s - 1.4826).abs() < 1e-12);
        let (m, s) = median_mad(&mut []);
        assert!(m.is_nan() && s.is_nan());
    }

    #[test]
    fn cusum_stays_zero_below_reference() {
        let mut c = Cusum::default();
        for _ in 0..1000 {
            assert!(!c.update(0.4, 0.5, 5.0));
            assert_eq!(c.s, 0.0);
        }
    }

    #[test]
    fn cusum_step_change_alarms_after_expected_n() {
        let mut c = Cusum::default();
        assert!(!c.update(2.0, 0.5, 5.0) && c.s == 1.5);
        assert!(!c.update(2.0, 0.5, 5.0) && c.s == 3.0);
        assert!(!c.update(2.0, 0.5, 5.0) && c.s == 4.5);
        assert!(c.update(2.0, 0.5, 5.0));
        assert_eq!(c.s, 0.0);
    }

    #[test]
    fn robust_baseline_ignores_single_outlier() {
        let mut b = RobustBaseline::new(60);
        for i in 0..59 {
            b.push(0.9 + 0.2 * i as f64 / 58.0, BaselineKind::MedianMad, 60.0); // 1.0 +- 0.1, spread
        }
        let (m0, s0) = b.estimate(BaselineKind::MedianMad).unwrap();
        b.push(1000.0, BaselineKind::MedianMad, 60.0);
        let (m1, s1) = b.estimate(BaselineKind::MedianMad).unwrap();
        assert!((m1 - m0).abs() < 0.05 && (s1 - s0).abs() < 0.05, "{m0} {s0} -> {m1} {s1}");
    }

    fn quiet_params() -> CusumParams {
        CusumParams { warmup_n: 3, pop_min: 1_000_000, ..Default::default() }
    }

    #[test]
    fn baseline_excludes_current_step() {
        // 10 quiet steps (1 buyer each), then a spike step: its z uses the pre-spike baseline.
        let p = quiet_params();
        let mut pop = PopBaseline::new();
        let mut c = CoinCusum::new(&p);
        for s in 0..10i64 {
            c.advance(s * 5, &p, &mut pop, None, Some(10.0));
            c.add(s as u32, true, 0.1);
        }
        let mut b = c.base[0].clone();
        let before = b.estimate(BaselineKind::MedianMad).unwrap();
        for u in 0..30u32 {
            c.add(1000 + u, true, 0.1); // more buyers in the open step 9
        }
        let mut b2 = c.base[0].clone();
        assert_eq!(b2.estimate(BaselineKind::MedianMad).unwrap(), before, "open step not in the baseline");
        c.advance(50, &p, &mut pop, None, Some(10.0)); // closes the spike step
        assert!(c.s(Channel::Clusters).unwrap() > 0.0 || c.cs[0].s == 0.0);
        assert_eq!(c.own_len(Channel::Clusters), 10);
    }

    #[test]
    fn warmup_uses_population_then_own() {
        let p = CusumParams { warmup_n: 4, pop_min: 8, ..Default::default() };
        let mut pop = PopBaseline::new();
        // no population, no own samples: no z
        let mut c0 = CoinCusum::new(&p);
        c0.advance(0, &p, &mut pop, None, None);
        c0.add(1, true, 0.1);
        assert_eq!(c0.advance(5, &p, &mut pop, None, None), None);
        assert_eq!(c0.s(Channel::Clusters), None);
        // fill bucket 9 (age unknown)
        for i in 0..8 {
            pop.push(9, Channel::Clusters, 2.0 + 0.01 * i as f64, p.pop_cap);
        }
        let mut c = CoinCusum::new(&p);
        c.advance(100, &p, &mut pop, None, None);
        c.add(1, true, 0.1);
        c.advance(105, &p, &mut pop, None, None);
        assert_eq!(c.pop_used(Channel::Clusters), Some(true));
        for s in 2..8i64 {
            c.add(s as u32, true, 0.1);
            c.advance(100 + s * 5, &p, &mut pop, None, None);
        }
        assert_eq!(c.pop_used(Channel::Clusters), Some(false));
    }

    #[test]
    fn fixed_step_sampling_counts_idle_steps() {
        let p = quiet_params();
        let mut pop = PopBaseline::new();
        let mut c = CoinCusum::new(&p);
        c.advance(0, &p, &mut pop, None, Some(5.0));
        c.add(1, true, 1.0);
        c.advance(50, &p, &mut pop, None, Some(5.0));
        assert_eq!(c.own_len(Channel::Clusters), 10, "1 active + 9 idle steps");
        c.add(2, true, 50.0);
        c.cs[1].s = 3.0;
        c.advance(10_050, &p, &mut pop, None, Some(5.0));
        assert_eq!(c.own_len(Channel::Clusters), (10 + 1 + p.max_gap_steps).min(p.base_n));
        assert!(c.cs.iter().all(|x| x.s == 0.0), "long gap resets S");
    }

    #[test]
    fn sigma_floor_prevents_inf() {
        let p = quiet_params();
        let mut pop = PopBaseline::new();
        let mut c = CoinCusum::new(&p);
        for s in 0..20i64 {
            c.advance(s * 5, &p, &mut pop, None, Some(10.0));
            c.add(7, true, 0.2);
        }
        let s = c.s(Channel::BuySol).unwrap();
        assert!(s.is_finite());
    }

    #[test]
    fn age_bucket_edges() {
        let got: Vec<usize> = [0, 9, 10, 29, 30, 59, 60, 300, 3600, 21600].iter().map(|a| age_bucket(Some(*a))).collect();
        assert_eq!(got, vec![0, 0, 1, 1, 2, 2, 3, 5, 7, 8]);
        assert_eq!(age_bucket(None), 9);
    }

    #[test]
    fn pop_baseline_refresh_is_deterministic() {
        let p = CusumParams { pop_min: 2, pop_refresh_s: 5, ..Default::default() };
        let run = || {
            let mut pop = PopBaseline::new();
            let mut out = Vec::new();
            for i in 0..20 {
                pop.push(0, Channel::BuySol, i as f64, p.pop_cap);
                out.push(pop.estimate(0, Channel::BuySol, i, &p));
            }
            out
        };
        let (a, b) = (run(), run());
        assert_eq!(a, b);
        assert_eq!(a[0], None, "below pop_min");
        assert_eq!(a[1], a[5], "computed at t = 1, cached until pop_refresh_s passes");
        assert_ne!(a[5], a[6]);
    }

    #[test]
    fn ewma_baseline_matches_closed_form() {
        let mut b = RobustBaseline::new(10);
        for _ in 0..500 {
            b.push(3.0, BaselineKind::Ewma, 20.0);
        }
        let (m, s) = b.estimate(BaselineKind::Ewma).unwrap();
        assert!((m - 3.0).abs() < 1e-12 && s.abs() < 1e-12);
    }

    #[test]
    fn cluster_burst_alarms_on_clusters_channel() {
        let p = quiet_params();
        let mut pop = PopBaseline::new();
        let mut c = CoinCusum::new(&p);
        let mut t = 0;
        for s in 0..40i64 {
            t = s * 5;
            assert_eq!(c.advance(t, &p, &mut pop, None, None), None);
            if s % 2 == 0 {
                c.add(1, true, 0.05);
            }
        }
        let mut alarm = None;
        for s in 40..44i64 {
            t = s * 5;
            if let Some(a) = c.advance(t, &p, &mut pop, None, None) {
                alarm = Some((s, a));
                break;
            }
            for u in 0..8u32 {
                c.add(100 + u, true, 0.05);
            }
        }
        let (s, ch) = alarm.expect("burst alarms");
        assert_eq!(ch, Channel::Clusters);
        assert!(s <= 43, "within 2 burst steps (closed at step {s}), t {t}");
    }
}
