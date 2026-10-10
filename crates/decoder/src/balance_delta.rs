//! Venue-agnostic trade decoder from balance changes (JSON-RPC `getBlock` encoding=json).
//!
//! Trader = fee payer (account index 0). For every mint whose balance changed in token accounts owned
//! by the trader:
//!   token delta  = Σ (post - pre) raw amount over the trader's token accounts of that mint;
//!   SOL delta    = Δlamports(payer) + fee + Σ Δlamports(token accounts owned by payer).
//! Adding the lamports of the payer's token accounts nets out ATA rent and WSOL wrap/unwrap, so the
//! SOL delta is what the trader really paid/received (priority fee excluded, tips and venue fees included).
//! A transaction is a clean SOL trade when exactly one non-quote mint changed for the trader and no
//! stablecoin changed. Everything else is classified, never silently dropped.

use rustc_hash::FxHashMap;
use serde_json::Value;

pub const WSOL: &str = "So11111111111111111111111111111111111111112";
pub const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
pub const USDT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone)]
pub struct Trade {
    pub trader: String,
    pub mint: String,
    pub side: Side,
    /// Absolute token amount, raw units.
    pub token_raw: u128,
    pub decimals: u8,
    /// Absolute SOL paid (buy) or received (sell), lamports.
    pub sol_lamports: u64,
}

