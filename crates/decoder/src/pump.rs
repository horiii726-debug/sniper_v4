//! pump.fun bonding curve + PumpSwap market events from emit_cpi events (V4 §9 venue decoders).
//! Each event carries the pool state needed for exact pool math in simulation (§15.1):
//! curve = virtual reserves after the trade; PumpSwap = pool reserves (event gives pre-trade
//! reserves; post-trade reserves are derived from the event amounts).

use crate::idl::{Idl, EVENT_IX_TAG};
use crate::txview::TxView;
use serde_json::Value;

pub const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
pub const PUMP_AMM: &str = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA";
const WSOL: &str = "So11111111111111111111111111111111111111112";
const DEFAULT_KEY: &str = "11111111111111111111111111111111";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Venue {
    Curve,
    Amm,
}

#[derive(Debug, Clone)]
pub struct MarketEvent {
    pub venue: Venue,
    pub mint: String,
    pub pool: String,
    pub buy: bool,
    /// SOL the trader paid (buy) or received (sell), lamports, fees included.
    pub sol: u64,
    pub tokens: u64,
    pub user: String,
    pub creator: String,
    /// Quote (SOL) and token reserves AFTER this trade: virtual reserves for the curve, pool reserves for the AMM.
    pub quote_res: u128,
    pub token_res: u128,
    /// Curve only: SOL that entered/left the curve itself (fees excluded).
    pub curve_quote: u64,
    /// Curve only: real reserves after the trade (caps for fills).
    pub real_quote: u64,
    pub real_token: u64,
    /// Total fee charged on this venue, basis points.
    pub fee_bps: u64,
    /// Curve finished (all real tokens sold) after this trade.
    pub complete: bool,
    /// Curve in pump.fun Mayhem mode: an agent wallet trades fee-free and rewrites the virtual SOL reserve
    /// at each of its trades, so the virtual-reserve price is not backed by real SOL (out of scope for now).
    pub mayhem: bool,
    /// Raw decoded event (diagnostics).
    pub raw: Value,
}

pub struct PumpDecoder {
    pump: Idl,
    amm: Idl,
    pub skipped_non_sol_quote: u64,
    pub undecodable: u64,
    /// Decoded but a required field is missing/mistyped or the payload is truncated: quarantined.
    pub malformed: u64,
}

/// Optional numeric field (absent in older event versions): 0 when missing.
fn u(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or(0)
}

/// Required numeric field: None when missing or mistyped (the event is quarantined, never defaulted).
fn ur(v: &Value, k: &str) -> Option<u64> {
    v[k].as_u64()
}

