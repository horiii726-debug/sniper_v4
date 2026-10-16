//! pump.fun bonding curve + PumpSwap market events from emit_cpi events (V4 §9 venue decoders).
//! Each event carries the pool state needed for exact pool math in simulation (§15.1):
//! curve = virtual reserves after the trade; PumpSwap = pool reserves (event gives pre-trade
//! reserves; post-trade reserves are derived from the event amounts).

use crate::idl::{Idl, EVENT_IX_TAG};
use crate::txview::TxView;
use serde_json::Value;

pub const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
pub const PUMP_AMM: &str = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA";
/// pump.fun initial real token reserves (raw, 6 decimals): 793.1M tokens (curve progress = 1 - real / this).
pub const CURVE_INITIAL_REAL_TOKEN: f64 = 793_100_000_000_000.0;
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
    /// Raw decoded event (diagnostics).
    pub raw: Value,
}

pub struct PumpDecoder {
    pump: Idl,
    amm: Idl,
    pub skipped_non_sol_quote: u64,
    pub undecodable: u64,
    pub skipped_mayhem: u64,
}

fn u(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or(0)
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

impl PumpDecoder {
    pub fn new(pump_idl: &str, amm_idl: &str) -> Result<Self, String> {
        Ok(Self { pump: Idl::from_json(pump_idl)?, amm: Idl::from_json(amm_idl)?, skipped_non_sol_quote: 0, undecodable: 0, skipped_mayhem: 0 })
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
                        // Mayhem mode: an agent wallet trades fee-free and rewrites the virtual SOL reserve at each of
                        // its trades, so the curve price is not backed by real SOL. Out of scope: skipped and counted.
                        if v["mayhem_mode"].as_bool() == Some(true) {
                            self.skipped_mayhem += 1;
                            continue;
                        }
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
                        let real_token = u(&v, "real_token_reserves");
                        // sol_amount is what entered/left the curve; fees are charged on top (buy) / deducted (sell).
                        let fees = u(&v, "fee") + u(&v, "creator_fee");
                        let is_buy = v["is_buy"].as_bool().unwrap_or(false);
                        let net = u(&v, "sol_amount");
                        out.push(MarketEvent {
                            venue: Venue::Curve,
                            mint: s(&v, "mint"),
                            pool: parent_pool.unwrap_or_default(),
                            buy: is_buy,
                            sol: if is_buy { net + fees } else { net.saturating_sub(fees) },
                            curve_quote: net,
                            tokens: u(&v, "token_amount"),
                            user: s(&v, "user"),
                            creator: s(&v, "creator"),
                            quote_res: u(&v, "virtual_sol_reserves") as u128,
                            token_res: u(&v, "virtual_token_reserves") as u128,
                            real_quote: u(&v, "real_sol_reserves"),
                            real_token,
                            fee_bps: u(&v, "fee_basis_points") + u(&v, "creator_fee_basis_points"),
                            complete: real_token == 0,
                            raw: v.clone(),
                        });
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
                // The virtual quote reserve is signed (i128) and is often negative: never clamp it to 0.
                let Some(pq) = effective_quote(&v) else {
                    self.undecodable += 1;
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
                    raw: v.clone(),
                });
            }
        }
        out
    }
}

/// Coin creation seen in a transaction: pump.fun `CreateEvent` (curve launch) or PumpSwap `CreatePoolEvent`
/// (pool launch). `price` = quote/token reserves at creation, the same unit as the marginal pool price.
#[derive(Debug, Clone, PartialEq)]
pub struct Creation {
    pub venue: Venue,
    pub mint: String,
    pub time: i64,
    pub price: f64,
    /// A PumpSwap pool created by a curve migration (pump `CompletePumpAmmMigrationEvent` for the same mint in the
    /// same transaction): NOT a coin birth.
    pub migration: bool,
}

impl PumpDecoder {
    /// Creation events of one successful transaction (SOL-quoted only).
    pub fn decode_creations(&mut self, tx: &TxView) -> Vec<Creation> {
        let mut out = Vec::new();
        if tx.failed {
            return out;
        }
        let mut migrated: Vec<String> = Vec::new();
        for e in &tx.exec {
            if e.data.len() >= 16 && e.data[..8] == EVENT_IX_TAG && tx.program_id(e) == PUMP {
                if let Some((name, v)) = self.pump.decode_event(&e.data[8..]) {
                    if name == "CompletePumpAmmMigrationEvent" {
                        migrated.push(s(&v, "mint"));
                    }
                }
            }
        }
        for e in &tx.exec {
            if e.data.len() < 16 || e.data[..8] != EVENT_IX_TAG {
                continue;
            }
            let pid = tx.program_id(e);
            let (venue, idl) = if pid == PUMP {
                (Venue::Curve, &self.pump)
            } else if pid == PUMP_AMM {
                (Venue::Amm, &self.amm)
            } else {
                continue;
            };
            let Some((name, v)) = idl.decode_event(&e.data[8..]) else { continue };
            let (mint, quote, q, t) = match (venue, name.as_str()) {
                (Venue::Curve, "CreateEvent") => {
                    (s(&v, "mint"), s(&v, "quote_mint"), u(&v, "virtual_sol_reserves") as f64, u(&v, "virtual_token_reserves") as f64)
                }
                (Venue::Amm, "CreatePoolEvent") => {
                    (s(&v, "base_mint"), s(&v, "quote_mint"), u(&v, "pool_quote_amount") as f64, u(&v, "pool_base_amount") as f64)
                }
                _ => continue,
            };
            if !(quote.is_empty() || quote == DEFAULT_KEY || quote == WSOL) || t <= 0.0 {
                continue;
            }
            let migration = venue == Venue::Amm && migrated.contains(&mint);
            out.push(Creation { venue, mint, time: v["timestamp"].as_i64().unwrap_or(0), price: q / t, migration });
        }
        out
    }
}

