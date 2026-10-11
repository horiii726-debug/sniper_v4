//! Generic SOL-quoted trades on the non-pump venues of `config/venues.toml` (Raydium AMM v4 / CPMM / CLMM /
//! LaunchLab, Meteora DLMM / DAMM v2 / DBC, Orca Whirlpool, Moonshot), from balance changes (getBlock json).
//!
//! Trade = `balance_delta::decode` (fee payer, one non-quote mint, SOL-quoted). Venue = the single non-pump venue
//! program invoked by the transaction (top-level or inner); transactions that also invoke pump.fun / PumpSwap are
//! left to the pump decoder (no double counting) and multi-venue routes are skipped (classified, not guessed).
//! Pool = the token vault: the non-trader token account of the traded mint whose change offsets the trader's.
//! Trade sizes = the vault deltas (pool side), not the trader's balances.
//! Liquidity = the SOL (WSOL) vault of the same owner after the trade (withdrawable quote; for concentrated
//! liquidity it is the whole pool's SOL, not only the active range).
//! Reserves for the CPMM venues (Raydium AMM v4, CPMM) are the vault balances after the trade (their ratio is the
//! marginal price); for the other venues the token reserve is set so that quote / token = the trade's execution
//! price (their vault ratio is not a price).

use crate::balance_delta::{self, Outcome, Side, Trade, WSOL};
use rustc_hash::FxHashMap;
use serde_json::Value;

/// Venue codes stored in events. 0 and 1 are the pump decoder's venues.
pub mod code {
    pub const PUMP_CURVE: u8 = 0;
    pub const PUMPSWAP: u8 = 1;
    pub const RAYDIUM_AMM_V4: u8 = 2;
    pub const RAYDIUM_CPMM: u8 = 3;
    pub const RAYDIUM_CLMM: u8 = 4;
    pub const RAYDIUM_LAUNCHLAB: u8 = 5;
    pub const METEORA_DLMM: u8 = 6;
    pub const METEORA_DAMM_V2: u8 = 7;
    pub const METEORA_DBC: u8 = 8;
    pub const ORCA_WHIRLPOOL: u8 = 9;
    pub const MOONSHOT: u8 = 10;
    /// Program-agnostic pool leg (`pool_legs`) on a pool not seen in a single-venue trade (unknown program or route).
    pub const OTHER_VENUE: u8 = 11;
    /// Program-agnostic pool leg quoted in USDC / USDT, converted to SOL with the on-chain SOL/USD price legs.
    pub const STABLE_QUOTED: u8 = 12;
    pub const UNKNOWN: u8 = 255;

    /// Code of a `config/venues.toml` venue name.
    pub fn of_name(name: &str) -> u8 {
        match name {
            "pumpfun_curve" => PUMP_CURVE,
            "pumpswap_amm" => PUMPSWAP,
            "raydium_amm_v4" => RAYDIUM_AMM_V4,
            "raydium_cpmm" => RAYDIUM_CPMM,
            "raydium_clmm" => RAYDIUM_CLMM,
            "raydium_launchlab" => RAYDIUM_LAUNCHLAB,
            "meteora_dlmm" => METEORA_DLMM,
            "meteora_damm_v2" => METEORA_DAMM_V2,
            "meteora_dbc" => METEORA_DBC,
            "orca_whirlpool" => ORCA_WHIRLPOOL,
            "moonshot" => MOONSHOT,
            _ => UNKNOWN,
        }
    }

