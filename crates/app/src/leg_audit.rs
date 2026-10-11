//! Coverage audit of the recording filter: fetch recent blocks UNFILTERED and count pool legs (any venue, any
//! program) in transactions the venue-program filter keeps vs drops, with the programs of the dropped ones.

use anyhow::{anyhow, Result};
use decoder::pool_legs::{self, LegKind, Quote};
use feed::rpc::Rpc;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

const INFRA: &[&str] = &[
    "11111111111111111111111111111111", "ComputeBudget111111111111111111111111111111", "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
    "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb", "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL", "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",
    "Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo",
];

fn invoked(t: &Value) -> Vec<String> {
    let mut k: Vec<&str> = t["transaction"]["message"]["accountKeys"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    for side in ["writable", "readonly"] {
        if let Some(a) = t["meta"]["loadedAddresses"][side].as_array() {
            k.extend(a.iter().filter_map(|x| x.as_str()));
        }
    }
    let mut out: HashSet<String> = HashSet::new();
    let mut add = |ix: &Value| {
        if let Some(p) = ix["programIdIndex"].as_u64().and_then(|i| k.get(i as usize)) {
            if !INFRA.contains(p) {
                out.insert(p.to_string());
            }
        }
    };
    for ix in t["transaction"]["message"]["instructions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        add(ix);
    }
    for inner in t["meta"]["innerInstructions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        for ix in inner["instructions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            add(ix);
        }
    }
    let mut v: Vec<String> = out.into_iter().collect();
    v.sort();
    v
}

pub async fn run(rpc_url: String, programs: Vec<String>, slots: u64, out: &std::path::Path) -> Result<()> {
    let rpc = Rpc::new(rpc_url);
    let tip = rpc.call("getSlot", json!([{"commitment": "finalized"}])).await?["result"].as_u64().ok_or_else(|| anyhow!("getSlot"))? - 200;
    let pset: HashSet<String> = programs.into_iter().collect();
    let mut c: BTreeMap<&str, u64> = BTreeMap::new();
    let mut dropped_prog: HashMap<String, (u64, u64)> = HashMap::new(); // program -> (txs, legs)
    let mut dropped_mints: HashSet<String> = HashSet::new();
    let mut kept_mints: HashSet<String> = HashSet::new();
    let (mut bytes_old, mut bytes_new, mut bytes_all) = (0u64, 0u64, 0u64);
    let mut fetched = 0u64;
    for slot in (tip + 1 - slots)..=tip {
        let params = json!([slot, {"encoding": "json", "maxSupportedTransactionVersion": 1, "transactionDetails": "full", "rewards": false, "commitment": "finalized"}]);
        let mut v = Value::Null;
        for attempt in 0..8u32 {
            match rpc.call("getBlock", params.clone()).await {
                Ok(r) if r.get("error").is_none() => { v = r; break; }
                Ok(r) if [-32007, -32009].contains(&r["error"]["code"].as_i64().unwrap_or(0)) => break,
                _ => tokio::time::sleep(std::time::Duration::from_millis(300 * 2u64.pow(attempt.min(4)))).await,
            }
        }
        let Some(txs) = v["result"]["transactions"].as_array() else { continue };
        fetched += 1;
        for t in txs {
            let sz = t.to_string().len() as u64;
            let is_vote = invoked(t).iter().any(|p| p == "Vote111111111111111111111111111111111111111");
            if is_vote {
                *c.entry("vote_txs").or_default() += 1;
                continue;
            }
            bytes_all += sz;
            *c.entry("nonvote_txs").or_default() += 1;
            let old = feed::backfill::tx_touches(t, &pset);
            let (legs, skips) = pool_legs::legs(t);
            for s in &skips {
                *c.entry(match s { pool_legs::LegSkip::Failed => "skip_failed", pool_legs::LegSkip::Ambiguous => "skip_ambiguous", pool_legs::LegSkip::NativeMismatch => "skip_native_mismatch", pool_legs::LegSkip::NotPool => "skip_not_pool" }).or_default() += 1;
            }
            let token_legs: Vec<_> = legs.iter().filter(|l| l.kind != LegKind::Price).collect();
            let new = old || !token_legs.is_empty();
            if old { bytes_old += sz; }
            if new { bytes_new += sz; }
            if legs.iter().any(|l| l.kind == LegKind::Price) { *c.entry("txs_with_price_leg").or_default() += 1; }
            if token_legs.is_empty() { continue; }
            *c.entry("txs_with_token_legs").or_default() += 1;
            for l in &token_legs {
                *c.entry(match (l.kind, l.quote) { (LegKind::Native, _) => "legs_native", (_, Quote::Sol) => "legs_vault_sol", (_, Quote::Usdc) => "legs_vault_usdc", _ => "legs_vault_usdt" }).or_default() += 1;
                if old { kept_mints.insert(l.mint.clone()); } else { dropped_mints.insert(l.mint.clone()); }
            }
            if old {
                *c.entry("token_leg_txs_kept_by_old_filter").or_default() += 1;
                *c.entry("token_legs_kept_by_old_filter").or_default() += token_legs.len() as u64;
            } else {
                *c.entry("token_leg_txs_DROPPED_by_old_filter").or_default() += 1;
                *c.entry("token_legs_DROPPED_by_old_filter").or_default() += token_legs.len() as u64;
                for p in invoked(t) {
                    let e = dropped_prog.entry(p).or_default();
                    e.0 += 1;
                    e.1 += token_legs.len() as u64;
                }
            }
        }
    }
    let only_dropped = dropped_mints.difference(&kept_mints).count();
    let mut progs: Vec<(String, (u64, u64))> = dropped_prog.into_iter().collect();
    progs.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    let rep = json!({
        "label": "recording-filter coverage audit (recent unfiltered blocks)", "tip_slot": tip, "slots": slots, "blocks": fetched,
        "counts": c, "mints_seen_kept": kept_mints.len(), "mints_only_in_dropped_txs": only_dropped,
        "bytes_nonvote": bytes_all, "bytes_old_filter": bytes_old, "bytes_new_filter": bytes_new,
        "dropped_tx_programs_top": progs.iter().take(40).map(|(p, (t, l))| json!({"program": p, "txs": t, "legs": l})).collect::<Vec<_>>(),
    });
    std::fs::create_dir_all(out.parent().unwrap_or(std::path::Path::new(".")))?;
    std::fs::write(out, serde_json::to_string_pretty(&rep)?)?;
    println!("{}", serde_json::to_string_pretty(&rep)?);
    Ok(())
}