impl Trade {
    /// Price in SOL per whole token.
    pub fn price_sol(&self) -> f64 {
        let tokens = self.token_raw as f64 / 10f64.powi(self.decimals as i32);
        (self.sol_lamports as f64 / 1e9) / tokens
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Outcome {
    Trade,
    Failed,
    /// No non-quote token changed for the fee payer (arbitrage round trip, LP ops by others, relayed tx).
    NoTraderTokenChange,
    /// More than one non-quote mint changed (token-to-token route, LP add/remove).
    MultiMint,
    /// A stablecoin changed too (quote is not SOL).
    StableQuote,
    /// SOL moved in the same direction as the token (not a swap: e.g. LP withdraw, airdrop + fee).
    NotASwap,
    Malformed,
}

fn as_i128(v: &Value) -> Option<i128> {
    v.as_str()?.parse().ok()
}

/// Decode one `getBlock` (encoding=json) transaction.
pub fn decode(t: &Value) -> (Outcome, Option<Trade>) {
    let meta = &t["meta"];
    if !meta["err"].is_null() {
        return (Outcome::Failed, None);
    }
    let msg = &t["transaction"]["message"];
    let Some(payer) = msg["accountKeys"].get(0).and_then(Value::as_str) else { return (Outcome::Malformed, None) };
    let (Some(pre_l), Some(post_l)) = (meta["preBalances"].as_array(), meta["postBalances"].as_array()) else {
        return (Outcome::Malformed, None);
    };
    let lam = |i: usize| -> i128 {
        post_l.get(i).and_then(Value::as_i64).unwrap_or(0) as i128 - pre_l.get(i).and_then(Value::as_i64).unwrap_or(0) as i128
    };
    let fee = meta["fee"].as_i64().unwrap_or(0) as i128;

    // Token balances of the payer, keyed by account index.
    struct Acc {
        mint: String,
        decimals: u8,
        pre: i128,
        post: i128,
    }
    let mut accs: FxHashMap<u64, Acc> = FxHashMap::default();
    for (side, key) in [(0, "preTokenBalances"), (1, "postTokenBalances")] {
        for b in meta[key].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            if b["owner"].as_str() != Some(payer) {
                continue;
            }
            let Some(idx) = b["accountIndex"].as_u64() else { continue };
            let amt = as_i128(&b["uiTokenAmount"]["amount"]).unwrap_or(0);
            let e = accs.entry(idx).or_insert_with(|| Acc {
                mint: b["mint"].as_str().unwrap_or("").to_string(),
                decimals: b["uiTokenAmount"]["decimals"].as_u64().unwrap_or(0) as u8,
                pre: 0,
                post: 0,
            });
            if side == 0 {
                e.pre = amt;
            } else {
                e.post = amt;
            }
        }
    }
    let mut sol = lam(0) + fee;
    let mut by_mint: FxHashMap<&str, (i128, u8)> = FxHashMap::default();
    for (idx, a) in &accs {
        sol += lam(*idx as usize);
        if a.mint == WSOL {
            continue; // already counted through the account's lamports
        }
        let e = by_mint.entry(a.mint.as_str()).or_insert((0, a.decimals));
        e.0 += a.post - a.pre;
    }
    by_mint.retain(|_, (d, _)| *d != 0);
    if by_mint.keys().any(|m| *m == USDC || *m == USDT) {
        return (Outcome::StableQuote, None);
    }
    if by_mint.is_empty() {
        return (Outcome::NoTraderTokenChange, None);
    }
    if by_mint.len() > 1 {
        return (Outcome::MultiMint, None);
    }
    let (mint, (dtok, decimals)) = by_mint.into_iter().next().expect("one mint");
    let side = if dtok > 0 { Side::Buy } else { Side::Sell };
    // A buy must cost SOL, a sell must return SOL.
    if (side == Side::Buy && sol >= 0) || (side == Side::Sell && sol <= 0) {
        return (Outcome::NotASwap, None);
    }
    (
        Outcome::Trade,
        Some(Trade {
            trader: payer.to_string(),
            mint: mint.to_string(),
            side,
            token_raw: dtok.unsigned_abs(),
            decimals,
            sol_lamports: sol.unsigned_abs() as u64,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tb(idx: u64, owner: &str, mint: &str, amount: &str) -> Value {
        json!({"accountIndex": idx, "owner": owner, "mint": mint, "uiTokenAmount": {"amount": amount, "decimals": 6}})
    }

    #[test]
    fn buy_with_new_ata_nets_out_rent() {
        // payer pays 1 SOL + 2_039_280 rent into a new ATA (index 1) + 5000 fee; receives 1_000_000 raw tokens.
        let t = json!({
            "transaction": {"message": {"accountKeys": ["P", "ATA", "POOL"]}},
            "meta": {"err": null, "fee": 5000,
                "preBalances": [10_000_000_000i64, 0, 50_000_000_000i64],
                "postBalances": [10_000_000_000i64 - 1_000_000_000 - 2_039_280 - 5000, 2_039_280, 51_000_000_000i64],
                "preTokenBalances": [],
                "postTokenBalances": [tb(1, "P", "MINT", "1000000")]}
        });
        let (o, tr) = decode(&t);
        assert_eq!(o, Outcome::Trade);
        let tr = tr.unwrap();
        assert_eq!(tr.side, Side::Buy);
        assert_eq!(tr.sol_lamports, 1_000_000_000);
        assert_eq!(tr.token_raw, 1_000_000);
        assert!((tr.price_sol() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn sell_through_wsol_account_closed_in_tx() {
        // payer sells 500_000 raw tokens; 0.4 SOL arrives in a temp WSOL account (index 2) that is closed to the payer.
        let t = json!({
            "transaction": {"message": {"accountKeys": ["P", "ATA", "WSOLACC"]}},
            "meta": {"err": null, "fee": 5000,
                "preBalances": [1_000_000_000i64, 2_039_280, 0],
                "postBalances": [1_000_000_000i64 + 400_000_000 - 5000, 2_039_280, 0],
                "preTokenBalances": [tb(1, "P", "MINT", "500000")],
                "postTokenBalances": [tb(1, "P", "MINT", "0")]}
        });
        let (o, tr) = decode(&t);
        assert_eq!(o, Outcome::Trade);
        let tr = tr.unwrap();
        assert_eq!(tr.side, Side::Sell);
        assert_eq!(tr.sol_lamports, 400_000_000);
    }

    #[test]
    fn failed_and_round_trip_are_classified() {
        let failed = json!({"transaction": {"message": {"accountKeys": ["P"]}}, "meta": {"err": {"x": 1}}});
        assert_eq!(decode(&failed).0, Outcome::Failed);
        let arb = json!({
            "transaction": {"message": {"accountKeys": ["P"]}},
            "meta": {"err": null, "fee": 5000, "preBalances": [100], "postBalances": [200],
                "preTokenBalances": [tb(1, "P", "MINT", "10")], "postTokenBalances": [tb(1, "P", "MINT", "10")]}
        });
        assert_eq!(decode(&arb).0, Outcome::NoTraderTokenChange);
    }
}
