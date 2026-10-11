//! Historical backfill (HISTORICAL-FIRST mode, G0.H1–H3): fetch every slot of a range with getBlock,
//! keep transactions touching the venue programs OR trading a token against any pool (program-agnostic pool legs,
//! `decoder::pool_legs`: unknown venues, routes), and emit RawMsgs in the live recorder format
//! (`Source::Rpc`, method `getBlock`) so the same replay/pipeline code reads both.

use crate::rpc::Rpc;
use anyhow::{anyhow, Result};
use events::{RawMsg, Source};
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Default, Debug)]
pub struct FetchCounters {
    pub get_block_calls: AtomicU64,
    pub get_block_retries: AtomicU64,
    pub get_blocks_calls: AtomicU64,
    pub bytes_decoded: AtomicU64,
    pub txs_total: AtomicU64,
    pub txs_kept: AtomicU64,
}

/// Confirmed (non-skipped) block slots in [start, end], per the cluster (`getBlocks`, finalized).
pub async fn confirmed_blocks(rpc: &Rpc, start: u64, end: u64, c: &FetchCounters) -> Result<Vec<u64>> {
    let mut out = Vec::new();
    let mut from = start;
    while from <= end {
        let to = end.min(from + 499_999);
        c.get_blocks_calls.fetch_add(1, Ordering::Relaxed);
        let v = rpc.call("getBlocks", json!([from, to, {"commitment": "finalized"}])).await?;
        if let Some(e) = v.get("error") {
            return Err(anyhow!("getBlocks: {e}"));
        }
        out.extend(v["result"].as_array().ok_or_else(|| anyhow!("getBlocks: no result"))?.iter().filter_map(Value::as_u64));
        from = to + 1;
    }
    Ok(out)
}

pub fn tx_touches(t: &Value, programs: &HashSet<String>) -> bool {
    let keys = t["transaction"]["message"]["accountKeys"].as_array();
    let hit = |a: Option<&Vec<Value>>| a.is_some_and(|a| a.iter().any(|k| k.as_str().is_some_and(|k| programs.contains(k))));
    hit(keys) || hit(t["meta"]["loadedAddresses"]["writable"].as_array()) || hit(t["meta"]["loadedAddresses"]["readonly"].as_array())
}

/// One slot: getBlock with retries on transient errors (rate limit, node behind).
/// The recorded result keeps block header fields and only the venue transactions.
async fn fetch_slot(rpc: Rpc, slot: u64, programs: Arc<HashSet<String>>, c: Arc<FetchCounters>) -> RawMsg {
    let params = json!([slot, {
        "encoding": "json", "maxSupportedTransactionVersion": 1,
        "transactionDetails": "full", "rewards": false, "commitment": "finalized"
    }]);
    let mut last_err = Value::Null;
    for attempt in 0..8u32 {
        c.get_block_calls.fetch_add(1, Ordering::Relaxed);
        if attempt > 0 {
            c.get_block_retries.fetch_add(1, Ordering::Relaxed);
        }
        match rpc.call_bytes("getBlock", params.clone()).await {
            Ok((v, nbytes)) => {
                c.bytes_decoded.fetch_add(nbytes, Ordering::Relaxed);
                if let Some(err) = v.get("error") {
                    let code = err["code"].as_i64().unwrap_or(0);
                    if code == -32007 || code == -32009 {
                        return wrap(slot, Value::Null, Value::Null, 0);
                    }
                    last_err = err.clone();
                    if ![-32004, -32014, -32016, -32429, -32005].contains(&code) {
                        break;
                    }
                } else {
                    let mut r = v["result"].clone();
                    let txs = r["transactions"].take();
                    let all = txs.as_array().cloned().unwrap_or_default();
                    let n_all = all.len() as u64;
                    let kept: Vec<Value> = all.into_iter().filter(|t| tx_touches(t, &programs) || decoder::pool_legs::has_token_leg(t)).collect();
                    c.txs_total.fetch_add(n_all, Ordering::Relaxed);
                    c.txs_kept.fetch_add(kept.len() as u64, Ordering::Relaxed);
                    r["transactions"] = Value::Array(kept);
                    return wrap(slot, r, Value::Null, n_all);
                }
            }
            Err(e) => last_err = json!({ "message": e.to_string() }),
        }
        tokio::time::sleep(Duration::from_millis(250 * 2u64.pow(attempt.min(5)))).await;
    }
    wrap(slot, Value::Null, last_err, 0)
}

fn wrap(slot: u64, result: Value, error: Value, block_tx_count: u64) -> RawMsg {
    let v = json!({
        "method": "getBlock",
        "params": { "slot": slot, "filtered": "venue_programs_or_pool_legs", "block_tx_count": block_tx_count },
        "result": result, "error": error
    });
    RawMsg { source: Source::Rpc, recv_wall_ns: latency::wall_ns(), recv_mono_ns: latency::mono_ns(), payload: v.to_string().into_bytes() }
}

/// Fetch `slots` with bounded concurrency; results are yielded in slot order (deterministic recording).
pub fn fetch_ordered(
    rpc: Rpc,
    slots: Vec<u64>,
    programs: Arc<HashSet<String>>,
    concurrency: usize,
    c: Arc<FetchCounters>,
) -> impl futures::Stream<Item = RawMsg> {
    futures::stream::iter(slots)
        .map(move |s| fetch_slot(rpc.clone(), s, programs.clone(), c.clone()))
        .buffered(concurrency)
}
