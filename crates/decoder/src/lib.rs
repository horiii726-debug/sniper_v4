//! Stage 1 decoding. `balance_delta` is the venue-agnostic reference decoder (V4 §9); `pump` decodes
//! pump.fun / PumpSwap events through the IDL-driven borsh reader in `idl`, over the flat
//! execution view in `txview`.

pub mod balance_delta;
pub mod idl;
pub mod pump;
pub mod txview;
pub mod pool_legs;
pub mod venue_pool;