    pub fn name(c: u8) -> &'static str {
        match c {
            PUMP_CURVE => "pumpfun_curve",
            PUMPSWAP => "pumpswap_amm",
            RAYDIUM_AMM_V4 => "raydium_amm_v4",
            RAYDIUM_CPMM => "raydium_cpmm",
            RAYDIUM_CLMM => "raydium_clmm",
            RAYDIUM_LAUNCHLAB => "raydium_launchlab",
            METEORA_DLMM => "meteora_dlmm",
            METEORA_DAMM_V2 => "meteora_damm_v2",
            METEORA_DBC => "meteora_dbc",
            ORCA_WHIRLPOOL => "orca_whirlpool",
            MOONSHOT => "moonshot",
            OTHER_VENUE => "other_venue",
            STABLE_QUOTED => "stable_quoted",
            _ => "unknown",
        }
    }

    /// Pool math validated against on-chain data (`pump-check`): fills and exits are simulated only here.
    pub fn validated(c: u8) -> bool {
        c == PUMP_CURVE || c == PUMPSWAP
    }

    /// Gate C18 (reports/v6/FILL_CHECK_w1.json): the simulator's fill math predicts real trades <= 0.2 SOL on these
    /// venues (median pred/actual 0.985-1.000, >= 95 % within +-10 %). Raydium CPMM / LaunchLab failed (vault balances
    /// include unclaimed protocol fees / curve state) and stable-quoted pools need a USDC route: not included.
    pub fn fill_validated(c: u8) -> bool {
        // OTHER_VENUE removed after the per-pool price / reserve consistency check (6.8 % of its pools inconsistent)
        matches!(c, PUMP_CURVE | PUMPSWAP | RAYDIUM_AMM_V4 | RAYDIUM_CLMM | METEORA_DLMM | METEORA_DAMM_V2 | METEORA_DBC | ORCA_WHIRLPOOL)
    }

    /// Constant-product pools whose vault balances are the reserves.
    pub fn cpmm_vaults(c: u8) -> bool {
        c == RAYDIUM_AMM_V4 || c == RAYDIUM_CPMM
    }
}

#[derive(Debug, Clone)]
pub struct PoolTrade {
    pub venue: u8,
    pub trade: Trade,
    /// Token vault account (pool key).
    pub pool: String,
    /// Reserves after the trade (lamports / raw tokens), see the module doc.
    pub quote_res: u128,
    pub token_res: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Skip {
    NoVenue,
    PumpTx,
    MultiVenue,
    NotTrade(Outcome),
    NoTokenVault,
    NoSolVault,
}

/// Non-pump venue programs (program id -> venue code).
pub struct VenueDecoder {
    programs: FxHashMap<String, u8>,
    pub skipped: FxHashMap<Skip, u64>,
    pub decoded: FxHashMap<u8, u64>,
}

fn keys(t: &Value) -> Vec<&str> {
    let msg = &t["transaction"]["message"];
    let meta = &t["meta"];
    let mut k: Vec<&str> = msg["accountKeys"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    for side in ["writable", "readonly"] {
        if let Some(a) = meta["loadedAddresses"][side].as_array() {
            k.extend(a.iter().filter_map(|x| x.as_str()));
        }
    }
    k
}

/// Program ids invoked by the transaction (top-level and inner instructions).
fn invoked<'a>(t: &Value, keys: &[&'a str]) -> Vec<&'a str> {
    let mut idx: Vec<usize> = Vec::new();
    for ix in t["transaction"]["message"]["instructions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        if let Some(i) = ix["programIdIndex"].as_u64() {
            idx.push(i as usize);
        }
    }
    for inner in t["meta"]["innerInstructions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        for ix in inner["instructions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            if let Some(i) = ix["programIdIndex"].as_u64() {
                idx.push(i as usize);
            }
        }
    }
    idx.sort_unstable();
    idx.dedup();
    idx.into_iter().filter_map(|i| keys.get(i).copied()).collect()
}

fn amount(b: &Value) -> i128 {
    b["uiTokenAmount"]["amount"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0)
}

