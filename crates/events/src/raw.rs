/// Where a raw message came from. The value is stored in recordings: never renumber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Source {
    /// LaserStream gRPC `SubscribeUpdate`, protobuf-encoded.
    Grpc = 1,
    /// Solana JSON-RPC WebSocket notification text.
    Ws = 2,
    /// JSON-RPC backfill response wrapped as `{"method":..,"params":..,"result":..}`.
    Rpc = 3,
    /// Process control marker, JSON (`{"event":..}`).
    Control = 4,
}

impl Source {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Source::Grpc),
            2 => Some(Source::Ws),
            3 => Some(Source::Rpc),
            4 => Some(Source::Control),
            _ => None,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Source::Grpc => "grpc",
            Source::Ws => "ws",
            Source::Rpc => "rpc",
            Source::Control => "ctl",
        }
    }
}

/// One message exactly as received, with receive timestamps (ns).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMsg {
    pub source: Source,
    pub recv_wall_ns: u64,
    pub recv_mono_ns: u64,
    pub payload: Vec<u8>,
}

impl RawMsg {
    pub fn control(recv_wall_ns: u64, recv_mono_ns: u64, json: serde_json::Value) -> Self {
        RawMsg { source: Source::Control, recv_wall_ns, recv_mono_ns, payload: json.to_string().into_bytes() }
    }
}