/// Trade-vs-transfer audit on a recording: for every transaction with token pool legs, which programs it invokes.
/// A leg in a transaction that invokes only infrastructure programs (system, token, ATA, compute budget, memo) would
/// be a transfer misread as a swap.
pub fn run_rec(rec: &std::path::Path, programs: Vec<String>, max_blocks: usize, out: &std::path::Path) -> Result<()> {
    let pset: HashSet<String> = programs.into_iter().collect();
    let mut c: BTreeMap<&str, u64> = BTreeMap::new();
    let mut other_prog: HashMap<String, (u64, u64)> = HashMap::new();
    let mut blocks = 0usize;
    for m in recorder::Reader::open(rec)? {
        let m = m?;
        if m.source != events::Source::Rpc {
            continue;
        }
        let v: Value = serde_json::from_slice(&m.payload)?;
        let Some(txs) = v["result"]["transactions"].as_array() else { continue };
        blocks += 1;
        if blocks > max_blocks {
            break;
        }
        for t in txs {
            let (legs, _) = pool_legs::legs(t);
            let token_legs = legs.iter().filter(|l| l.kind != LegKind::Price).count() as u64;
            if token_legs == 0 {
                continue;
            }
            let inv = invoked(t);
            *c.entry("txs_with_token_legs").or_default() += 1;
            *c.entry("token_legs").or_default() += token_legs;
            if inv.is_empty() {
                *c.entry("txs_INFRA_ONLY_possible_transfer").or_default() += 1;
                *c.entry("legs_INFRA_ONLY_possible_transfer").or_default() += token_legs;
            } else if inv.iter().any(|p| pset.contains(p)) {
                *c.entry("txs_known_venue_program").or_default() += 1;
            } else {
                *c.entry("txs_other_program").or_default() += 1;
                for p in inv {
                    let e = other_prog.entry(p).or_default();
                    e.0 += 1;
                    e.1 += token_legs;
                }
            }
        }
    }
    let mut progs: Vec<(String, (u64, u64))> = other_prog.into_iter().collect();
    progs.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    let rep = json!({"label": "trade-vs-transfer audit of pool legs (recording)", "rec": rec, "blocks": blocks.min(max_blocks), "counts": c,
        "other_programs_top": progs.iter().take(30).map(|(p, (t, l))| json!({"program": p, "txs": t, "legs": l})).collect::<Vec<_>>()});
    std::fs::write(out, serde_json::to_string_pretty(&rep)?)?;
    println!("{}", serde_json::to_string_pretty(&rep)?);
    Ok(())
}