/// PumpSwap pricing quote reserve: real pool quote + signed virtual quote (absent in older events = 0).
/// None when the field is mistyped or the sum is not positive.
fn effective_quote(v: &Value) -> Option<u128> {
    let real = v["pool_quote_token_reserves"].as_u64()? as i128;
    let vq = match &v["virtual_quote_reserves"] {
        Value::Null => 0,
        Value::String(x) => x.parse::<i128>().ok()?,
        x => x.as_i64()? as i128,
    };
    u128::try_from(real.checked_add(vq)?).ok().filter(|q| *q > 0)
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
mod creation_tests {
    use super::*;
    use crate::txview::Exec;

    /// Borsh-encode an IDL event (fields from `vals`, zero / empty defaults), prefixed by the emit_cpi tag.
    fn encode(idl_json: &str, name: &str, vals: &[(&str, Value)]) -> Vec<u8> {
        let idl: Value = serde_json::from_str(idl_json).unwrap();
        let ev = idl["events"].as_array().unwrap().iter().find(|e| e["name"] == name).unwrap();
        let mut out = EVENT_IX_TAG.to_vec();
        out.extend(ev["discriminator"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8));
        let ty = idl["types"].as_array().unwrap().iter().find(|t| t["name"] == name).unwrap();
        for f in ty["type"]["fields"].as_array().unwrap() {
            let n = f["name"].as_str().unwrap();
            let v = vals.iter().find(|x| x.0 == n).map(|x| x.1.clone()).unwrap_or(Value::Null);
            match f["type"].as_str().unwrap() {
                "pubkey" | "publicKey" => {
                    let k = v.as_str().unwrap_or(DEFAULT_KEY);
                    out.extend(bs58::decode(k).into_vec().unwrap());
                }
                "string" => {
                    let s = v.as_str().unwrap_or("");
                    out.extend((s.len() as u32).to_le_bytes());
                    out.extend(s.as_bytes());
                }
                "u64" => out.extend(v.as_u64().unwrap_or(0).to_le_bytes()),
                "i64" => out.extend(v.as_i64().unwrap_or(0).to_le_bytes()),
                "u16" => out.extend((v.as_u64().unwrap_or(0) as u16).to_le_bytes()),
                "u8" | "bool" => out.push(v.as_u64().unwrap_or(0) as u8),
                t => panic!("type {t} not covered by the test encoder"),
            }
        }
        out
    }

    const MINT: &str = "So11111111111111111111111111111111111111112";

    fn tx(events: Vec<(usize, Vec<u8>)>) -> TxView {
        let exec = events.into_iter().map(|(program, data)| Exec { program, accounts: vec![], data, stack: 2, top: 0 }).collect();
        TxView { keys: vec![PUMP.to_string(), PUMP_AMM.to_string()], exec, failed: false, fee_payer: String::new() }
    }

    #[test]
    fn creations_flag_pump_amm_migration() {
        let (pi, ai) = (include_str!("../../../config/idl/pump.json"), include_str!("../../../config/idl/pump_amm.json"));
        let mut d = PumpDecoder::new(pi, ai).unwrap();
        let mint = "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R";
        let create = encode(pi, "CreateEvent", &[("mint", mint.into()), ("timestamp", 100.into()), ("virtual_sol_reserves", 30_000_000_000u64.into()),
            ("virtual_token_reserves", 1_073_000_000_000_000u64.into())]);
        let pool = encode(ai, "CreatePoolEvent", &[("base_mint", mint.into()), ("quote_mint", MINT.into()), ("timestamp", 200.into()),
            ("pool_base_amount", 200_000_000_000_000u64.into()), ("pool_quote_amount", 85_000_000_000u64.into())]);
        let migr = encode(pi, "CompletePumpAmmMigrationEvent", &[("mint", mint.into()), ("timestamp", 200.into())]);
        let a = d.decode_creations(&tx(vec![(0, create)]));
        assert_eq!(a.len(), 1);
        assert!(a[0].venue == Venue::Curve && !a[0].migration && a[0].mint == mint && a[0].time == 100);
        let b = d.decode_creations(&tx(vec![(0, migr), (1, pool.clone())]));
        assert_eq!(b.len(), 1);
        assert!(b[0].venue == Venue::Amm && b[0].migration, "{b:?}");
        let c = d.decode_creations(&tx(vec![(1, pool)]));
        assert!(c.len() == 1 && !c[0].migration, "pool launch without migration is a birth");
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
        assert_eq!(effective_quote(&json!({"pool_quote_token_reserves": 100u64})), Some(100));
        assert_eq!(effective_quote(&json!({"pool_quote_token_reserves": 100u64, "virtual_quote_reserves": "-100"})), None);
    }
}
