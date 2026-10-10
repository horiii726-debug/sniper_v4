//! Coin census over a backfill recording (Stage 1 research tool, no trading logic):
//! decodes every venue transaction with the balance-delta decoder, then reports per coin the price path
//! (start / peak / low / end multiples), trader activity and buy/sell sizes.
//!
//! Price path: only trades of at least `MIN_PRICE_SOL` are priced; the series is a trailing median of
//! 3 trades (one outlier trade cannot make a peak); start = median of the first 3 priced trades.
//! Early-activity features use only the first `EARLY_SECS` after the coin's first trade in the window,
//! and the outcome is measured strictly after that point (no look-ahead between features and outcome).

use anyhow::{anyhow, Result};
use decoder::balance_delta::{decode, Outcome, Side};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;
use std::io::Write;
use std::path::Path;

const MIN_PRICE_SOL: f64 = 0.01;
const MIN_PRICED_TRADES: usize = 10;
const EARLY_SECS: i64 = 120;

struct T {
    slot: u64,
    time: i64,
    trader: u32,
    buy: bool,
    sol: f64,
    price: f64,
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

#[derive(Serialize, Default, Clone)]
struct Coin {
    mint: String,
    trades: usize,
    buys: usize,
    sells: usize,
    traders: usize,
    buyers: usize,
    sellers: usize,
    buy_sol: f64,
    sell_sol: f64,
    net_flow_sol: f64,
    median_buy_sol: f64,
    median_sell_sol: f64,
    max_buy_sol: f64,
    first_time: i64,
    last_time: i64,
    priced_trades: usize,
    start_price: f64,
    peak_mult: f64,
    peak_after_s: i64,
    low_mult: f64,
    end_mult: f64,
    /// Share of buyers whose first trade in the whole window was this coin.
    new_wallet_buyer_share: f64,
    // Early window (first EARLY_SECS) features and the outcome after it.
    early_buyers: usize,
    early_sellers: usize,
    early_buy_sol: f64,
    early_sell_sol: f64,
    early_median_buy_sol: f64,
    early_trades: usize,
    early_max_buy_sol: f64,
    /// Share of early buys below MIN_PRICE_SOL (dust used to inflate maker counts).
    early_dust_buy_share: f64,
    /// Share of early buyers with more than one buy.
    early_repeat_buyer_share: f64,
    /// Share of early buyers whose first trade in the window was this coin.
    early_new_wallet_share: f64,
    /// Price at the end of the early window / start price.
    early_price_mult: f64,
    /// First trade more than 60 s after the window start (new or newly active coin).
    newborn_in_window: bool,
    after_early_peak_mult: f64,
}

#[derive(Serialize)]
struct Bucket {
    label: &'static str,
    coins: usize,
}

pub fn run(rec: &Path, out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    let mut outcomes: FxHashMap<&'static str, u64> = FxHashMap::default();
    let mut trader_ids: FxHashMap<String, u32> = FxHashMap::default();
    let mut by_mint: FxHashMap<String, Vec<T>> = FxHashMap::default();
    let mut first_coin_of_trader: FxHashMap<u32, String> = FxHashMap::default();
    let (mut blocks, mut min_slot, mut max_slot, mut min_t, mut max_t) = (0u64, u64::MAX, 0u64, i64::MAX, i64::MIN);
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
        let slot = v["params"]["slot"].as_u64().ok_or_else(|| anyhow!("no slot"))?;
        let time = r["blockTime"].as_i64().unwrap_or(0);
        blocks += 1;
        min_slot = min_slot.min(slot);
        max_slot = max_slot.max(slot);
        min_t = min_t.min(time);
        max_t = max_t.max(time);
        for t in r["transactions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            let (o, tr) = decode(t);
            let key = match o {
                Outcome::Trade => "trade",
                Outcome::Failed => "failed",
                Outcome::NoTraderTokenChange => "no_trader_token_change",
                Outcome::MultiMint => "multi_mint",
                Outcome::StableQuote => "stable_quote",
                Outcome::NotASwap => "not_a_swap",
                Outcome::Malformed => "malformed",
            };
            *outcomes.entry(key).or_default() += 1;
            let Some(tr) = tr else { continue };
            let n = trader_ids.len() as u32;
            let id = *trader_ids.entry(tr.trader.clone()).or_insert(n);
            first_coin_of_trader.entry(id).or_insert_with(|| tr.mint.clone());
            by_mint.entry(tr.mint.clone()).or_default().push(T {
                slot,
                time,
                trader: id,
                buy: tr.side == Side::Buy,
                sol: tr.sol_lamports as f64 / 1e9,
                price: tr.price_sol(),
            });
        }
    }
    let window_start = min_t;
    let mut coins: Vec<Coin> = Vec::with_capacity(by_mint.len());
    for (mint, mut ts) in by_mint {
        ts.sort_by_key(|t| (t.slot, t.time));
        let mut c = Coin { mint: mint.clone(), trades: ts.len(), ..Default::default() };
        let (mut buyers, mut sellers, mut traders) = (FxHashSet::default(), FxHashSet::default(), FxHashSet::default());
        let (mut bsz, mut ssz) = (Vec::new(), Vec::new());
        c.first_time = ts[0].time;
        c.last_time = ts[ts.len() - 1].time;
        let early_end = c.first_time + EARLY_SECS;
        let (mut eb, mut es) = (FxHashSet::default(), FxHashSet::default());
        let mut ebsz = Vec::new();
        let mut ebuys_per: FxHashMap<u32, u32> = FxHashMap::default();
        for t in &ts {
            traders.insert(t.trader);
            if t.buy {
                c.buys += 1;
                c.buy_sol += t.sol;
                buyers.insert(t.trader);
                bsz.push(t.sol);
                c.max_buy_sol = c.max_buy_sol.max(t.sol);
            } else {
                c.sells += 1;
                c.sell_sol += t.sol;
                sellers.insert(t.trader);
                ssz.push(t.sol);
            }
            if t.time <= early_end {
                c.early_trades += 1;
                if t.buy {
                    eb.insert(t.trader);
                    c.early_buy_sol += t.sol;
                    ebsz.push(t.sol);
                    *ebuys_per.entry(t.trader).or_default() += 1;
                    c.early_max_buy_sol = c.early_max_buy_sol.max(t.sol);
                } else {
                    es.insert(t.trader);
                    c.early_sell_sol += t.sol;
                }
            }
        }
        c.traders = traders.len();
        c.buyers = buyers.len();
        c.sellers = sellers.len();
        c.net_flow_sol = c.buy_sol - c.sell_sol;
        c.median_buy_sol = median(&mut bsz);
        c.median_sell_sol = median(&mut ssz);
        c.early_buyers = eb.len();
        c.early_sellers = es.len();
        c.early_dust_buy_share =
            if ebsz.is_empty() { f64::NAN } else { ebsz.iter().filter(|x| **x < MIN_PRICE_SOL).count() as f64 / ebsz.len() as f64 };
        c.early_median_buy_sol = median(&mut ebsz);
        c.early_repeat_buyer_share =
            if eb.is_empty() { f64::NAN } else { ebuys_per.values().filter(|n| **n > 1).count() as f64 / eb.len() as f64 };
        c.early_new_wallet_share = if eb.is_empty() {
            f64::NAN
        } else {
            eb.iter().filter(|b| first_coin_of_trader.get(b) == Some(&mint)).count() as f64 / eb.len() as f64
        };
        c.newborn_in_window = c.first_time > window_start + 60;
        c.new_wallet_buyer_share = if buyers.is_empty() {
            f64::NAN
        } else {
            buyers.iter().filter(|b| first_coin_of_trader.get(b) == Some(&mint)).count() as f64 / buyers.len() as f64
        };
        // Price path.
        let priced: Vec<&T> = ts.iter().filter(|t| t.sol >= MIN_PRICE_SOL && t.price.is_finite() && t.price > 0.0).collect();
        c.priced_trades = priced.len();
        if priced.len() >= MIN_PRICED_TRADES {
            let mut first3: Vec<f64> = priced[..3].iter().map(|t| t.price).collect();
            c.start_price = median(&mut first3);
            let smooth: Vec<(i64, f64)> = (2..priced.len())
                .map(|i| {
                    let mut w = [priced[i - 2].price, priced[i - 1].price, priced[i].price];
                    (priced[i].time, median(&mut w))
                })
                .collect();
            let (pt, pk) = smooth.iter().fold((0, 0.0f64), |a, &(t, p)| if p > a.1 { (t, p) } else { a });
            let low = smooth.iter().fold(f64::INFINITY, |a, &(_, p)| a.min(p));
            c.peak_mult = pk / c.start_price;
            c.peak_after_s = pt - c.first_time;
            c.low_mult = low / c.start_price;
            c.end_mult = smooth.last().map(|x| x.1).unwrap_or(f64::NAN) / c.start_price;
            // Outcome strictly after the early window, relative to the price at its end.
            let at_early = smooth.iter().filter(|(t, _)| *t <= early_end).next_back().map(|x| x.1);
            c.early_price_mult = at_early.map(|p| p / c.start_price).unwrap_or(f64::NAN);
            let after_peak = smooth.iter().filter(|(t, _)| *t > early_end).fold(f64::NAN, |a, &(_, p)| if a.is_nan() || p > a { p } else { a });
            c.after_early_peak_mult = match at_early {
                Some(p0) if after_peak.is_finite() => after_peak / p0,
                _ => f64::NAN,
            };
        } else {
            c.start_price = f64::NAN;
            c.peak_mult = f64::NAN;
            c.low_mult = f64::NAN;
            c.end_mult = f64::NAN;
            c.after_early_peak_mult = f64::NAN;
            c.early_price_mult = f64::NAN;
        }
        coins.push(c);
    }
    // Highest peak first; coins without a price path last.
    let key = |c: &Coin| if c.peak_mult.is_finite() { c.peak_mult } else { f64::NEG_INFINITY };
    coins.sort_by(|a, b| key(b).total_cmp(&key(a)));

