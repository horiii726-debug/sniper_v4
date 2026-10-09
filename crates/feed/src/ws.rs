//! Solana JSON-RPC WebSocket fallback feed: `slotSubscribe` + one `logsSubscribe` per program.
//! On reconnect, the outage window is backfilled per program with `getSignaturesForAddress`.

use crate::rpc::{sigs_msg, Rpc};
use crate::{control, MsgTx};
use events::{RawMsg, Source};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Wait before backfilling so the outage window is confirmed on the RPC node.
const BACKFILL_DELAY: Duration = Duration::from_secs(20);

pub async fn run(url: String, rpc: Rpc, programs: Vec<String>, tx: MsgTx, mut stop: watch::Receiver<bool>) {
    let mut last_sig: HashMap<String, String> = HashMap::new();
    let mut connects: u64 = 0;
    loop {
        if *stop.borrow() {
            return;
        }
        let conn = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(url.as_str())).await;
        let mut ws = match conn {
            Ok(Ok((ws, _))) => ws,
            Ok(Err(e)) => {
                // tungstenite errors can embed the request URL: log only the kind.
                tracing::warn!(kind = %ws_err_kind(&e), "ws connect failed");
                let _ = tx.send(control("ws_connect_failed", json!({}))).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            Err(_) => {
                let _ = tx.send(control("ws_connect_timeout", json!({}))).await;
                continue;
            }
        };
        connects += 1;
        let _ = tx.send(control("ws_connected", json!({ "n": connects }))).await;
        let _ = ws.send(Message::Text(json!({"jsonrpc":"2.0","id":1,"method":"slotSubscribe"}).to_string())).await;
        let mut req_program: HashMap<u64, String> = HashMap::new();
        let mut sub_program: HashMap<u64, String> = HashMap::new();
        for (i, p) in programs.iter().enumerate() {
            let id = 100 + i as u64;
            req_program.insert(id, p.clone());
            let m = json!({"jsonrpc":"2.0","id":id,"method":"logsSubscribe",
                "params":[{"mentions":[p]},{"commitment":"processed"}]});
            let _ = ws.send(Message::Text(m.to_string())).await;
        }
        // Backfill the outage window (only after a reconnect, when we have a resume point).
        if connects > 1 {
            let gap_from = last_sig.clone();
            let rpc = rpc.clone();
            let tx2 = tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(BACKFILL_DELAY).await;
                for (program, until) in gap_from {
                    match rpc.signatures_until(&program, &until, 50).await {
                        Ok((sigs, complete)) => {
                            let _ = tx2.send(sigs_msg(&program, sigs, complete)).await;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, program = %program, "ws backfill failed");
                            let _ = tx2.send(sigs_msg(&program, vec![], false)).await;
                        }
                    }
                }
            });
        }
        let mut ping = tokio::time::interval(Duration::from_secs(15));
        let reason = loop {
            tokio::select! {
                _ = stop.changed() => {
                    if *stop.borrow() { let _ = ws.close(None).await; return; }
                }
                _ = ping.tick() => {
                    if ws.send(Message::Ping(Vec::new())).await.is_err() { break "ping_failed"; }
                }
                m = tokio::time::timeout(IDLE_TIMEOUT, ws.next()) => {
                    let m = match m {
                        Err(_) => break "idle_timeout",
                        Ok(None) => break "closed",
                        Ok(Some(Err(_))) => break "error",
                        Ok(Some(Ok(m))) => m,
                    };
                    let text = match m {
                        Message::Text(t) => t,
                        Message::Close(_) => break "close_frame",
                        _ => continue,
                    };
                    let wall = latency::wall_ns();
                    let mono = latency::mono_ns();
                    let v: Value = match serde_json::from_str(&text) { Ok(v) => v, Err(_) => continue };
                    if let Some(id) = v.get("id").and_then(Value::as_u64) {
                        // Subscription response.
                        if let Some(p) = req_program.get(&id) {
                            if let Some(sub) = v["result"].as_u64() {
                                sub_program.insert(sub, p.clone());
                                let _ = tx.send(control("ws_sub", json!({"sub_id": sub, "program": p}))).await;
                            } else {
                                let _ = tx.send(control("ws_sub_error", json!({"program": p, "error": v["error"]}))).await;
                            }
                        }
                        continue;
                    }
                    if v["method"] == "logsNotification" {
                        let sub = v["params"]["subscription"].as_u64().unwrap_or(0);
                        if let (Some(p), Some(sig)) = (sub_program.get(&sub), v["params"]["result"]["value"]["signature"].as_str()) {
                            last_sig.insert(p.clone(), sig.to_string());
                        }
                    }
                    let msg = RawMsg { source: Source::Ws, recv_wall_ns: wall, recv_mono_ns: mono, payload: text.into_bytes() };
                    if tx.send(msg).await.is_err() { return; }
                }
            }
        };
        let _ = tx.send(control("ws_disconnected", json!({ "reason": reason }))).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn ws_err_kind(e: &tokio_tungstenite::tungstenite::Error) -> &'static str {
    use tokio_tungstenite::tungstenite::Error as E;
    match e {
        E::Http(_) => "http",
        E::Io(_) => "io",
        E::Tls(_) => "tls",
        E::Url(_) => "url",
        E::Protocol(_) => "protocol",
        _ => "other",
    }
}
