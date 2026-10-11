//! Program-agnostic pool legs from balance changes (getBlock encoding=json): every swap against a pool on ANY
//! venue (known or not, single venue or multi-hop route), read from the pool's side.
//!
//! A pool is an owner that is not a signer of the transaction. Per owner:
//! - vault leg: the owner's quote token account (WSOL > USDC > USDT priority) and one non-quote token account move
//!   in opposite directions. Pairing is by sign: exactly one quote account up + one token account down = a buy
//!   (the trader receives the token); exactly one quote down + one token up = a sell. More than one candidate of
//!   a direction is ambiguous and skipped (counted). A shared vault authority (Raydium AMM v4, Meteora DAMM v2,
//!   ...) used for a buy and a sell in the same route still pairs correctly by sign.
//! - native leg: no quote token account moved, the owner account itself (in the account keys) gains / loses
//!   lamports opposite to its single token account (bonding curves holding native SOL). Accepted only when the fee
//!   payer's clean trade (`balance_delta::decode`) is the same mint and size (within 2 %), so rent refunds and
//!   distributor transfers are not legs.
//! - price leg: a vault leg whose non-quote mint is a stablecoin against WSOL (SOL/USD reference price).
//!
//! Failed transactions have no legs. Quote amounts are raw units of the quote mint (lamports for SOL).

use crate::balance_delta::{self, Side, USDC, USDT, WSOL};
use rustc_hash::FxHashMap;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Quote {
    Sol,
    Usdc,
    Usdt,
}

