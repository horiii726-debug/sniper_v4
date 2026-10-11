//! Forensics for the exploratory backtest: why runners were not monetised and why trades lost.
//! Uses the same simulator and frozen params; price paths use pool marginal prices (quote/token reserves).

use crate::backtest::{self, Costs, Events, Strategy};
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn path_stats(p: &[(i64, f64)]) -> Option<(f64, i64, i64, f64, i64)> {
    // (run multiple, base time, peak time, base price, time first crossing 1.3x base)
    if p.len() < 5 {
        return None;
    }
    let (ip, &(tp, peak)) = p.iter().enumerate().max_by(|a, b| a.1 .1.total_cmp(&b.1 .1))?;
    let (ib, &(tb, base)) = p[..=ip].iter().enumerate().min_by(|a, b| a.1 .1.total_cmp(&b.1 .1))?;
    let tc = p[ib..=ip].iter().find(|x| x.1 >= 1.3 * base).map(|x| x.0).unwrap_or(tp);
    Some((peak / base, tb, tp, base, tc))
}

pub fn run(recs: &[PathBuf], radar_params: &PathBuf) -> Result<()> {
    let rp = crate::radar_feed::load_params(radar_params)?;
    let st: Strategy = serde_json::from_str(&std::fs::read_to_string("config/strategy_params.json")?)?;
    let costs = Costs::default();
    let mut all = Vec::new();
    for rec in recs {
        let ev: Events = backtest::load(rec)?;
        let usd = crate::units::sol_usd();
        let r = backtest::simulate(&ev, &st, &costs, rp, crate::units::start_sol(), usd);
        let paths = backtest::price_paths(&ev);
        let mut trades_by: BTreeMap<String, Vec<&backtest::TradeRec>> = BTreeMap::new();
        for t in &r.trades_list {
            trades_by.entry(t.mint.clone()).or_default().push(t);
        }
        let mut tiers = Vec::new();
        for &x in &[2.0f64, 5.0, 11.0] {
            let (mut n, mut hot_before, mut never_hot, mut hot_after, mut entered_any, mut entered_in_run, mut profitable) = (0, 0, 0, 0, 0, 0, 0);
            let mut realized_mults = Vec::new();
            let mut blocks: BTreeMap<&'static str, u32> = BTreeMap::new();
            let mut exit_reasons: BTreeMap<String, u32> = BTreeMap::new();
            let mut examples = Vec::new();
            let (mut late_base, mut instant) = (0, 0);
            let mut from_hot: Vec<f64> = Vec::new();
            for (c, p) in paths.iter().enumerate() {
                let Some((run, tb, tp, _base, tc)) = path_stats(p) else { continue };
                if run < x {
                    continue;
                }
                n += 1;
                if tc - tb <= 2 {
                    instant += 1;
                }
                if tb < 60 {
                    late_base += 1;
                }
                let d = &r.diag[c];
                // Upside available from the first HOT flag (+1 s latency) to the peak, marginal price, before costs.
                if let Some(h) = d.first_hot_time {
                    if h < tp {
                        if let Some(ph) = p.iter().find(|q| q.0 >= h + 1).map(|q| q.1) {
                            let pk = p.iter().filter(|q| q.0 >= h + 1).map(|q| q.1).fold(0.0, f64::max);
                            from_hot.push(pk / ph);
                        }
                    }
                }
                match d.first_hot_time {
                    None => never_hot += 1,
                    Some(h) if h <= tc => hot_before += 1,
                    Some(_) => hot_after += 1,
                }
                let mint = &ev.mints[c];
                let ts = trades_by.get(mint);
                if ts.is_some() {
                    entered_any += 1;
                }
                let in_run: Vec<&&backtest::TradeRec> =
                    ts.map(|v| v.iter().filter(|t| t.entry_time >= tb - 5 && t.entry_time <= tp).collect()).unwrap_or_default();
                if !in_run.is_empty() {
                    entered_in_run += 1;
                    let pnl: f64 = in_run.iter().map(|t| t.pnl_sol).sum();
                    if pnl > 0.0 {
                        profitable += 1;
                    }
                    for t in &in_run {
                        *exit_reasons.entry(t.reason.clone()).or_default() += 1;
                        realized_mults.push((t.proceeds_sol / t.cost_sol, t.max_mult_held, run));
                    }
                } else {
                    let top = d.blocked.iter().max_by_key(|x| x.1).map(|x| *x.0).unwrap_or(if d.first_hot_time.is_none() { "never_hot" } else { "no_real_buy_while_hot" });
                    *blocks.entry(top).or_default() += 1;
                }
                if x >= 11.0 && examples.len() < 12 {
                    examples.push(serde_json::json!({
                        "mint": mint, "run": run, "base_t": tb, "cross30_t": tc, "peak_t": tp, "first_hot_t": d.first_hot_time,
                        "entries": d.entries, "trades": ts.map(|v| v.iter().map(|t| serde_json::json!({
                            "entry_t": t.entry_time, "exit_t": t.exit_time, "reason": t.reason, "ret": t.ret, "max_mult_held": t.max_mult_held
                        })).collect::<Vec<_>>()), "top_blocks": d.blocked,
                    }));
                }
            }
            realized_mults.sort_by(|a, b| a.0.total_cmp(&b.0));
            let med = |i: usize| -> f64 {
                if realized_mults.is_empty() {
                    return f64::NAN;
                }
                let v: Vec<f64> = realized_mults.iter().map(|m| [m.0, m.1, m.2][i]).collect();
                let mut v = v;
                v.sort_by(|a, b| a.total_cmp(b));
                v[v.len() / 2]
            };
            from_hot.sort_by(|a, b| a.total_cmp(b));
            let q = |f: f64| from_hot.get(((from_hot.len() as f64 - 1.0) * f) as usize).copied();
            tiers.push(serde_json::json!({
                "upside_from_first_hot_to_peak": {"n": from_hot.len(), "p25": q(0.25), "median": q(0.5), "p75": q(0.75),
                    ">=2x": from_hot.iter().filter(|x| **x >= 2.0).count(), ">=5x": from_hot.iter().filter(|x| **x >= 5.0).count(), ">=10x": from_hot.iter().filter(|x| **x >= 10.0).count()},
                "tier": x, "runners": n, "instant_jump_runners": instant, "base_in_first_60s": late_base,
                "hot_before_cross30": hot_before, "hot_only_after_cross30": hot_after, "never_hot": never_hot,
                "entered_any_time": entered_any, "entered_during_run": entered_in_run, "profitable_in_run": profitable,
                "median_realized_mult": med(0), "median_max_mult_while_held": med(1), "median_run_mult": med(2),
                "not_entered_top_block_reason": blocks, "exit_reasons_in_run": exit_reasons, "examples_11x": examples,
            }));
        }
        // Losers: how did losing trades end, and did we chase?
        let mut loss_reasons: BTreeMap<String, (u32, f64)> = BTreeMap::new();
        for t in r.trades_list.iter().filter(|t| t.pnl_sol <= 0.0) {
            let e = loss_reasons.entry(t.reason.clone()).or_default();
            e.0 += 1;
            e.1 += t.pnl_sol * usd;
        }
        let entry_vs_60s: Vec<f64> = r
            .trades_list
            .iter()
            .filter_map(|t| {
                let c = ev.mints.iter().position(|m| *m == t.mint)?;
                let p = &paths[c];
                let at = p.iter().filter(|x| x.0 <= t.entry_time).next_back()?.1;
                let before = p.iter().filter(|x| x.0 <= t.entry_time - 60).next_back().or(p.first())?.1;
                Some(at / before)
            })
            .collect();
        let mut ev60 = entry_vs_60s.clone();
        ev60.sort_by(|a, b| a.total_cmp(b));
        let costs_usd = r.tx_costs_sol * usd;
        let fees_est_usd: f64 = r.trades_list.iter().map(|t| t.cost_sol * 0.025).sum::<f64>() * usd;
        all.push(serde_json::json!({
            "recording": rec, "final_usd": r.final_usd, "trades": r.trades, "network_costs_usd": costs_usd,
            "venue_fees_est_usd": fees_est_usd, "tiers": tiers,
            "losing_trades_by_exit": loss_reasons.into_iter().map(|(k, (n, usd))| (k, serde_json::json!({"n": n, "usd": usd}))).collect::<serde_json::Map<_, _>>(),
            "entry_price_vs_60s_before": { "median": ev60.get(ev60.len() / 2), "p75": ev60.get(ev60.len() * 3 / 4), "p90": ev60.get(ev60.len() * 9 / 10) },
        }));
    }
    let out = serde_json::json!({ "strategy": st, "windows": all });
    std::fs::write("reports/diagnose_exploratory.json", serde_json::to_string_pretty(&out)?)?;
    println!("written reports/diagnose_exploratory.json");
    Ok(())
}
