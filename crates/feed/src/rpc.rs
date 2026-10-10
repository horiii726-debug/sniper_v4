//! JSON-RPC calls used off the hot path: gap backfill and verification.

use anyhow::{anyhow, Result};
use events::{RawMsg, Source};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone)]
pub struct Rpc {
    http: reqwest::Client,
    url: String,
}

/// JSON-RPC error codes that mean "this slot has no block" (final answer, no retry).
const SKIPPED_CODES: [i64; 2] = [-32007, -32009];
/// Error codes worth retrying (block not yet available, node behind, rate limited).
const TRANSIENT_CODES: [i64; 5] = [-32004, -32014, -32016, -32429, -32005];

impl Rpc {
    pub fn new(url: String) -> Self {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().expect("http client");
        Self { http, url }
    }

    /// Raw response JSON (`result` or `error`). Errors never include the URL (it carries the key).
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let resp = self.http.post(&self.url).json(&body).send().await.map_err(|e| anyhow!("{method}: {}", e.without_url()))?;
        let status = resp.status();
        let v: Value = resp.json().await.map_err(|e| anyhow!("{method}: body: {}", e.without_url()))?;
        if !status.is_success() && v.get("error").is_none() {
            return Err(anyhow!("{method}: http {status}"));
        }
        Ok(v)
    }

    /// Like `call`, also returning the decoded body size in bytes.
    pub async fn call_bytes(&self, method: &str, params: Value) -> Result<(Value, u64)> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let resp = self.http.post(&self.url).json(&body).send().await.map_err(|e| anyhow!("{method}: {}", e.without_url()))?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| anyhow!("{method}: body: {}", e.without_url()))?;
        let n = bytes.len() as u64;
        let v: Value = serde_json::from_slice(&bytes).map_err(|_| anyhow!("{method}: http {status}, non-JSON body"))?;
        if !status.is_success() && v.get("error").is_none() {
            return Err(anyhow!("{method}: http {status}"));
        }
        Ok((v, n))
    }

    /// `getBlock` with retries. Returns the RawMsg to record (block, skipped, or final error).
    pub async fn get_block_msg(&self, slot: u64, commitment: &str) -> RawMsg {
        let params = json!([slot, {
            "encoding": "json", "maxSupportedTransactionVersion": 1,
            "transactionDetails": "full", "rewards": false, "commitment": commitment
        }]);
        let mut last_err = Value::Null;
        for attempt in 0..5u32 {
            match self.call("getBlock", params.clone()).await {
                Ok(v) => {
                    if let Some(err) = v.get("error") {
                        let code = err["code"].as_i64().unwrap_or(0);
                        if SKIPPED_CODES.contains(&code) {
                            return wrap(slot, Value::Null, Value::Null);
                        }
                        last_err = err.clone();
                        if !TRANSIENT_CODES.contains(&code) {
                            break;
                        }
                    } else {
                        return wrap(slot, v["result"].clone(), Value::Null);
                    }
                }
                Err(e) => last_err = json!({ "message": e.to_string() }),
            }
            tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
        }
        wrap(slot, Value::Null, last_err)
    }

    /// All signatures for `program` newer than `until` (exclusive), newest first, paged.
    pub async fn signatures_until(&self, program: &str, until: &str, max_pages: usize) -> Result<(Vec<Value>, bool)> {
        let mut out = Vec::new();
        let mut before: Option<String> = None;
        for _ in 0..max_pages {
            let mut cfg = json!({ "limit": 1000, "until": until, "commitment": "confirmed" });
            if let Some(b) = &before {
                cfg["before"] = json!(b);
            }
            let v = self.call("getSignaturesForAddress", json!([program, cfg])).await?;
            if let Some(err) = v.get("error") {
                return Err(anyhow!("getSignaturesForAddress: {err}"));
            }
            let page = v["result"].as_array().cloned().unwrap_or_default();
            let n = page.len();
            before = page.last().and_then(|e| e["signature"].as_str().map(String::from));
            out.extend(page);
            if n < 1000 {
                return Ok((out, true));
            }
        }
        Ok((out, false))
    }
}

fn wrap(slot: u64, result: Value, error: Value) -> RawMsg {
    let v = json!({ "method": "getBlock", "params": { "slot": slot }, "result": result, "error": error });
    RawMsg { source: Source::Rpc, recv_wall_ns: latency::wall_ns(), recv_mono_ns: latency::mono_ns(), payload: v.to_string().into_bytes() }
}

pub fn sigs_msg(program: &str, sigs: Vec<Value>, complete: bool) -> RawMsg {
    let v = json!({ "method": "getSignaturesForAddress", "params": { "program": program, "complete": complete }, "result": sigs });
    RawMsg { source: Source::Rpc, recv_wall_ns: latency::wall_ns(), recv_mono_ns: latency::mono_ns(), payload: v.to_string().into_bytes() }
}