impl VenueDecoder {
    /// `venues` = (name, program id) pairs from `config/venues.toml`; pump venues are ignored here.
    pub fn new<'a>(venues: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let programs = venues
            .into_iter()
            .map(|(n, p)| (p.to_string(), code::of_name(n)))
            .filter(|(_, c)| *c != code::PUMP_CURVE && *c != code::PUMPSWAP && *c != code::UNKNOWN)
            .collect();
        Self { programs, skipped: FxHashMap::default(), decoded: FxHashMap::default() }
    }

    fn skip(&mut self, s: Skip) -> Option<PoolTrade> {
        *self.skipped.entry(s).or_default() += 1;
        None
    }

    /// Decode one getBlock (encoding=json) transaction; None when it is not a clean single-venue SOL trade.
    pub fn decode(&mut self, t: &Value) -> Option<PoolTrade> {
        let k = keys(t);
        let inv = invoked(t, &k);
        if inv.iter().any(|p| *p == crate::pump::PUMP || *p == crate::pump::PUMP_AMM) {
            return self.skip(Skip::PumpTx);
        }
        let mut venues: Vec<u8> = inv.iter().filter_map(|p| self.programs.get(*p).copied()).collect();
        venues.sort_unstable();
        venues.dedup();
        let venue = match venues.as_slice() {
            [] => return self.skip(Skip::NoVenue),
            [v] => *v,
            _ => return self.skip(Skip::MultiVenue),
        };
        let (o, tr) = balance_delta::decode(t);
        let Some(tr) = tr else { return self.skip(Skip::NotTrade(o)) };
        let meta = &t["meta"];
        // per account index: (owner, mint, pre, post)
        let mut acc: FxHashMap<u64, (String, String, i128, i128)> = FxHashMap::default();
        for (side, key) in [(0, "preTokenBalances"), (1, "postTokenBalances")] {
            for b in meta[key].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
                let Some(i) = b["accountIndex"].as_u64() else { continue };
                let e = acc.entry(i).or_insert_with(|| (b["owner"].as_str().unwrap_or("").to_string(), b["mint"].as_str().unwrap_or("").to_string(), 0, 0));
                if side == 0 {
                    e.2 = amount(b);
                } else {
                    e.3 = amount(b);
                }
            }
        }
        let trader_tok: i128 = if tr.side == Side::Buy { tr.token_raw as i128 } else { -(tr.token_raw as i128) };
        // token vault: non-trader account of the mint moving opposite to the trader, closest in size
        let tv = acc
            .iter()
            .filter(|(_, a)| a.1 == tr.mint && a.0 != tr.trader && (a.3 - a.2).signum() == -trader_tok.signum())
            .min_by_key(|(i, a)| (((a.3 - a.2) + trader_tok).unsigned_abs(), **i));
        let Some((&tv_idx, tv_acc)) = tv else { return self.skip(Skip::NoTokenVault) };
        let owner = tv_acc.0.clone();
        // SOL vault: WSOL account of the same owner moving opposite to the trader's SOL (largest move)
        let sol_dir: i128 = if tr.side == Side::Buy { 1 } else { -1 };
        let sv = acc
            .iter()
            .filter(|(i, a)| **i != tv_idx && a.1 == WSOL && a.0 == owner && (a.3 - a.2).signum() == sol_dir)
            .max_by_key(|(i, a)| ((a.3 - a.2).unsigned_abs(), u64::MAX - **i));
        let Some((_, sv_acc)) = sv else { return self.skip(Skip::NoSolVault) };
        // Swap amounts from the POOL side (vault deltas): the trader's balances can also include tokens forwarded to
        // another wallet or SOL moved for other reasons in the same transaction (seen: 17.9 SOL "for" 0.33 tokens).
        let mut tr = tr;
        tr.token_raw = (tv_acc.3 - tv_acc.2).unsigned_abs();
        tr.sol_lamports = (sv_acc.3 - sv_acc.2).unsigned_abs().min(u64::MAX as u128) as u64;
        if tr.token_raw == 0 || tr.sol_lamports == 0 {
            return self.skip(Skip::NoSolVault);
        }
        let quote_res = sv_acc.3.max(0) as u128;
        let token_res = if code::cpmm_vaults(venue) {
            tv_acc.3.max(0) as u128
        } else if tr.sol_lamports > 0 {
            // quote / token = execution price (lamports per raw token)
            (quote_res as f64 * tr.token_raw as f64 / tr.sol_lamports as f64) as u128
        } else {
            0
        };
        let pool = k.get(tv_idx as usize).copied().unwrap_or("").to_string();
        *self.decoded.entry(venue).or_default() += 1;
        Some(PoolTrade { venue, trade: tr, pool, quote_res, token_res })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CPMM: &str = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C";
    const CLMM: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";

    fn dec() -> VenueDecoder {
        VenueDecoder::new([("raydium_cpmm", CPMM), ("raydium_clmm", CLMM), ("pumpfun_curve", crate::pump::PUMP)])
    }

    fn tb(idx: u64, owner: &str, mint: &str, amount: i128) -> Value {
        json!({"accountIndex": idx, "owner": owner, "mint": mint, "uiTokenAmount": {"amount": amount.to_string(), "decimals": 6}})
    }

    /// Trader P buys 1,000,000 raw tokens for 1 SOL from pool vaults owned by AUTH (token vault idx 2, WSOL vault idx 3).
    fn buy_tx(program: &str) -> Value {
        json!({
            "transaction": {"message": {"accountKeys": ["P", "ATA", "TVAULT", "SVAULT", program],
                "instructions": [{"programIdIndex": 4, "accounts": [], "data": ""}]}},
            "meta": {"err": null, "fee": 5000,
                "preBalances": [10_000_000_000i64, 2_039_280, 2_039_280, 50_000_000_000i64, 1],
                "postBalances": [10_000_000_000i64 - 1_000_000_000 - 5000, 2_039_280, 2_039_280, 51_000_000_000i64, 1],
                "innerInstructions": [],
                "preTokenBalances": [tb(1, "P", "MINT", 0), tb(2, "AUTH", "MINT", 100_000_000), tb(3, "AUTH", WSOL, 50_000_000_000)],
                "postTokenBalances": [tb(1, "P", "MINT", 1_000_000), tb(2, "AUTH", "MINT", 99_000_000), tb(3, "AUTH", WSOL, 51_000_000_000)]}
        })
    }

    #[test]
    fn cpmm_trade_uses_vault_reserves() {
        let mut d = dec();
        let p = d.decode(&buy_tx(CPMM)).expect("trade");
        assert_eq!((p.venue, p.pool.as_str()), (code::RAYDIUM_CPMM, "TVAULT"));
        assert_eq!(p.trade.side, Side::Buy);
        assert_eq!((p.quote_res, p.token_res), (51_000_000_000, 99_000_000));
    }

    /// The trader forwards most bought tokens to another wallet in the same tx: the trade size is the vault delta.
    #[test]
    fn trade_size_from_vault_not_trader_balance() {
        let mut d = dec();
        let mut t = buy_tx(CLMM);
        t["transaction"]["message"]["accountKeys"].as_array_mut().unwrap().push(json!("FWD"));
        t["meta"]["preBalances"].as_array_mut().unwrap().push(json!(0));
        t["meta"]["postBalances"].as_array_mut().unwrap().push(json!(0));
        t["meta"]["postTokenBalances"] = json!([tb(1, "P", "MINT", 1_000), tb(2, "AUTH", "MINT", 99_000_000), tb(3, "AUTH", WSOL, 51_000_000_000), tb(5, "Q", "MINT", 999_000)]);
        let p = d.decode(&t).expect("trade");
        assert_eq!((p.trade.token_raw, p.trade.sol_lamports), (1_000_000, 1_000_000_000));
        let price = p.quote_res as f64 / p.token_res as f64;
        assert!((price - 1_000.0).abs() < 1e-6, "{price}");
    }

    #[test]
    fn non_cpmm_reserves_follow_execution_price() {
        let mut d = dec();
        let p = d.decode(&buy_tx(CLMM)).expect("trade");
        assert_eq!(p.venue, code::RAYDIUM_CLMM);
        assert_eq!(p.quote_res, 51_000_000_000);
        let price = p.quote_res as f64 / p.token_res as f64;
        assert!((price - 1_000.0).abs() < 1e-6, "1e9 lamports / 1e6 raw = 1000 lamports per raw: {price}");
    }

    #[test]
    fn pump_and_multi_venue_and_unknown_are_skipped() {
        let mut d = dec();
        let mut t = buy_tx(CPMM);
        t["transaction"]["message"]["accountKeys"].as_array_mut().unwrap().push(json!(crate::pump::PUMP));
        t["meta"]["innerInstructions"] = json!([{"index": 0, "instructions": [{"programIdIndex": 5, "accounts": [], "data": ""}]}]);
        assert!(d.decode(&t).is_none());
        let mut t = buy_tx(CPMM);
        t["transaction"]["message"]["accountKeys"].as_array_mut().unwrap().push(json!(CLMM));
        t["meta"]["innerInstructions"] = json!([{"index": 0, "instructions": [{"programIdIndex": 5, "accounts": [], "data": ""}]}]);
        assert!(d.decode(&t).is_none());
        assert!(d.decode(&buy_tx("Other1111111111111111111111111111111111111")).is_none());
        assert_eq!(d.skipped[&Skip::PumpTx], 1);
        assert_eq!(d.skipped[&Skip::MultiVenue], 1);
        assert_eq!(d.skipped[&Skip::NoVenue], 1);
    }

    #[test]
    fn missing_sol_vault_is_classified() {
        let mut d = dec();
        let mut t = buy_tx(CPMM);
        t["meta"]["preTokenBalances"][2]["owner"] = json!("OTHER");
        t["meta"]["postTokenBalances"][2]["owner"] = json!("OTHER");
        assert!(d.decode(&t).is_none());
        assert_eq!(d.skipped[&Skip::NoSolVault], 1);
    }

    #[test]
    fn venue_codes_roundtrip() {
        for c in 0..=10u8 {
            assert_eq!(code::of_name(code::name(c)), c);
        }
        assert!(code::validated(code::PUMP_CURVE) && code::validated(code::PUMPSWAP) && !code::validated(code::RAYDIUM_CPMM));
    }
}
