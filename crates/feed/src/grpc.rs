use crate::{control, MsgTx};
use events::{RawMsg, Source};
use futures::StreamExt;
use helius_laserstream::grpc::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest, SubscribeRequestFilterBlockFooter,
    SubscribeRequestFilterBlocksMeta, SubscribeRequestFilterSlots, SubscribeRequestFilterTransactions,
};
use helius_laserstream::{subscribe, LaserstreamConfig};
use prost::Message;
use std::time::Duration;
use tokio::sync::watch;

/// Processed-commitment subscription: transactions touching `programs` (or all non-vote if None),
/// every slot status, block meta and block footers (needed for gap tracking and slot-time latency).
pub fn build_request(programs: Option<&[String]>) -> SubscribeRequest {
    let mut req = SubscribeRequest { commitment: Some(CommitmentLevel::Processed as i32), ..Default::default() };
    req.transactions.insert(
        "tx".into(),
        SubscribeRequestFilterTransactions {
            vote: Some(false),
            failed: None,
            account_include: programs.map(|p| p.to_vec()).unwrap_or_default(),
            ..Default::default()
        },
    );
    req.slots.insert(
        "slots".into(),
        SubscribeRequestFilterSlots { filter_by_commitment: Some(false), interslot_updates: Some(true), ..Default::default() },
    );
    req.blocks_meta.insert("meta".into(), SubscribeRequestFilterBlocksMeta::default());
    req.block_footer.insert("footer".into(), SubscribeRequestFilterBlockFooter::default());
    req
}

fn update_slot(u: &UpdateOneof) -> Option<u64> {
    match u {
        UpdateOneof::Slot(s) => Some(s.slot),
        UpdateOneof::BlockMeta(m) => Some(m.slot),
        UpdateOneof::Transaction(t) => Some(t.slot),
        _ => None,
    }
}

/// Runs until `stop` flips to true. The SDK reconnects internally with replay from the last
/// tracked slot; if it gives up (terminal error) we resubscribe from our own last seen slot.
pub async fn run(endpoint: String, token: String, programs: Option<Vec<String>>, tx: MsgTx, mut stop: watch::Receiver<bool>) {
    let mut last_slot: u64 = 0;
    let mut attempt: u32 = 0;
    loop {
        let mut req = build_request(programs.as_deref());
        if last_slot > 0 {
            req.from_slot = Some(last_slot.saturating_sub(31));
        }
        let cfg = LaserstreamConfig::new(endpoint.clone(), token.clone());
        let (stream, _handle) = subscribe(cfg, req);
        tokio::pin!(stream);
        let _ = tx.send(control("grpc_subscribe", serde_json::json!({ "attempt": attempt, "from_slot": last_slot }))).await;
        loop {
            tokio::select! {
                _ = stop.changed() => {
                    if *stop.borrow() { return; }
                }
                item = stream.next() => {
                    match item {
                        Some(Ok(u)) => {
                            let wall = latency::wall_ns();
                            let mono = latency::mono_ns();
                            if let Some(s) = u.update_oneof.as_ref().and_then(update_slot) {
                                last_slot = last_slot.max(s);
                            }
                            let msg = RawMsg { source: Source::Grpc, recv_wall_ns: wall, recv_mono_ns: mono, payload: u.encode_to_vec() };
                            if tx.send(msg).await.is_err() { return; }
                        }
                        Some(Err(e)) => {
                            tracing::error!(error = %e, "laserstream terminal error, resubscribing");
                            let _ = tx.send(control("grpc_terminal_error", serde_json::json!({}))).await;
                            break;
                        }
                        None => {
                            let _ = tx.send(control("grpc_stream_end", serde_json::json!({}))).await;
                            break;
                        }
                    }
                }
            }
        }
        attempt += 1;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
