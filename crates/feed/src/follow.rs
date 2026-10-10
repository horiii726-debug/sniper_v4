//! Per-address history fetch for the V5 two-tier backfill (`sniper follow`): page
//! `getSignaturesForAddress` newest -> oldest, keep the signatures inside a slot/time range, and fetch
//! single transactions with `getTransaction`. Every call goes through one shared rate limiter and is
//! counted per method (credit accounting, V5 plan §2.5).

use crate::rpc::Rpc;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

/// Global request pacing: at most one request start every `gap` across all tasks.
#[derive(Clone)]
pub struct Limiter {
    next: Arc<Mutex<Instant>>,
    gap: Duration,
}

impl Limiter {
    pub fn new(rps: f64) -> Self {
        Self { next: Arc::new(Mutex::new(Instant::now())), gap: Duration::from_secs_f64(1.0 / rps.max(0.01)) }
    }

    pub async fn wait(&self) {
        let at = {
            let mut n = self.next.lock().await;
            let at = (*n).max(Instant::now());
            *n = at + self.gap;
            at
        };
        tokio::time::sleep_until(at).await;
    }
}

#[derive(Default, Debug)]
pub struct FollowCounters {
    pub sig_calls: AtomicU64,
    pub sig_retries: AtomicU64,
    pub tx_calls: AtomicU64,
    pub tx_retries: AtomicU64,
    pub tx_not_found: AtomicU64,
    pub bytes: AtomicU64,
}