impl Quote {
    pub fn of(mint: &str) -> Option<Quote> {
        match mint {
            WSOL => Some(Quote::Sol),
            USDC => Some(Quote::Usdc),
            USDT => Some(Quote::Usdt),
            _ => None,
        }
    }
    fn rank(self) -> u8 {
        match self {
            Quote::Sol => 0,
            Quote::Usdc => 1,
            Quote::Usdt => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LegKind {
    Vault,
    Native,
    /// SOL against a stablecoin (reference price), `mint` is the stablecoin.
    Price,
}

#[derive(Debug, Clone)]
pub struct Leg {
    pub kind: LegKind,
    /// Pool owner (vault authority / pool account / curve account).
    pub owner: String,
    pub mint: String,
    pub decimals: u8,
    pub quote: Quote,
    /// Trader side: Buy = the pool sold the token for quote.
    pub side: Side,
    /// Absolute quote amount (raw units of the quote mint).
    pub quote_amt: u128,
    pub token_amt: u128,
    /// Pool balances after the trade (quote vault or lamports; token vault).
    pub quote_post: u128,
    pub token_post: u128,
    /// Token vault account (pool key).
    pub token_vault: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LegSkip {
    Failed,
    /// An owner with more than one candidate account in one direction.
    Ambiguous,
    NativeMismatch,
    /// Paired accounts that are not a pool (a vault empty before or after the trade).
    NotPool,
}

struct Acc<'a> {
    owner: &'a str,
    mint: &'a str,
    decimals: u8,
    pre: i128,
    post: i128,
}

fn keys(t: &Value) -> Vec<&str> {
    let msg = &t["transaction"]["message"];
    let mut k: Vec<&str> = msg["accountKeys"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    for side in ["writable", "readonly"] {
        if let Some(a) = t["meta"]["loadedAddresses"][side].as_array() {
            k.extend(a.iter().filter_map(|x| x.as_str()));
        }
    }
    k
}

fn amount(b: &Value) -> i128 {
    b["uiTokenAmount"]["amount"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// All pool legs of one transaction, plus the skip reasons met (for coverage stats).
pub fn legs(t: &Value) -> (Vec<Leg>, Vec<LegSkip>) {
    let meta = &t["meta"];
    if !meta["err"].is_null() {
        return (Vec::new(), vec![LegSkip::Failed]);
    }
    let k = keys(t);
    let n_sig = t["transaction"]["message"]["header"]["numRequiredSignatures"].as_u64().unwrap_or(1) as usize;
    let signers: Vec<&str> = k.iter().take(n_sig.max(1)).copied().collect();
    let mut accs: FxHashMap<u64, Acc> = FxHashMap::default();
    for (side, key) in [(0, "preTokenBalances"), (1, "postTokenBalances")] {
        for b in meta[key].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            let Some(i) = b["accountIndex"].as_u64() else { continue };
            let (Some(owner), Some(mint)) = (b["owner"].as_str(), b["mint"].as_str()) else { continue };
            let e = accs.entry(i).or_insert(Acc { owner, mint, decimals: b["uiTokenAmount"]["decimals"].as_u64().unwrap_or(0) as u8, pre: 0, post: 0 });
            if side == 0 {
                e.pre = amount(b);
            } else {
                e.post = amount(b);
            }
        }
    }
    // owner -> moving accounts (index, account)
    let mut by_owner: FxHashMap<&str, Vec<(u64, &Acc)>> = FxHashMap::default();
    for (i, a) in &accs {
        if a.post != a.pre && !signers.contains(&a.owner) {
            by_owner.entry(a.owner).or_default().push((*i, a));
        }
    }
    let mut owners: Vec<&str> = by_owner.keys().copied().collect();
    owners.sort_unstable();
    let (mut out, mut skips) = (Vec::new(), Vec::new());
    let lamports = |idx: usize| -> Option<(i128, i128)> {
        let pre = meta["preBalances"].as_array()?.get(idx)?.as_i64()? as i128;
        let post = meta["postBalances"].as_array()?.get(idx)?.as_i64()? as i128;
        Some((pre, post))
    };
    for owner in owners {
        let mut v = by_owner.remove(owner).unwrap_or_default();
        v.sort_unstable_by_key(|(i, _)| *i);
        let quotes: Vec<&(u64, &Acc)> = v.iter().filter(|(_, a)| Quote::of(a.mint).is_some()).collect();
        let toks: Vec<&(u64, &Acc)> = v.iter().filter(|(_, a)| Quote::of(a.mint).is_none()).collect();
        if !quotes.is_empty() {
            // the pool's quote is the best-ranked quote mint it moved; a stablecoin moved with WSOL is a price leg
            let q = quotes.iter().map(|(_, a)| Quote::of(a.mint).expect("quote")).min_by_key(|q| q.rank()).expect("non-empty");
            let qacc: Vec<&(u64, &Acc)> = quotes.iter().copied().filter(|(_, a)| Quote::of(a.mint) == Some(q)).collect();
            let mut other: Vec<&(u64, &Acc)> = toks.clone();
            if q == Quote::Sol {
                other.extend(quotes.iter().copied().filter(|(_, a)| Quote::of(a.mint) != Some(Quote::Sol)));
            }
            for (qsign, side) in [(1i128, Side::Buy), (-1, Side::Sell)] {
                let qs: Vec<_> = qacc.iter().filter(|(_, a)| (a.post - a.pre).signum() == qsign).collect();
                let ts: Vec<_> = other.iter().filter(|(_, a)| (a.post - a.pre).signum() == -qsign).collect();
                match (qs.as_slice(), ts.as_slice()) {
                    ([], _) | (_, []) => {}
                    ([qa], [ta]) => {
                        let (ti, ta) = (ta.0, ta.1);
                        let qa = qa.1;
                        // a pool holds both balances before and after the trade; a wallet filled by a limit order /
                        // DCA keeper (non-signer owner, fresh WSOL account) does not
                        if qa.pre <= 0 || qa.post <= 0 || ta.pre <= 0 || ta.post <= 0 {
                            skips.push(LegSkip::NotPool);
                            continue;
                        }
                        let kind = if Quote::of(ta.mint).is_some() { LegKind::Price } else { LegKind::Vault };
                        out.push(Leg {
                            kind,
                            owner: owner.to_string(),
                            mint: ta.mint.to_string(),
                            decimals: ta.decimals,
                            quote: q,
                            side,
                            quote_amt: (qa.post - qa.pre).unsigned_abs(),
                            token_amt: (ta.post - ta.pre).unsigned_abs(),
                            quote_post: qa.post.max(0) as u128,
                            token_post: ta.post.max(0) as u128,
                            token_vault: k.get(ti as usize).copied().unwrap_or("").to_string(),
                        });
                    }
                    _ => skips.push(LegSkip::Ambiguous),
                }
            }
            continue;
        }
        // native leg: one token account, owner lamports move the other way
        let [(ti, ta)] = toks.as_slice() else {
            if toks.len() > 1 {
                skips.push(LegSkip::Ambiguous);
            }
            continue;
        };
        let Some(oi) = k.iter().position(|x| *x == owner) else { continue };
        let Some((lpre, lpost)) = lamports(oi) else { continue };
        let (dl, dt) = (lpost - lpre, ta.post - ta.pre);
        if dl == 0 || dl.signum() == dt.signum() {
            continue;
        }
        let side = if dl > 0 { Side::Buy } else { Side::Sell };
        let (_, tr) = balance_delta::decode(t);
        let ok = tr.is_some_and(|tr| {
            let near = |a: u128, b: u128| (a as f64 - b as f64).abs() <= 0.02 * (b as f64).max(1.0);
            tr.mint == ta.mint && tr.side == side && near(tr.token_raw, dt.unsigned_abs()) && dl.unsigned_abs() as f64 <= 1.02 * tr.sol_lamports as f64
                && dl.unsigned_abs() as f64 >= 0.5 * tr.sol_lamports as f64
        });
        if !ok {
            skips.push(LegSkip::NativeMismatch);
            continue;
        }
        out.push(Leg {
            kind: LegKind::Native,
            owner: owner.to_string(),
            mint: ta.mint.to_string(),
            decimals: ta.decimals,
            quote: Quote::Sol,
            side,
            quote_amt: dl.unsigned_abs(),
            token_amt: dt.unsigned_abs(),
            quote_post: lpost.max(0) as u128,
            token_post: ta.post.max(0) as u128,
            token_vault: k.get(*ti as usize).copied().unwrap_or("").to_string(),
        });
    }
    (out, skips)
}

/// Recording filter: the transaction trades a non-stable token against a pool (vault or native leg). SOL/stable
/// price legs alone do not qualify (reference pools are kept through the recorder's account list).
pub fn has_token_leg(t: &Value) -> bool {
    legs(t).0.iter().any(|l| l.kind != LegKind::Price)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TOK: &str = "TokenMintAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const TOK2: &str = "TokenMintBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

    fn tb(idx: u64, owner: &str, mint: &str, amount: i128) -> Value {
        json!({"accountIndex": idx, "owner": owner, "mint": mint, "uiTokenAmount": {"amount": amount.to_string(), "decimals": 6}})
    }

    fn tx(keys: &[&str], pre_tb: Vec<Value>, post_tb: Vec<Value>, pre_l: Vec<i64>, post_l: Vec<i64>) -> Value {
        json!({"transaction": {"message": {"accountKeys": keys, "header": {"numRequiredSignatures": 1}, "instructions": []}},
               "meta": {"err": null, "fee": 5000, "preBalances": pre_l, "postBalances": post_l,
                        "preTokenBalances": pre_tb, "postTokenBalances": post_tb}})
    }

    #[test]
    fn single_vault_buy() {
        // trader P buys TOK for 1 SOL from pool owned by AUTH (WSOL vault 3, token vault 2)
        let t = tx(&["P", "ATA", "TV", "SV", "AUTH"],
            vec![tb(1, "P", TOK, 0), tb(2, "AUTH", TOK, 1_000_000), tb(3, "AUTH", WSOL, 10_000_000_000)],
            vec![tb(1, "P", TOK, 1000), tb(2, "AUTH", TOK, 999_000), tb(3, "AUTH", WSOL, 11_000_000_000)],
            vec![5_000_000_000, 0, 0, 0, 0], vec![3_999_995_000, 0, 0, 0, 0]);
        let (l, s) = legs(&t);
        assert!(s.is_empty());
        assert_eq!(l.len(), 1);
        let l = &l[0];
        assert_eq!((l.kind, l.side, l.quote, l.mint.as_str()), (LegKind::Vault, Side::Buy, Quote::Sol, TOK));
        assert_eq!((l.quote_amt, l.token_amt, l.quote_post, l.token_post), (1_000_000_000, 1000, 11_000_000_000, 999_000));
        assert_eq!(l.token_vault, "TV");
    }

    #[test]
    fn multi_hop_through_shared_authority_pairs_by_sign() {
        // TOK -> SOL -> TOK2, both pools under one authority AUTH: a sell of TOK and a buy of TOK2
        let t = tx(&["P", "A1", "A2", "TV1", "SV1", "TV2", "SV2", "AUTH"],
            vec![tb(1, "P", TOK, 500), tb(2, "P", TOK2, 0), tb(3, "AUTH", TOK, 1000), tb(4, "AUTH", WSOL, 900), tb(5, "AUTH", TOK2, 2000), tb(6, "AUTH", WSOL, 800)],
            vec![tb(1, "P", TOK, 0), tb(2, "P", TOK2, 70), tb(3, "AUTH", TOK, 1500), tb(4, "AUTH", WSOL, 850), tb(5, "AUTH", TOK2, 1930), tb(6, "AUTH", WSOL, 850)],
            vec![10; 8], vec![10; 8]);
        let (mut l, s) = legs(&t);
        assert!(s.is_empty(), "{s:?}");
        l.sort_by_key(|x| x.mint.clone());
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].mint.as_str(), l[0].side, l[0].quote_amt, l[0].token_amt), (TOK, Side::Sell, 50, 500));
        assert_eq!((l[1].mint.as_str(), l[1].side, l[1].quote_amt, l[1].token_amt), (TOK2, Side::Buy, 50, 70));
    }

    #[test]
    fn stable_quote_and_price_legs() {
        // pool X: USDC/TOK (trader buys TOK with USDC); pool Y: SOL/USDC price leg
        let t = tx(&["P", "A", "TVX", "UVX", "X", "UVY", "SVY", "Y"],
            vec![tb(2, "X", TOK, 1000), tb(3, "X", USDC, 5000), tb(5, "Y", USDC, 100_000), tb(6, "Y", WSOL, 1_000_000_000)],
            vec![tb(2, "X", TOK, 900), tb(3, "X", USDC, 5500), tb(5, "Y", USDC, 99_500), tb(6, "Y", WSOL, 1_003_000_000)],
            vec![10; 8], vec![10; 8]);
        let (mut l, _) = legs(&t);
        l.sort_by_key(|x| x.owner.clone());
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].kind, l[0].quote, l[0].side, l[0].quote_amt), (LegKind::Vault, Quote::Usdc, Side::Buy, 500));
        // Y received SOL and paid USDC: from the trader's side a sell of USDC for SOL
        assert_eq!((l[1].kind, l[1].quote, l[1].mint.as_str(), l[1].side, l[1].quote_amt, l[1].token_amt), (LegKind::Price, Quote::Sol, USDC, Side::Buy, 3_000_000, 500));
    }

    #[test]
    fn fee_accounts_and_signer_accounts_are_not_legs() {
        // FEE owner receives WSOL only (protocol fee): no pairing token account -> no leg
        let t = tx(&["P", "W", "F"],
            vec![tb(1, "P", WSOL, 100), tb(2, "FEE", WSOL, 0)],
            vec![tb(1, "P", WSOL, 0), tb(2, "FEE", WSOL, 100)],
            vec![10; 3], vec![10; 3]);
        assert!(legs(&t).0.is_empty());
    }

    #[test]
    fn ambiguous_owner_is_skipped_not_guessed() {
        // AUTH sells two different tokens for SOL in one tx (two buys of two coins): one quote up but two tokens down
        let t = tx(&["P", "TV1", "TV2", "SV", "AUTH"],
            vec![tb(1, "AUTH", TOK, 1000), tb(2, "AUTH", TOK2, 1000), tb(3, "AUTH", WSOL, 100)],
            vec![tb(1, "AUTH", TOK, 900), tb(2, "AUTH", TOK2, 900), tb(3, "AUTH", WSOL, 300)],
            vec![10; 5], vec![10; 5]);
        let (l, s) = legs(&t);
        assert!(l.is_empty());
        assert_eq!(s, vec![LegSkip::Ambiguous]);
    }

    #[test]
    fn native_curve_leg_needs_matching_trader_trade() {
        // trader P pays 1 SOL to curve account C (native lamports), receives 1000 TOK from C's token account
        let mk = |curve_gain: i64| tx(&["P", "ATA", "CT", "C"],
            vec![tb(1, "P", TOK, 0), tb(2, "C", TOK, 10_000)],
            vec![tb(1, "P", TOK, 1000), tb(2, "C", TOK, 9000)],
            vec![5_000_000_000, 2_039_280, 2_039_280, 30_000_000_000],
            vec![3_999_995_000, 2_039_280, 2_039_280, 30_000_000_000 + curve_gain]);
        let (l, s) = legs(&mk(990_000_000));
        assert!(s.is_empty(), "{s:?}");
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].kind, l[0].side, l[0].quote_amt, l[0].token_amt, l[0].quote_post), (LegKind::Native, Side::Buy, 990_000_000, 1000, 30_990_000_000));
        // a curve gain far from the trader's payment is not accepted
        let (l, s) = legs(&mk(10_000));
        assert!(l.is_empty());
        assert_eq!(s, vec![LegSkip::NativeMismatch]);
    }

    #[test]
    fn limit_order_fill_to_a_wallet_is_not_a_pool() {
        // keeper K signs; user U (non-signer) sells TOK via a limit order: U's TOK account goes down, U's fresh WSOL
        // account goes from 0 up; the real pool AUTH is the only leg
        let t = tx(&["K", "UT", "UW", "TV", "SV"],
            vec![tb(1, "U", TOK, 100), tb(3, "AUTH", TOK, 5000), tb(4, "AUTH", WSOL, 9000)],
            vec![tb(1, "U", TOK, 0), tb(2, "U", WSOL, 40), tb(3, "AUTH", TOK, 5100), tb(4, "AUTH", WSOL, 8960)],
            vec![10; 5], vec![10; 5]);
        let (l, s) = legs(&t);
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].owner.as_str(), l[0].side, l[0].quote_amt), ("AUTH", Side::Sell, 40));
        assert_eq!(s, vec![LegSkip::NotPool]);
    }

    #[test]
    fn failed_tx_has_no_legs() {
        let mut t = tx(&["P"], vec![], vec![], vec![1], vec![1]);
        t["meta"]["err"] = json!({"InstructionError": [0, "Custom"]});
        assert_eq!(legs(&t).1, vec![LegSkip::Failed]);
    }
}
