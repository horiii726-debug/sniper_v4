//! Raw message -> normalized events. Pure functions of the payload.

use crate::raw::{RawMsg, Source};
use anyhow::{anyhow, Context, Result};
use helius_laserstream::grpc::{subscribe_update::UpdateOneof, SubscribeUpdate};
use prost::Message;
use serde_json::Value;

pub type Sig = [u8; 64];

#[derive(Debug, Clone, PartialEq)]
pub struct TxRef {
    pub sig: Sig,
    pub index: u64,
    pub failed: bool,
    /// All account keys (static + loaded). Filled for RPC blocks only, so the pipeline can apply the program filter.
    pub accounts: Vec<String>,
    pub is_vote: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Norm {
    Slot { slot: u64, parent: Option<u64>, status: i32, dead: bool },
    BlockMeta { slot: u64, parent_slot: u64, blockhash: String, block_time: Option<i64>, executed_tx: u64 },
    BlockFooter { slot: u64, bank_id: u64, producer_time_ns: u64 },
    Tx { slot: u64, tx: TxRef, created_at_ns: Option<u64> },
    UpdateParent { slot: u64, cleared_bank_id: u64, parent_slot: u64 },
    WsSlot { slot: u64, parent: u64, root: u64 },
    WsLog { sub_id: u64, slot: u64, sig: Sig, failed: bool },
    /// `getBlock` backfill result. `block == None` means the slot was skipped / has no block.
    RpcBlock { slot: u64, block: Option<RpcBlockData>, error: Option<String> },
    /// `getSignaturesForAddress` backfill page set for one program.
    RpcSigs { program: String, sigs: Vec<(Sig, u64, bool)>, complete: bool },
    Control(Value),
    Ignored,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RpcBlockData {
    pub parent_slot: u64,
    pub blockhash: String,
    pub block_time: Option<i64>,
    pub txs: Vec<TxRef>,
}

const VOTE_PROGRAM: &str = "Vote111111111111111111111111111111111111111";

pub fn decode_sig_b58(s: &str) -> Result<Sig> {
    let v = bs58::decode(s).into_vec().map_err(|e| anyhow!("bad sig {s}: {e}"))?;
    v.try_into().map_err(|_| anyhow!("sig len != 64"))
}

fn sig_from_bytes(b: &[u8]) -> Result<Sig> {
    b.try_into().map_err(|_| anyhow!("sig len {} != 64", b.len()))
}

pub fn normalize(msg: &RawMsg) -> Result<Vec<Norm>> {
    match msg.source {
        Source::Grpc => normalize_grpc(&msg.payload),
        Source::Ws => normalize_ws(&msg.payload),
        Source::Rpc => normalize_rpc(&msg.payload),
        Source::Control => Ok(vec![Norm::Control(serde_json::from_slice(&msg.payload)?)]),
    }
}

fn normalize_grpc(payload: &[u8]) -> Result<Vec<Norm>> {
    let u = SubscribeUpdate::decode(payload).context("decode SubscribeUpdate")?;
    let created_at_ns = u
        .created_at
        .as_ref()
        .filter(|t| t.seconds > 0)
        .map(|t| t.seconds as u64 * 1_000_000_000 + t.nanos.max(0) as u64);
    let n = match u.update_oneof {
        Some(UpdateOneof::Slot(s)) => {
            Norm::Slot { slot: s.slot, parent: s.parent, status: s.status, dead: s.dead_error.is_some() }
        }
        Some(UpdateOneof::BlockMeta(m)) => Norm::BlockMeta {
            slot: m.slot,
            parent_slot: m.parent_slot,
            blockhash: m.blockhash,
            block_time: m.block_time.map(|t| t.timestamp),
            executed_tx: m.executed_transaction_count,
        },
        Some(UpdateOneof::BlockFooter(f)) => {
            Norm::BlockFooter { slot: f.slot, bank_id: f.bank_id, producer_time_ns: f.block_producer_time_nanos }
        }
        Some(UpdateOneof::Transaction(t)) => {
            let info = t.transaction.ok_or_else(|| anyhow!("tx update without info"))?;
            let failed = info.meta.as_ref().map(|m| m.err.is_some()).unwrap_or(false);
            Norm::Tx {
                slot: t.slot,
                tx: TxRef {
                    sig: sig_from_bytes(&info.signature)?,
                    index: info.index,
                    failed,
                    accounts: Vec::new(),
                    is_vote: info.is_vote,
                },
                created_at_ns,
            }
        }
        Some(UpdateOneof::EntryUpdateParent(p)) => {
            Norm::UpdateParent { slot: p.slot, cleared_bank_id: p.cleared_bank_id, parent_slot: p.parent_slot }
        }
        _ => Norm::Ignored,
    };
    Ok(vec![n])
}

fn normalize_ws(payload: &[u8]) -> Result<Vec<Norm>> {
    let v: Value = serde_json::from_slice(payload)?;
    let method = v.get("method").and_then(Value::as_str).unwrap_or("");
    let params = &v["params"];
    let n = match method {
        "slotNotification" => {
            let r = &params["result"];
            Norm::WsSlot {
                slot: r["slot"].as_u64().unwrap_or(0),
                parent: r["parent"].as_u64().unwrap_or(0),
                root: r["root"].as_u64().unwrap_or(0),
            }
        }
        "logsNotification" => {
            let r = &params["result"];
            Norm::WsLog {
                sub_id: params["subscription"].as_u64().unwrap_or(0),
                slot: r["context"]["slot"].as_u64().unwrap_or(0),
                sig: decode_sig_b58(r["value"]["signature"].as_str().unwrap_or(""))?,
                failed: !r["value"]["err"].is_null(),
            }
        }
        // Subscription confirmations are carried by Control messages.
        _ => Norm::Ignored,
    };
    Ok(vec![n])
}

/// Parse one JSON-RPC `getBlock` transaction (encoding=json) into a TxRef.
fn rpc_tx(index: u64, t: &Value) -> Result<TxRef> {
    let tx = &t["transaction"];
    let meta = &t["meta"];
    let sig = decode_sig_b58(tx["signatures"][0].as_str().ok_or_else(|| anyhow!("no signature"))?)?;
    let msg = &tx["message"];
    let mut accounts: Vec<String> =
        msg["accountKeys"].as_array().map(|a| a.iter().filter_map(|k| k.as_str().map(String::from)).collect()).unwrap_or_default();
    for side in ["writable", "readonly"] {
        if let Some(a) = meta["loadedAddresses"][side].as_array() {
            accounts.extend(a.iter().filter_map(|k| k.as_str().map(String::from)));
        }
    }
    // Same definition as Agave's simple vote tx: one instruction, invoking the vote program.
    let ixs = msg["instructions"].as_array().cloned().unwrap_or_default();
    let is_vote = ixs.len() == 1
        && ixs[0]["programIdIndex"].as_u64().and_then(|i| accounts.get(i as usize)).map(|p| p == VOTE_PROGRAM).unwrap_or(false);
    Ok(TxRef { sig, index, failed: !meta["err"].is_null(), accounts, is_vote })
}

fn normalize_rpc(payload: &[u8]) -> Result<Vec<Norm>> {
    let v: Value = serde_json::from_slice(payload)?;
    match v["method"].as_str().unwrap_or("") {
        "getBlock" => {
            let slot = v["params"]["slot"].as_u64().ok_or_else(|| anyhow!("getBlock without slot"))?;
            if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
                return Ok(vec![Norm::RpcBlock { slot, block: None, error: Some(err.to_string()) }]);
            }
            let r = &v["result"];
            if r.is_null() {
                return Ok(vec![Norm::RpcBlock { slot, block: None, error: None }]);
            }
            let mut txs = Vec::new();
            for (i, t) in r["transactions"].as_array().cloned().unwrap_or_default().iter().enumerate() {
                txs.push(rpc_tx(i as u64, t)?);
            }
            Ok(vec![Norm::RpcBlock {
                slot,
                block: Some(RpcBlockData {
                    parent_slot: r["parentSlot"].as_u64().unwrap_or(0),
                    blockhash: r["blockhash"].as_str().unwrap_or("").to_string(),
                    block_time: r["blockTime"].as_i64(),
                    txs,
                }),
                error: None,
            }])
        }
        "getSignaturesForAddress" => {
            let program = v["params"]["program"].as_str().unwrap_or("").to_string();
            let mut sigs = Vec::new();
            for e in v["result"].as_array().cloned().unwrap_or_default() {
                sigs.push((
                    decode_sig_b58(e["signature"].as_str().unwrap_or(""))?,
                    e["slot"].as_u64().unwrap_or(0),
                    !e["err"].is_null(),
                ));
            }
            Ok(vec![Norm::RpcSigs { program, sigs, complete: v["params"]["complete"].as_bool().unwrap_or(false) }])
        }
        m => Err(anyhow!("unknown rpc method {m}")),
    }
}
