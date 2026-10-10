//! Stage 1 decoding. `balance_delta` is the venue-agnostic reference decoder (V4 §9): it derives the
//! economic trade of the transaction's fee payer from pre/post token and lamport balances.

pub mod balance_delta;
