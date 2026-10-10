//! Verification of the pump.fun / PumpSwap decoder (G1A for these venues) and of the pool math used in
//! simulation (§15.1):
//! 1. side + token amount vs the balance-delta reference decoder, on transactions with exactly one
//!    venue event whose trader is the fee payer;
//! 2. pool-math prediction: from the reserves after event k, predict the token amount of the next
//!    trade on the same pool (k+1) from its SOL amount, and compare with what the chain did.

use anyhow::Result;
use decoder::balance_delta::{self, Outcome, Side};
use decoder::pump::{MarketEvent, PumpDecoder, Venue};
use decoder::txview::TxView;
use rustc_hash::FxHashMap;
use std::path::Path;

pub fn load_decoder() -> Result<PumpDecoder> {
    let p = std::fs::read_to_string("config/idl/pump.json")?;
    let a = std::fs::read_to_string("config/idl/pump_amm.json")?;
    PumpDecoder::new(&p, &a).map_err(|e| anyhow::anyhow!(e))
}

pub fn run(rec: &Path, dump: Option<&Path>, max_blocks: usize) -> Result<()> {
    let mut dumpf = match dump { Some(p) => Some(std::io::BufWriter::new(std::fs::File::create(p)?)), None => None };
    let mut blocks = 0usize;
    let mut dec = load_decoder()?;
    let (mut n_single, mut side_ok, mut amt_ok) = (0u64, 0u64, 0u64);
    let mut by_venue: FxHashMap<&'static str, (u64, u64, u64)> = FxHashMap::default();
    let mut mismatches = Vec::new();
    let mut last: FxHashMap<String, MarketEvent> = FxHashMap::default();
    let mut pred: FxHashMap<&'static str, (u64, u64, Vec<f64>)> = FxHashMap::default();
    let mut events_total = 0u64;
    for m in recorder::Reader::open(rec)? {
        let m = m?;
        if m.source != events::Source::Rpc {
            continue;
        }
        let v: serde_json::Value = serde_json::from_slice(&m.payload)?;
        let r = &v["result"];
        if r.is_null() {
            continue;
        }
        blocks += 1;
        if max_blocks > 0 && blocks > max_blocks {
            break;
        }
        for t in r["transactions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            let Some(tx) = TxView::parse(t) else { continue };
            let evs = dec.decode(&tx);
            events_total += evs.len() as u64;
            if let Some(f) = dumpf.as_mut() {
                use std::io::Write;
                for e in &evs {
                    writeln!(f, "{}", serde_json::json!({"venue": format!("{:?}", e.venue), "mint": e.mint, "pool": e.pool, "buy": e.buy,
                        "sol": e.sol, "tokens": e.tokens, "user": e.user, "payer": tx.fee_payer, "raw": e.raw}))?;
                }
            }
            // Pool-math prediction against the previous state of the same pool.
            for e in &evs {
                let key = format!("{:?}:{}", e.venue, if e.venue == Venue::Curve { &e.mint } else { &e.pool });
                if let Some(prev) = last.get(&key) {
                    let tag = if e.venue == Venue::Curve { "curve" } else { "amm" };
                    let ent = pred.entry(tag).or_insert((0, 0, Vec::new()));
                    // Curve buys: tokens from the net SOL into the curve (fee 0).
                    // PumpSwap sells: SOL out (after fees) from the tokens sold.
                    let check = match (e.venue, e.buy) {
                        (Venue::Curve, true) if !prev.complete && e.curve_quote > 0 => {
                            Some((decoder::pump::buy_tokens_out(prev.quote_res, prev.token_res, e.curve_quote, 0) as f64, e.tokens as f64))
                        }
                        (Venue::Amm, false) if e.tokens > 0 => {
                            Some((decoder::pump::sell_quote_out(prev.quote_res, prev.token_res, e.tokens, e.fee_bps) as f64, e.sol as f64))
                        }
                        _ => None,
                    };
                    if let Some((p, actual)) = check {
                        let err = (p - actual).abs() / actual.max(1.0);
                        ent.0 += 1;
                        if err <= 0.001 {
                            ent.1 += 1;
                        }
                        ent.2.push(err);
                    }
                }
                last.insert(key, e.clone());
            }
            if evs.len() != 1 {
                continue;
            }
            let e = &evs[0];
            if e.user != tx.fee_payer {
                continue;
            }
            let (o, rt) = balance_delta::decode(t);
            if o != Outcome::Trade {
                continue;
            }
            let rt = rt.expect("trade");
            if rt.mint != e.mint {
                continue;
            }
            n_single += 1;
            let tag = if e.venue == Venue::Curve { "pumpfun_curve" } else { "pumpswap_amm" };
            let ent = by_venue.entry(tag).or_insert((0, 0, 0));
            ent.0 += 1;
            let sok = (rt.side == Side::Buy) == e.buy;
            let aok = rt.token_raw == e.tokens as u128;
            if sok {
                side_ok += 1;
                ent.1 += 1;
            }
            if aok {
                amt_ok += 1;
                ent.2 += 1;
            }
            if (!sok || !aok) && mismatches.len() < 10 {
                mismatches.push(serde_json::json!({
                    "sig": t["transaction"]["signatures"][0], "venue": tag, "event_buy": e.buy, "event_tokens": e.tokens,
                    "ref_side": format!("{:?}", rt.side), "ref_tokens": rt.token_raw.to_string(),
                }));
            }
        }
    }
    let pct = |a: u64, b: u64| if b == 0 { f64::NAN } else { a as f64 * 100.0 / b as f64 };
    let pred_out: serde_json::Value = pred
        .iter_mut()
        .map(|(k, (n, ok, errs))| {
            errs.sort_by(|a, b| a.total_cmp(b));
            let q = |p: f64| errs.get(((errs.len() as f64 - 1.0) * p) as usize).copied().unwrap_or(f64::NAN);
            (k.to_string(), serde_json::json!({ "checked": n, "within_0.1pct": ok, "pct": pct(*ok, *n),
                "median_rel_err": q(0.5), "p90_rel_err": q(0.9), "p99_rel_err": q(0.99) }))
        })
        .collect::<serde_json::Map<_, _>>()
        .into();
    let venues: serde_json::Value = by_venue
        .iter()
        .map(|(k, (n, s, a))| (k.to_string(), serde_json::json!({ "compared": n, "side_match_pct": pct(*s, *n), "amount_match_pct": pct(*a, *n) })))
        .collect::<serde_json::Map<_, _>>()
        .into();
    let out = serde_json::json!({
        "events_decoded": events_total, "undecodable_events": dec.undecodable, "skipped_non_sol_quote": dec.skipped_non_sol_quote,
        "vs_reference": { "compared": n_single, "side_match_pct": pct(side_ok, n_single), "amount_match_pct": pct(amt_ok, n_single), "per_venue": venues },
        "pool_math_next_buy_prediction": pred_out, "mismatch_examples": mismatches,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