/// Required non-empty string field.
fn sr(v: &Value, k: &str) -> Option<String> {
    v[k].as_str().filter(|x| !x.is_empty() && *x != DEFAULT_KEY).map(String::from)
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

impl PumpDecoder {
    pub fn new(pump_idl: &str, amm_idl: &str) -> Result<Self, String> {
        Ok(Self { pump: Idl::from_json(pump_idl)?, amm: Idl::from_json(amm_idl)?, skipped_non_sol_quote: 0, undecodable: 0, malformed: 0 })
    }

    /// Market events of one successful transaction, in execution order.
    pub fn decode(&mut self, tx: &TxView) -> Vec<MarketEvent> {
        let mut out = Vec::new();
        if tx.failed {
            return out;
        }
        for (i, e) in tx.exec.iter().enumerate() {
            if e.data.len() < 16 || e.data[..8] != EVENT_IX_TAG {
                continue;
            }
            let pid = tx.program_id(e);
            if pid == PUMP {
                match self.pump.decode_event(&e.data[8..]) {
                    Some((name, v)) if name == "TradeEvent" => {
                        let qm = s(&v, "quote_mint");
                        if !(qm.is_empty() || qm == DEFAULT_KEY || qm == WSOL) {
                            self.skipped_non_sol_quote += 1;
                            continue;
                        }
                        let parent_pool = tx.parent(i).and_then(|p| {
                            let pe = &tx.exec[p];
                            let def = self.pump.instruction(&pe.data)?;
                            let idx = def.accounts.iter().position(|a| a == "bonding_curve")?;
                            tx.account(pe, idx).map(String::from)
                        });
                        let ev = curve_event(&v, parent_pool);
                        match ev {
                            Some(e) => out.push(e),
                            None => self.malformed += 1,
                        }
                    }
                    Some(_) => {}
                    None => self.undecodable += 1,
                }
            } else if pid == PUMP_AMM {
                let Some((name, v)) = self.amm.decode_event(&e.data[8..]) else {
                    self.undecodable += 1;
                    continue;
                };
                let buy = match name.as_str() {
                    "BuyEvent" => true,
                    "SellEvent" => false,
                    _ => continue,
                };
                // Mints come from the swap instruction that emitted the event.
                let Some(p) = tx.parent(i) else {
                    self.undecodable += 1;
                    continue;
                };
                let pe = &tx.exec[p];
                let Some(def) = self.amm.instruction(&pe.data) else {
                    self.undecodable += 1;
                    continue;
                };
                let acc = |n: &str| def.accounts.iter().position(|a| a == n).and_then(|ix| tx.account(pe, ix)).map(String::from);
                let (Some(base), Some(quote)) = (acc("base_mint"), acc("quote_mint")) else {
                    self.undecodable += 1;
                    continue;
                };
                if quote != WSOL {
                    self.skipped_non_sol_quote += 1;
                    continue;
                }
                let pb = u(&v, "pool_base_token_reserves") as u128;
                // Pools may carry virtual quote liquidity: pricing uses real + virtual quote reserves.
                // The virtual quote reserve is signed (i128) and can be negative: never clamp it to 0.
                let Some(pq) = effective_quote(&v) else {
                    self.malformed += 1;
                    continue;
                };
                let (tokens, sol, token_res, quote_res) = if buy {
                    let base_out = u(&v, "base_amount_out");
                    let q_in = u(&v, "quote_amount_in_with_lp_fee");
                    // Total paid: `user_quote_amount_in` (exact-out buy) or `quote_amount_in` (exact-quote-in buy).
                    let paid = u(&v, "user_quote_amount_in").max(u(&v, "quote_amount_in"));
                    (base_out, paid, pb.saturating_sub(base_out as u128), pq + q_in as u128)
                } else {
                    let base_in = u(&v, "base_amount_in");
                    let q_out = u(&v, "quote_amount_out").saturating_sub(u(&v, "lp_fee"));
                    (base_in, u(&v, "user_quote_amount_out"), pb + base_in as u128, pq.saturating_sub(q_out as u128))
                };
                out.push(MarketEvent {
                    venue: Venue::Amm,
                    mint: base,
                    pool: s(&v, "pool"),
                    buy,
                    sol,
                    tokens,
                    user: s(&v, "user"),
                    creator: s(&v, "coin_creator"),
                    quote_res,
                    token_res,
                    curve_quote: 0,
                    real_quote: 0,
                    real_token: 0,
                    fee_bps: u(&v, "lp_fee_basis_points") + u(&v, "protocol_fee_basis_points") + u(&v, "coin_creator_fee_basis_points"),
                    complete: false,
                    mayhem: false,
                    raw: v.clone(),
                });
            }
        }
        out
    }
}

/// PumpSwap pricing quote reserve: real pool quote + signed virtual quote (absent in older events = 0).
/// None when the field is mistyped or the sum is not positive.
fn effective_quote(v: &Value) -> Option<u128> {
    let real = ur(v, "pool_quote_token_reserves")? as i128;
    let vq = match &v["virtual_quote_reserves"] {
        Value::Null => 0,
        Value::String(x) => x.parse::<i128>().ok()?,
        x => x.as_i64()? as i128,
    };
    u128::try_from(real.checked_add(vq)?).ok().filter(|q| *q > 0)
}

/// One curve TradeEvent; None (quarantine) when a required field is missing/mistyped or the payload is truncated.
fn curve_event(v: &Value, parent_pool: Option<String>) -> Option<MarketEvent> {
    if v.get("_truncated").is_some() {
        return None;
    }
    let real_token = ur(v, "real_token_reserves")?;
    // sol_amount is what entered/left the curve; every fee is charged on top (buy) /
    // deducted (sell): protocol, creator, cashback, buyback, holder rewards.
    let fees = u(v, "fee")
        + u(v, "creator_fee")
        + u(v, "cashback")
        + u(v, "buyback_fee")
        + u(v, "holder_rewards");
    let is_buy = v["is_buy"].as_bool()?;
    let net = ur(v, "sol_amount")?;
    let tokens = ur(v, "token_amount")?;
    let (qr, tr) = (ur(v, "virtual_sol_reserves")?, ur(v, "virtual_token_reserves")?);
    if tokens == 0 || qr == 0 || tr == 0 {
        return None;
    }
    Some(MarketEvent {
        venue: Venue::Curve,
        mint: sr(v, "mint")?,
        pool: parent_pool.unwrap_or_default(),
        buy: is_buy,
        sol: if is_buy { net + fees } else { net.saturating_sub(fees) },
        curve_quote: net,
        tokens,
        user: sr(v, "user")?,
        creator: s(v, "creator"),
        quote_res: qr as u128,
        token_res: tr as u128,
        real_quote: ur(v, "real_sol_reserves")?,
        real_token,
        fee_bps: u(v, "fee_basis_points")
            + u(v, "creator_fee_basis_points")
            + u(v, "cashback_fee_basis_points")
            + u(v, "buyback_fee_basis_points")
            + u(v, "holder_rewards_bps"),
        complete: real_token == 0,
        mayhem: v["mayhem_mode"].as_bool().unwrap_or(false),
        raw: v.clone(),
    })
}

/// Tokens received for spending `paid` lamports in total (fee taken from the input, exact-quote-in),
/// constant product on (quote_res, token_res). Curve: virtual reserves; PumpSwap: real + virtual quote.
pub fn buy_tokens_out(quote_res: u128, token_res: u128, paid: u64, fee_bps: u64) -> u128 {
    let q = paid as u128 * 10_000 / (10_000 + fee_bps as u128);
    if quote_res + q == 0 {
        return 0;
    }
    token_res - (quote_res * token_res).div_ceil(quote_res + q)
}

/// Lamports received for selling `tokens` (fee deducted from the output).
pub fn sell_quote_out(quote_res: u128, token_res: u128, tokens: u64, fee_bps: u64) -> u128 {
    let gross = quote_res * tokens as u128 / (token_res + tokens as u128);
    gross - (gross * fee_bps as u128).div_ceil(10_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn trade() -> Value {
        json!({"mint": "M1", "user": "U1", "creator": "C1", "is_buy": true, "sol_amount": 1_000_000_000u64,
               "token_amount": 30_000_000_000_000u64, "virtual_sol_reserves": 31_000_000_000u64,
               "virtual_token_reserves": 1_043_000_000_000_000u64, "real_sol_reserves": 1_000_000_000u64,
               "real_token_reserves": 763_000_000_000_000u64, "fee": 9_500_000u64, "fee_basis_points": 95u64,
               "creator_fee": 3_000_000u64, "creator_fee_basis_points": 30u64, "mayhem_mode": false})
    }

    #[test]
    fn curve_event_valid() {
        let e = curve_event(&trade(), None).unwrap();
        assert!(e.buy && !e.mayhem && !e.complete);
        assert_eq!(e.sol, 1_012_500_000);
        assert_eq!(e.fee_bps, 125);
    }

    #[test]
    fn missing_required_field_is_quarantined_not_defaulted() {
        for k in ["is_buy", "sol_amount", "token_amount", "virtual_sol_reserves", "virtual_token_reserves", "real_sol_reserves", "real_token_reserves", "mint", "user"] {
            let mut v = trade();
            v.as_object_mut().unwrap().remove(k);
            assert!(curve_event(&v, None).is_none(), "missing {k} must quarantine");
            let mut v = trade();
            v[k] = json!("bad");
            if k != "mint" && k != "user" {
                assert!(curve_event(&v, None).is_none(), "mistyped {k} must quarantine");
            }
        }
        let mut v = trade();
        v["mint"] = json!("");
        assert!(curve_event(&v, None).is_none());
        let mut v = trade();
        v["_truncated"] = json!(true);
        assert!(curve_event(&v, None).is_none());
        let mut v = trade();
        v["token_amount"] = json!(0);
        assert!(curve_event(&v, None).is_none());
    }

    #[test]
    fn extra_fees_and_mayhem_flag() {
        let mut v = trade();
        v["cashback"] = json!(1_000u64);
        v["buyback_fee"] = json!(2_000u64);
        v["holder_rewards"] = json!(3_000u64);
        v["mayhem_mode"] = json!(true);
        let e = curve_event(&v, None).unwrap();
        assert_eq!(e.sol, 1_012_506_000);
        assert!(e.mayhem);
        v["is_buy"] = json!(false);
        assert_eq!(curve_event(&v, None).unwrap().sol, 1_000_000_000 - 12_506_000);
    }
}

#[cfg(test)]
mod amm_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn negative_virtual_quote_is_subtracted_not_clamped() {
        let v = json!({"pool_quote_token_reserves": 175_445_274_980u64, "virtual_quote_reserves": "-9019950071"});
        assert_eq!(effective_quote(&v), Some(175_445_274_980 - 9_019_950_071));
        let v = json!({"pool_quote_token_reserves": 100u64, "virtual_quote_reserves": "50"});
        assert_eq!(effective_quote(&v), Some(150));
        let v = json!({"pool_quote_token_reserves": 100u64});
        assert_eq!(effective_quote(&v), Some(100));
        let v = json!({"pool_quote_token_reserves": 100u64, "virtual_quote_reserves": "-100"});
        assert_eq!(effective_quote(&v), None);
        let v = json!({"pool_quote_token_reserves": 100u64, "virtual_quote_reserves": "x"});
        assert_eq!(effective_quote(&v), None);
    }
}
