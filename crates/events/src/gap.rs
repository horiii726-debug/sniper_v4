//! Slot-gap tracking for a block-level feed (gRPC + RPC backfill).
//!
//! Invariant checked: every finalized (rooted) slot is reachable from the previous
//! finalized slot through BlockMeta parent links we actually received. A link whose
//! BlockMeta is missing is a gap; it is backfilled with `getBlock`, and the walk resumes.
//! Checks run `grace` slots behind the newest finalized slot so that stream replay after
//! a reconnect has time to deliver the data first. Slot-based only -> deterministic.

use rustc_hash::FxHashMap;
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GapEvent {
    /// Slot `slot` (needed on the finalized chain) has no BlockMeta: backfill requested.
    Detected { slot: u64 },
    /// Backfill delivered the block.
    Recovered { slot: u64 },
    /// Backfill could not deliver it (error / skipped although on the chain) or the chain is inconsistent.
    Unrecovered { slot: u64, reason: &'static str },
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct GapStats {
    pub finalized_checked: u64,
    pub detected: u64,
    pub recovered: u64,
    pub unrecovered: u64,
    pub first_checked: Option<u64>,
    pub last_checked: Option<u64>,
}

pub struct GapTracker {
    grace: u64,
    metas: FxHashMap<u64, u64>,
    queue: VecDeque<u64>,
    max_finalized: u64,
    last_ok: Option<u64>,
    first_meta: Option<u64>,
    waiting: Option<u64>,
    pub stats: GapStats,
}

impl GapTracker {
    pub fn new(grace: u64) -> Self {
        Self {
            grace,
            metas: FxHashMap::default(),
            queue: VecDeque::new(),
            max_finalized: 0,
            last_ok: None,
            first_meta: None,
            waiting: None,
            stats: GapStats::default(),
        }
    }

    pub fn on_meta(&mut self, slot: u64, parent: u64) {
        self.metas.insert(slot, parent);
        if self.first_meta.is_none() {
            self.first_meta = Some(slot);
        }
    }

    pub fn on_finalized(&mut self, slot: u64) {
        if slot > self.max_finalized {
            self.max_finalized = slot;
            self.queue.push_back(slot);
        }
    }

    /// Slot currently waiting for a backfill result, if any.
    pub fn waiting(&self) -> Option<u64> {
        self.waiting
    }

    /// Backfill result for `slot`: `Some(parent)` if the block exists.
    pub fn on_backfill(&mut self, slot: u64, parent: Option<u64>, out: &mut Vec<GapEvent>) {
        if self.waiting != Some(slot) {
            if let Some(p) = parent {
                self.metas.insert(slot, p);
            }
            return;
        }
        self.waiting = None;
        match parent {
            Some(p) => {
                self.metas.insert(slot, p);
                self.stats.recovered += 1;
                out.push(GapEvent::Recovered { slot });
            }
            None => {
                self.stats.unrecovered += 1;
                out.push(GapEvent::Unrecovered { slot, reason: "backfill_failed" });
                // Cannot verify this finalized slot's chain: accept it and move on.
                if let Some(s) = self.queue.pop_front() {
                    self.accept(s);
                }
            }
        }
    }

    fn accept(&mut self, s: u64) {
        self.last_ok = Some(s);
        self.stats.finalized_checked += 1;
        self.stats.first_checked.get_or_insert(s);
        self.stats.last_checked = Some(s);
        let floor = s.saturating_sub(5_000);
        if self.metas.len() > 50_000 {
            self.metas.retain(|k, _| *k >= floor);
        }
    }

    /// Run due checks. Returns slots that need a backfill via `backfill`.
    pub fn poll(&mut self, out: &mut Vec<GapEvent>, backfill: &mut Vec<u64>) {
        while self.waiting.is_none() {
            let Some(&s) = self.queue.front() else { break };
            if s + self.grace > self.max_finalized {
                break;
            }
            let Some(first) = self.first_meta else { break };
            if s < first {
                self.queue.pop_front();
                continue;
            }
            let Some(f) = self.last_ok else {
                if self.metas.contains_key(&s) {
                    self.queue.pop_front();
                    self.accept(s);
                } else {
                    self.request(s, out, backfill);
                }
                continue;
            };
            // Walk parent links from s down to f.
            let mut x = s;
            let mut missing = None;
            let mut mismatch = false;
            while x > f {
                match self.metas.get(&x) {
                    Some(&p) => x = p,
                    None => {
                        missing = Some(x);
                        break;
                    }
                }
            }
            if missing.is_none() && x != f {
                mismatch = true;
            }
            match missing {
                Some(m) => self.request(m, out, backfill),
                None => {
                    self.queue.pop_front();
                    if mismatch {
                        self.stats.unrecovered += 1;
                        out.push(GapEvent::Unrecovered { slot: s, reason: "chain_mismatch" });
                    }
                    self.accept(s);
                }
            }
        }
    }

    fn request(&mut self, slot: u64, out: &mut Vec<GapEvent>, backfill: &mut Vec<u64>) {
        self.waiting = Some(slot);
        self.stats.detected += 1;
        out.push(GapEvent::Detected { slot });
        backfill.push(slot);
    }

    /// Finalized slots received but not yet old enough to check (reported at shutdown, not counted as gaps).
    pub fn unchecked_tail(&self) -> usize {
        self.queue.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(t: &mut GapTracker) -> (Vec<GapEvent>, Vec<u64>) {
        let mut ev = Vec::new();
        let mut bf = Vec::new();
        t.poll(&mut ev, &mut bf);
        (ev, bf)
    }

    #[test]
    fn contiguous_chain_with_skipped_slot_has_no_gap() {
        let mut t = GapTracker::new(2);
        // 100 <- 101 <- 103 (102 skipped by the leader)
        for (s, p) in [(100, 99), (101, 100), (103, 101), (104, 103), (105, 104), (106, 105)] {
            t.on_meta(s, p);
        }
        for s in [100, 101, 103, 104, 105, 106] {
            t.on_finalized(s);
        }
        let (ev, bf) = run(&mut t);
        assert!(ev.is_empty() && bf.is_empty(), "{ev:?}");
        assert_eq!(t.stats.finalized_checked, 4); // 100,101,103,104 (105,106 within grace)
    }

    #[test]
    fn missing_meta_is_detected_and_recovered_by_backfill() {
        let mut t = GapTracker::new(1);
        for (s, p) in [(100, 99), (102, 101), (103, 102)] {
            t.on_meta(s, p); // 101 missing
        }
        for s in [100, 101, 102, 103] {
            t.on_finalized(s);
        }
        let (ev, bf) = run(&mut t);
        assert_eq!(bf, vec![101]);
        assert_eq!(ev, vec![GapEvent::Detected { slot: 101 }]);
        let mut ev = Vec::new();
        t.on_backfill(101, Some(100), &mut ev);
        assert_eq!(ev, vec![GapEvent::Recovered { slot: 101 }]);
        let (ev, bf) = run(&mut t);
        assert!(ev.is_empty() && bf.is_empty());
        assert_eq!(t.stats.detected, 1);
        assert_eq!(t.stats.recovered, 1);
        assert_eq!(t.stats.unrecovered, 0);
    }

    #[test]
    fn missed_finalized_status_is_covered_by_parent_walk() {
        let mut t = GapTracker::new(0);
        for (s, p) in [(100, 99), (101, 100), (102, 101), (103, 102)] {
            t.on_meta(s, p);
        }
        t.on_finalized(100);
        t.on_finalized(103); // finalized statuses for 101/102 never seen
        let (ev, _) = run(&mut t);
        assert!(ev.is_empty());
        assert_eq!(t.stats.last_checked, Some(103));
    }

    #[test]
    fn failed_backfill_is_unrecovered() {
        let mut t = GapTracker::new(0);
        t.on_meta(100, 99);
        t.on_meta(102, 101);
        t.on_finalized(100);
        t.on_finalized(102);
        let (_, bf) = run(&mut t);
        assert_eq!(bf, vec![101]);
        let mut ev = Vec::new();
        t.on_backfill(101, None, &mut ev);
        assert_eq!(t.stats.unrecovered, 1);
        assert_eq!(t.stats.last_checked, Some(102));
    }
}
