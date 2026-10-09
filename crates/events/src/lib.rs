//! Canonical event bus primitives for Stage 0: raw feed messages, normalization,
//! dedup and slot-gap tracking. Everything here is deterministic: it only reads
//! data carried by the messages, never the wall clock (V4 §3.3, §26).

pub mod gap;
pub mod normalize;
pub mod pipeline;
pub mod raw;

pub use normalize::{normalize, Norm, TxRef};
pub use pipeline::{Action, Pipeline, PipelineStats, ProgramFilter};
pub use raw::{RawMsg, Source};