    // coins.csv
    let mut f = std::io::BufWriter::new(std::fs::File::create(out.join("coins.csv"))?);
    writeln!(f, "mint,trades,buys,sells,traders,buyers,sellers,buy_sol,sell_sol,net_flow_sol,median_buy_sol,median_sell_sol,max_buy_sol,priced_trades,start_price_sol,peak_mult,peak_after_s,low_mult,end_mult,new_wallet_buyer_share,early_trades,early_buyers,early_sellers,early_buy_sol,early_sell_sol,early_median_buy_sol,early_max_buy_sol,early_dust_buy_share,early_repeat_buyer_share,early_new_wallet_share,early_price_mult,newborn_in_window,after_early_peak_mult,first_time,last_time")?;
    for c in &coins {
        writeln!(
            f,
            "{},{},{},{},{},{},{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{},{:.3e},{:.3},{},{:.3},{:.3},{:.3},{},{},{},{:.4},{:.4},{:.4},{:.4},{:.3},{:.3},{:.3},{:.3},{},{:.3},{},{}",
            c.mint, c.trades, c.buys, c.sells, c.traders, c.buyers, c.sellers, c.buy_sol, c.sell_sol, c.net_flow_sol,
            c.median_buy_sol, c.median_sell_sol, c.max_buy_sol, c.priced_trades, c.start_price, c.peak_mult, c.peak_after_s,
            c.low_mult, c.end_mult, c.new_wallet_buyer_share, c.early_trades, c.early_buyers, c.early_sellers, c.early_buy_sol,
            c.early_sell_sol, c.early_median_buy_sol, c.early_max_buy_sol, c.early_dust_buy_share, c.early_repeat_buyer_share,
            c.early_new_wallet_share, c.early_price_mult, c.newborn_in_window as u8, c.after_early_peak_mult, c.first_time, c.last_time
        )?;
    }
    f.flush()?;

