//! Monotonic/wall ns clocks and latency histograms (V4 §24).

use hdrhistogram::Histogram;
use serde::Serialize;

fn clock_ns(id: libc::clockid_t) -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: valid pointer to a stack timespec; clock ids are constants supported on Linux.
    unsafe { libc::clock_gettime(id, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// CLOCK_MONOTONIC in ns: for intervals inside this process.
#[inline]
pub fn mono_ns() -> u64 {
    clock_ns(libc::CLOCK_MONOTONIC)
}

/// CLOCK_REALTIME in ns since the Unix epoch: for comparing against chain/server timestamps.
#[inline]
pub fn wall_ns() -> u64 {
    clock_ns(libc::CLOCK_REALTIME)
}

/// Latency histogram in ns (1 ns .. 1 h, 3 significant digits). Negative samples are counted separately.
pub struct LatencyHist {
    h: Histogram<u64>,
    negative: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub count: u64,
    pub negative: u64,
    pub p50_ms: f64,
    pub p75_ms: f64,
    pub p90_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

impl Default for LatencyHist {
    fn default() -> Self {
        Self::new()
    }
}

impl LatencyHist {
    pub fn new() -> Self {
        Self { h: Histogram::new_with_bounds(1, 3_600_000_000_000, 3).expect("valid bounds"), negative: 0 }
    }

    /// Record `later - earlier` (both ns). A negative difference (clock skew) is counted, not clamped into the data.
    pub fn record_diff(&mut self, later_ns: u64, earlier_ns: u64) {
        if later_ns >= earlier_ns {
            let _ = self.h.record((later_ns - earlier_ns).max(1));
        } else {
            self.negative += 1;
        }
    }

    pub fn summary(&self) -> Summary {
        let q = |p: f64| self.h.value_at_quantile(p) as f64 / 1e6;
        Summary {
            count: self.h.len(),
            negative: self.negative,
            p50_ms: q(0.50),
            p75_ms: q(0.75),
            p90_ms: q(0.90),
            p95_ms: q(0.95),
            p99_ms: q(0.99),
            max_ms: self.h.max() as f64 / 1e6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks_advance() {
        let a = mono_ns();
        let b = mono_ns();
        assert!(b >= a);
        assert!(wall_ns() > 1_700_000_000_000_000_000);
    }

    #[test]
    fn hist_quantiles_and_negative() {
        let mut h = LatencyHist::new();
        for i in 1..=100u64 {
            h.record_diff(i * 1_000_000, 0);
        }
        h.record_diff(0, 5);
        let s = h.summary();
        assert_eq!(s.count, 100);
        assert_eq!(s.negative, 1);
        assert!((s.p50_ms - 50.0).abs() < 0.1);
        assert!((s.p99_ms - 99.0).abs() < 0.1);
    }
}
