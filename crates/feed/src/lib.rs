//! Feed sources (V4 §3): LaserStream gRPC (primary), Solana WebSocket (fallback), JSON-RPC backfill.
//! Every source pushes `RawMsg` into one bounded channel consumed by the recorder + pipeline thread.

pub mod backfill;
pub mod follow;
pub mod grpc;
pub mod rpc;
pub mod ws;

use events::RawMsg;
use tokio::sync::mpsc;

pub type MsgTx = mpsc::Sender<RawMsg>;

pub fn control(event: &str, extra: serde_json::Value) -> RawMsg {
    let mut v = serde_json::json!({ "event": event });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in e {
            o.insert(k.clone(), val.clone());
        }
    }
    RawMsg::control(latency::wall_ns(), latency::mono_ns(), v)
}