    let priced: Vec<&Coin> = coins.iter().filter(|c| c.peak_mult.is_finite()).collect();
    let count = |p: &dyn Fn(&Coin) -> bool| priced.iter().filter(|c| p(c)).count();
    let peak_buckets = vec![
        Bucket { label: "peak >= 2x (+100%)", coins: count(&|c| c.peak_mult >= 2.0) },
        Bucket { label: "peak >= 3x (+200%)", coins: count(&|c| c.peak_mult >= 3.0) },
        Bucket { label: "peak >= 5x (+400%)", coins: count(&|c| c.peak_mult >= 5.0) },
        Bucket { label: "peak >= 10x (+900%)", coins: count(&|c| c.peak_mult >= 10.0) },
        Bucket { label: "peak >= 11x (+1000%)", coins: count(&|c| c.peak_mult >= 11.0) },
        Bucket { label: "peak >= 20x", coins: count(&|c| c.peak_mult >= 20.0) },
        Bucket { label: "peak >= 50x", coins: count(&|c| c.peak_mult >= 50.0) },
        Bucket { label: "peak >= 100x", coins: count(&|c| c.peak_mult >= 100.0) },
        Bucket { label: "peak 1.5x-2x", coins: count(&|c| c.peak_mult >= 1.5 && c.peak_mult < 2.0) },
        Bucket { label: "peak < 1.5x", coins: count(&|c| c.peak_mult < 1.5) },
    ];
    let low_buckets = vec![
        Bucket { label: "low <= 0.5x (fell -50%)", coins: count(&|c| c.low_mult <= 0.5) },
        Bucket { label: "low <= 0.1x (fell -90%)", coins: count(&|c| c.low_mult <= 0.1) },
        Bucket { label: "end <= 0.5x", coins: count(&|c| c.end_mult <= 0.5) },
        Bucket { label: "end >= 2x", coins: count(&|c| c.end_mult >= 2.0) },
    ];