impl FollowCounters {
    pub fn to_json(&self) -> Value {
        json!({
            "getSignaturesForAddress": self.sig_calls.load(Relaxed), "getSignaturesForAddress_retries": self.sig_retries.load(Relaxed),
            "getTransaction": self.tx_calls.load(Relaxed), "getTransaction_retries": self.tx_retries.load(Relaxed),
            "getTransaction_not_found": self.tx_not_found.load(Relaxed), "bytes_decoded": self.bytes.load(Relaxed),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SigEntry {
    pub sig: String,
    pub slot: u64,
    pub time: i64,
    pub failed: bool,
}

/// Result of paging one address.
#[derive(Debug, Default)]
pub struct Scan {
    /// Successful transactions with `slot > after_slot` and `time <= until_time`, oldest first.
    pub in_range: Vec<SigEntry>,
    /// True when every signature newer than `after_slot` was seen (the range is complete).
    pub range_complete: bool,
    /// Oldest signature seen; it is the address's first transaction when `exhausted`.
    pub oldest: Option<SigEntry>,
    /// The address history was paged to its very first signature.
    pub exhausted: bool,
    pub pages: u32,
}

/// Paging state machine, separated from the network so it can be tested.
pub struct ScanState {
    after_slot: u64,
    until_time: i64,
    want_history: bool,
    rev: Vec<SigEntry>,
    pub scan: Scan,
}

impl ScanState {
    pub fn new(after_slot: u64, until_time: i64, want_history: bool) -> Self {
        Self { after_slot, until_time, want_history, rev: Vec::new(), scan: Scan::default() }
    }

    /// Feed one page (newest first). Returns true when paging should continue.
    pub fn page(&mut self, entries: &[Value], page_limit: usize) -> bool {
        self.scan.pages += 1;
        for e in entries {
            let (Some(sig), Some(slot)) = (e["signature"].as_str(), e["slot"].as_u64()) else { continue };
            let s = SigEntry { sig: sig.to_string(), slot, time: e["blockTime"].as_i64().unwrap_or(0), failed: !e["err"].is_null() };
            if slot <= self.after_slot {
                self.scan.range_complete = true;
            } else if s.time <= self.until_time && !s.failed {
                self.rev.push(s.clone());
            }
            self.scan.oldest = Some(s);
        }
        if entries.len() < page_limit {
            self.scan.exhausted = true;
            self.scan.range_complete = true;
            return false;
        }
        !(self.scan.range_complete && !self.want_history)
    }

    pub fn finish(mut self) -> Scan {
        self.rev.reverse();
        self.scan.in_range = self.rev;
        self.scan
    }
}

const SIG_PAGE: usize = 1000;
const TRANSIENT: [i64; 6] = [-32004, -32014, -32016, -32429, -32005, 429];

/// Page `address` from now back to `after_slot` (and on to its first signature when `want_history`, bounded
/// by `max_history_pages` extra pages past the range). `max_range_pages` bounds the range part.
#[allow(clippy::too_many_arguments)]
pub async fn scan_address(
    rpc: &Rpc,
    lim: &Limiter,
    c: &FollowCounters,
    address: &str,
    after_slot: u64,
    until_time: i64,
    want_history: bool,
    max_range_pages: u32,
    max_history_pages: u32,
) -> Result<Scan> {
    let mut st = ScanState::new(after_slot, until_time, want_history);
    let mut before: Option<String> = None;
    let mut history_pages = 0u32;
    loop {
        let mut cfg = json!({ "limit": SIG_PAGE, "commitment": "finalized" });
        if let Some(b) = &before {
            cfg["before"] = json!(b);
        }
        let v = call_retry(rpc, lim, "getSignaturesForAddress", json!([address, cfg]), &c.sig_calls, &c.sig_retries, &c.bytes).await?;
        let page = v["result"].as_array().cloned().ok_or_else(|| anyhow!("getSignaturesForAddress: no result"))?;
        let was_complete = st.scan.range_complete;
        let more = st.page(&page, SIG_PAGE);
        if was_complete {
            history_pages += 1;
        }
        before = page.last().and_then(|e| e["signature"].as_str().map(String::from));
        if !more || before.is_none() {
            break;
        }
        if !st.scan.range_complete && st.scan.pages >= max_range_pages {
            break;
        }
        if st.scan.range_complete && history_pages >= max_history_pages {
            break;
        }
    }
    Ok(st.finish())
}

/// `getTransaction` (json encoding, same shape as a `getBlock` transaction entry plus slot / blockTime).
/// Ok(None) = not found after retries.
pub async fn get_transaction(rpc: &Rpc, lim: &Limiter, c: &FollowCounters, sig: &str) -> Result<Option<Value>> {
    let params = json!([sig, { "encoding": "json", "maxSupportedTransactionVersion": 1, "commitment": "finalized" }]);
    for attempt in 0..4u32 {
        let v = call_retry(rpc, lim, "getTransaction", params.clone(), &c.tx_calls, &c.tx_retries, &c.bytes).await?;
        if !v["result"].is_null() {
            return Ok(Some(v["result"].clone()));
        }
        c.tx_not_found.fetch_add(1, Relaxed);
        tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
    }
    Ok(None)
}

/// One JSON-RPC call with retries on transport errors and transient error codes.
async fn call_retry(rpc: &Rpc, lim: &Limiter, method: &str, params: Value, calls: &AtomicU64, retries: &AtomicU64, bytes: &AtomicU64) -> Result<Value> {
    let mut last = String::new();
    for attempt in 0..10u32 {
        if attempt > 0 {
            retries.fetch_add(1, Relaxed);
            tokio::time::sleep(Duration::from_millis(250 * 2u64.pow(attempt.min(6)))).await;
        }
        lim.wait().await;
        calls.fetch_add(1, Relaxed);
        match rpc.call_bytes(method, params.clone()).await {
            Ok((v, n)) => {
                bytes.fetch_add(n, Relaxed);
                match v.get("error") {
                    None => return Ok(v),
                    Some(e) => {
                        let code = e["code"].as_i64().unwrap_or(0);
                        last = format!("{method}: {e}");
                        if !TRANSIENT.contains(&code) {
                            return Err(anyhow!(last));
                        }
                    }
                }
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(anyhow!("{last} (gave up after retries)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(sig: &str, slot: u64, time: i64, failed: bool) -> Value {
        json!({ "signature": sig, "slot": slot, "blockTime": time, "err": if failed { json!({"x": 1}) } else { Value::Null } })
    }

    /// Range = (after_slot, until_time]; failed txs dropped; output oldest first; stops once past the range
    /// when no history is wanted.
    #[test]
    fn scan_state_range_and_order() {
        let mut st = ScanState::new(100, 1_000, false);
        let page1 = vec![e("late", 300, 1_200, false), e("c", 250, 900, false), e("f", 200, 800, true), e("b", 150, 700, false)];
        assert!(st.page(&page1, 4));
        let page2 = vec![e("a", 120, 600, false), e("old", 100, 500, false), e("older", 90, 400, false), e("x", 80, 300, false)];
        assert!(!st.page(&page2, 4), "past after_slot and no history wanted: stop");
        let s = st.finish();
        let sigs: Vec<&str> = s.in_range.iter().map(|x| x.sig.as_str()).collect();
        assert_eq!(sigs, ["a", "b", "c"]);
        assert!(s.range_complete && !s.exhausted);
        assert_eq!(s.oldest.unwrap().sig, "x");
    }

    /// With history wanted, paging continues past the range until a short page (first signature reached).
    #[test]
    fn scan_state_history_until_exhausted() {
        let mut st = ScanState::new(100, 1_000, true);
        assert!(st.page(&[e("b", 150, 700, false), e("old", 90, 400, false)], 2));
        assert!(!st.page(&[e("first", 10, 100, false)], 2));
        let s = st.finish();
        assert!(s.exhausted && s.range_complete);
        assert_eq!(s.oldest.unwrap().sig, "first");
        assert_eq!(s.in_range.len(), 1);
    }
}