    // Early activity by outcome group (features from the first EARLY_SECS, outcome strictly after).
    #[derive(Serialize)]
    struct Group {
        group: &'static str,
        coins: usize,
        median_early_buyers: f64,
        median_early_sellers: f64,
        median_early_buy_sol: f64,
        median_early_sell_sol: f64,
        median_early_buy_sell_ratio: f64,
        median_early_median_buy_sol: f64,
        median_early_trades: f64,
    }
    let with_outcome: Vec<&&Coin> = priced.iter().filter(|c| c.after_early_peak_mult.is_finite()).collect();
    let group = |name: &'static str, sel: &dyn Fn(f64) -> bool| -> Group {
        let g: Vec<&&&Coin> = with_outcome.iter().filter(|c| sel(c.after_early_peak_mult)).collect();
        let med = |f: &dyn Fn(&Coin) -> f64| {
            let mut v: Vec<f64> = g.iter().map(|c| f(c)).filter(|x| x.is_finite()).collect();
            median(&mut v)
        };
        Group {
            group: name,
            coins: g.len(),
            median_early_buyers: med(&|c| c.early_buyers as f64),
            median_early_sellers: med(&|c| c.early_sellers as f64),
            median_early_buy_sol: med(&|c| c.early_buy_sol),
            median_early_sell_sol: med(&|c| c.early_sell_sol),
            median_early_buy_sell_ratio: med(&|c| c.early_buy_sol / (c.early_sell_sol + 1e-9)),
            median_early_median_buy_sol: med(&|c| c.early_median_buy_sol),
            median_early_trades: med(&|c| c.early_trades as f64),
        }
    };
    let groups = vec![
        group("after-early peak < 1.5x", &|m| m < 1.5),
        group("after-early peak 1.5x-2x", &|m| (1.5..2.0).contains(&m)),
        group("after-early peak 2x-5x", &|m| (2.0..5.0).contains(&m)),
        group("after-early peak 5x-10x", &|m| (5.0..10.0).contains(&m)),
        group("after-early peak >= 10x", &|m| m >= 10.0),
    ];

    let top: Vec<serde_json::Value> = priced
        .iter()
        .take(25)
        .map(|c| {
            serde_json::json!({ "mint": c.mint, "peak_mult": c.peak_mult, "peak_after_s": c.peak_after_s, "end_mult": c.end_mult,
            "trades": c.trades, "buyers": c.buyers, "sellers": c.sellers, "buy_sol": c.buy_sol, "sell_sol": c.sell_sol,
            "median_buy_sol": c.median_buy_sol, "median_sell_sol": c.median_sell_sol, "max_buy_sol": c.max_buy_sol,
            "new_wallet_buyer_share": c.new_wallet_buyer_share })
        })
        .collect();
    let mut oc: Vec<(&str, u64)> = outcomes.into_iter().collect();
    oc.sort();
    let summary = serde_json::json!({
        "blocks": blocks, "slot_range": [min_slot, max_slot], "chain_time_s": max_t - min_t,
        "tx_outcomes": oc.into_iter().collect::<std::collections::BTreeMap<_, _>>(),
        "traders": trader_ids.len(), "coins_traded_vs_sol": coins.len(),
        "coins_with_price_path": priced.len(), "price_path_rule": format!(">= {MIN_PRICED_TRADES} trades of >= {MIN_PRICE_SOL} SOL"),
        "peak_buckets": peak_buckets, "low_end_buckets": low_buckets,
        "early_window_s": EARLY_SECS, "early_activity_by_outcome": groups, "top25_by_peak": top,
    });
    std::fs::write(out.join("summary.json"), serde_json::to_string_pretty(&summary)?)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
