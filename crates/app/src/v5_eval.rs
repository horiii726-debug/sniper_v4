//! V5 Phases 7–8 (EXPLORATORY): walk-forward strategy evaluation on out-of-sample model scores, a small grid
//! (<= 24 points, selected by the MEDIAN dev-window return), cost sensitivity, the acceptance check, and Monte
//! Carlo (execution-randomness reruns + trade / window bootstrap). All simulations run in parallel (rayon).
//!
//! Fold layout (python/ml/v5_train.py): <models>/fold_<W>/scores_<W>.csv.zst are scores of window W from models
//! trained only on earlier windows (purged by H); <models>/fold_<W>/entry_bin.meta.json lists those training
//! windows. R_k for W is measured on exactly those training windows.

use crate::ml_sim::{self, H_S};
use crate::strategy_v5::{self as s5, CostsV5, FileScores, ReportV5, StratV5, WindowData};
use anyhow::{anyhow, Result};
use radar::Params as RadarParams;
use rayon::prelude::*;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub struct Loaded {
    pub name: String,
    pub dir: PathBuf,
    pub ev: crate::backtest::Events,
    pub sol_usd: f64,
}

pub fn load_windows(recs: &[PathBuf]) -> Result<Vec<Loaded>> {
    recs.par_iter()
        .map(|r| -> Result<Loaded> {
            let ev = crate::backtest::load(r)?;
            let sol_usd = crate::backtest::sol_usd_at(ev.window.0)?;
            Ok(Loaded { name: dir_name(r), dir: r.clone(), ev, sol_usd })
        })
        .collect()
}

pub fn dir_name(p: &Path) -> String {
    p.file_name().and_then(|x| x.to_str()).unwrap_or("").to_string()
}

fn wd(l: &Loaded) -> Result<WindowData<'_>> {
    Ok(WindowData::new(l.name.clone(), &l.ev, ml_sim::load_context(&l.dir, &l.ev)?, l.sol_usd))
}

/// Training windows of fold W from its meta file (names compared by directory basename).
fn fold_train(models: &Path, w: &str) -> Result<Vec<String>> {
    let p = models.join(format!("fold_{w}")).join("entry_bin.meta.json");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).map_err(|e| anyhow!("{}: {e}", p.display()))?)?;
    Ok(v["train_windows"].as_array().ok_or_else(|| anyhow!("{}: no train_windows", p.display()))?
        .iter().filter_map(|x| x.as_str()).map(|x| dir_name(Path::new(x))).collect())
}

fn scores(models: &Path, fold: &str, w: &str) -> Result<FileScores> {
    FileScores::read(&models.join(format!("fold_{fold}")).join(format!("scores_{w}.csv.zst")))
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowResult {
    pub window: String,
    pub rk: [f64; 7],
    pub rk_trades: [usize; 7],
    pub report: ReportV5,
}

/// Walk-forward run of one strategy over the test windows (each with its own fold models and R_k).
pub fn run_walk_forward(all: &[Loaded], data: &[WindowData], tests: &[usize], models: &Path, rp: RadarParams, st: &StratV5, c: &CostsV5) -> Result<Vec<WindowResult>> {
    tests.par_iter().map(|&ti| -> Result<WindowResult> {
        let w = &all[ti];
        let train_names = fold_train(models, &w.name)?;
        let train: Vec<&WindowData> = data.iter().filter(|d| train_names.contains(&d.name)).collect();
        // Purge check (defensive): no training window may end within H of the test window start.
        for d in &train {
            if d.ev.data_end() > w.ev.window.0 - H_S {
                return Err(anyhow!("fold {}: training window {} ends within H of the test start", w.name, d.name));
            }
        }
        // Out-of-sample scores of each training window (from its own earlier fold), when that fold exists.
        let own: Vec<Option<FileScores>> = train.iter().map(|d| scores(models, &d.name, &d.name).ok()).collect();
        let pairs: Vec<(&WindowData, Option<&FileScores>)> = train.iter().zip(own.iter()).map(|(d, s)| (*d, s.as_ref())).collect();
        let (rk, rk_trades) = s5::estimate_rk(&pairs, rp, st, c);
        let mut sc = scores(models, &w.name, &w.name)?;
        let report = s5::simulate(&data[ti], rp, &mut sc, st, c, &rk, false);
        Ok(WindowResult { window: w.name.clone(), rk, rk_trades, report })
    }).collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub windows: usize,
    pub positive_windows: usize,
    pub median_return_pct: f64,
    pub mean_return_pct: f64,
    pub trades: usize,
    pub win_rate: f64,
    pub profit_factor: f64,
    pub expectancy_sol: f64,
    pub expectancy_excl_top1pct_sol: f64,
    pub total_pnl_sol: f64,
    pub max_drawdown_pct_worst: f64,
    pub costs_sol: f64,
    pub runners_10x: usize,
    pub runners_captured_5x: usize,
    pub moonshot_capture: f64,
}

pub fn summarize(rs: &[WindowResult]) -> Summary {
    let mut rets: Vec<f64> = rs.iter().map(|r| r.report.return_pct).collect();
    rets.sort_by(|a, b| a.total_cmp(b));
    let trades: Vec<&s5::TradeV5> = rs.iter().flat_map(|r| r.report.trades_list.iter()).collect();
    let n = trades.len();
    let mut pnls: Vec<f64> = trades.iter().map(|t| t.pnl_sol).collect();
    pnls.sort_by(|a, b| b.total_cmp(a));
    let drop = (n as f64 * 0.01).ceil() as usize;
    let excl = &pnls[drop.min(n)..];
    let gp: f64 = pnls.iter().filter(|p| **p > 0.0).sum();
    let gl: f64 = -pnls.iter().filter(|p| **p < 0.0).sum::<f64>();
    let runners: usize = rs.iter().map(|r| r.report.runners_10x).sum();
    let caught: usize = rs.iter().map(|r| r.report.runners_10x_captured_5x).sum();
    Summary {
        windows: rs.len(),
        positive_windows: rs.iter().filter(|r| r.report.return_pct > 0.0).count(),
        median_return_pct: median(&rets),
        mean_return_pct: if rets.is_empty() { 0.0 } else { rets.iter().sum::<f64>() / rets.len() as f64 },
        trades: n,
        win_rate: if n > 0 { pnls.iter().filter(|p| **p > 0.0).count() as f64 / n as f64 } else { 0.0 },
        profit_factor: if gl > 0.0 { gp / gl } else if gp > 0.0 { f64::INFINITY } else { 0.0 },
        expectancy_sol: if n > 0 { pnls.iter().sum::<f64>() / n as f64 } else { 0.0 },
        expectancy_excl_top1pct_sol: if excl.is_empty() { 0.0 } else { excl.iter().sum::<f64>() / excl.len() as f64 },
        total_pnl_sol: pnls.iter().sum(),
        max_drawdown_pct_worst: rs.iter().map(|r| r.report.max_drawdown_pct).fold(0.0, f64::max),
        costs_sol: rs.iter().map(|r| r.report.costs_sol).sum(),
        runners_10x: runners,
        runners_captured_5x: caught,
        moonshot_capture: if runners > 0 { caught as f64 / runners as f64 } else { 0.0 },
    }
}

fn median(sorted: &[f64]) -> f64 {
    match sorted.len() {
        0 => 0.0,
        n if n % 2 == 1 => sorted[n / 2],
        n => 0.5 * (sorted[n / 2 - 1] + sorted[n / 2]),
    }
}

/// V5 §9 grid: 3 x 2 x 2 x 2 = 24 points over ev_min, bag_trail, tp1_mult, pre_tp_stop.
pub fn grid(base: &StratV5) -> Vec<StratV5> {
    let mut g = Vec::new();
    for &ev_min in &[0.05, 0.15, 0.30] {
        for &bag_trail in &[0.40, 0.55] {
            for &tp1_mult in &[2.0, 3.0] {
                for &pre_tp_stop in &[0.35, 0.50] {
                    g.push(StratV5 { ev_min, bag_trail, tp1_mult, pre_tp_stop, ..*base });
                }
            }
        }
    }
    g
}

#[derive(Debug, Clone, Serialize)]
pub struct Acceptance {
    pub expectancy_excl_top1pct_positive: bool,
    pub profit_factor_gt_1_3: bool,
    pub positive_windows_ge_60pct: bool,
    pub moonshot_capture_ge_20pct: bool,
    pub positive_at_latency4_fail030: bool,
    pub all_pass: bool,
}

pub fn acceptance(base: &Summary, stress: &Summary) -> Acceptance {
    let a = base.expectancy_excl_top1pct_sol > 0.0;
    let b = base.profit_factor > 1.3;
    let c = base.windows > 0 && base.positive_windows as f64 >= 0.6 * base.windows as f64;
    let d = base.moonshot_capture >= 0.2;
    let e = stress.total_pnl_sol > 0.0;
    Acceptance { expectancy_excl_top1pct_positive: a, profit_factor_gt_1_3: b, positive_windows_ge_60pct: c, moonshot_capture_ge_20pct: d, positive_at_latency4_fail030: e, all_pass: a && b && c && d && e }
}

#[derive(Debug, Clone, Serialize)]
pub struct Dist {
    pub p5: f64,
    pub p25: f64,
    pub p50: f64,
    pub p75: f64,
    pub p95: f64,
    pub mean: f64,
    pub prob_le_zero: f64,
}

fn dist(mut v: Vec<f64>) -> Dist {
    v.retain(|x| x.is_finite());
    v.sort_by(|a, b| a.total_cmp(b));
    let q = |f: f64| if v.is_empty() { f64::NAN } else { v[((v.len() - 1) as f64 * f).round() as usize] };
    Dist {
        p5: q(0.05), p25: q(0.25), p50: q(0.5), p75: q(0.75), p95: q(0.95),
        mean: if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 },
        prob_le_zero: if v.is_empty() { f64::NAN } else { v.iter().filter(|x| **x <= 0.0).count() as f64 / v.len() as f64 },
    }
}

fn rng(seed: u64) -> impl FnMut() -> u64 {
    let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
    move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MonteCarlo {
    pub execution_runs: usize,
    /// Distribution over execution-randomness reruns (buy-failure seeds) of the pooled walk-forward results.
    pub exec_total_pnl_sol: Dist,
    pub exec_profit_factor: Dist,
    pub exec_win_rate: Dist,
    pub exec_median_window_return_pct: Dist,
    pub bootstrap_resamples: usize,
    /// Trade bootstrap (resample trades with replacement) of the base run.
    pub boot_profit_factor: Dist,
    pub boot_win_rate: Dist,
    pub boot_expectancy_sol: Dist,
    /// Window bootstrap (resample windows with replacement): mean window return and its probability of <= 0.
    pub boot_mean_window_return_pct: Dist,
}

pub fn monte_carlo(all: &[Loaded], data: &[WindowData], tests: &[usize], models: &Path, rp: RadarParams, st: &StratV5, c: &CostsV5, base: &[WindowResult], runs: usize, resamples: usize) -> Result<MonteCarlo> {
    let sums: Vec<Summary> = (1..=runs as u64).into_par_iter()
        .map(|seed| run_walk_forward(all, data, tests, models, rp, st, &CostsV5 { fail_seed: seed, ..*c }).map(|r| summarize(&r)))
        .collect::<Result<_>>()?;
    let trades: Vec<f64> = base.iter().flat_map(|r| r.report.trades_list.iter().map(|t| t.pnl_sol)).collect();
    let wrets: Vec<f64> = base.iter().map(|r| r.report.return_pct).collect();
    let boots: Vec<(f64, f64, f64, f64)> = (0..resamples as u64).into_par_iter().map(|b| {
        let mut r = rng(b + 1);
        let (mut gp, mut gl, mut wins, mut sum) = (0.0, 0.0, 0usize, 0.0);
        for _ in 0..trades.len() {
            let p = trades[(r() % trades.len().max(1) as u64) as usize];
            sum += p;
            if p > 0.0 {
                gp += p;
                wins += 1;
            } else {
                gl -= p;
            }
        }
        let n = trades.len().max(1) as f64;
        let mut wm = 0.0;
        for _ in 0..wrets.len() {
            wm += wrets[(r() % wrets.len().max(1) as u64) as usize];
        }
        (if gl > 0.0 { gp / gl } else { f64::INFINITY }, wins as f64 / n, sum / n, wm / wrets.len().max(1) as f64)
    }).collect();
    Ok(MonteCarlo {
        execution_runs: runs,
        exec_total_pnl_sol: dist(sums.iter().map(|s| s.total_pnl_sol).collect()),
        exec_profit_factor: dist(sums.iter().map(|s| s.profit_factor).collect()),
        exec_win_rate: dist(sums.iter().map(|s| s.win_rate).collect()),
        exec_median_window_return_pct: dist(sums.iter().map(|s| s.median_return_pct).collect()),
        bootstrap_resamples: resamples,
        boot_profit_factor: dist(boots.iter().map(|b| b.0).collect()),
        boot_win_rate: dist(boots.iter().map(|b| b.1).collect()),
        boot_expectancy_sol: dist(boots.iter().map(|b| b.2).collect()),
        boot_mean_window_return_pct: dist(boots.iter().map(|b| b.3).collect()),
    })
}

pub struct EvalOpts {
    pub models: PathBuf,
    pub out: PathBuf,
    pub grid: bool,
    pub mc_runs: usize,
    pub mc_resamples: usize,
    pub costs: CostsV5,
    pub strategy: StratV5,
}

/// Full Phase 7–8 run on the walk-forward (dev) windows. Holdout windows are evaluated separately, once.
pub fn run(recs: &[PathBuf], rp: RadarParams, o: &EvalOpts) -> Result<()> {
    let t0 = std::time::Instant::now();
    let all = load_windows(recs)?;
    let data: Vec<WindowData> = all.iter().map(wd).collect::<Result<_>>()?;
    let tests: Vec<usize> = (0..all.len()).filter(|&i| o.models.join(format!("fold_{}", all[i].name)).join(format!("scores_{}.csv.zst", all[i].name)).exists()).collect();
    if tests.is_empty() {
        return Err(anyhow!("no fold score files under {}", o.models.display()));
    }
    // Grid (selection by median dev-window return), else the given strategy.
    let mut grid_rows = Vec::new();
    let chosen = if o.grid {
        let g = grid(&o.strategy);
        let mut res: Vec<(StratV5, Summary)> = g.par_iter().map(|st| run_walk_forward(&all, &data, &tests, &o.models, rp, st, &o.costs).map(|r| (*st, summarize(&r)))).collect::<Result<_>>()?;
        res.sort_by(|a, b| b.1.median_return_pct.total_cmp(&a.1.median_return_pct));
        grid_rows = res.iter().map(|(st, s)| serde_json::json!({ "strategy": st, "summary": s })).collect();
        res[0].0
    } else {
        o.strategy
    };
    let base = run_walk_forward(&all, &data, &tests, &o.models, rp, &chosen, &o.costs)?;
    let base_sum = summarize(&base);
    // Sensitivity (§8): latency 1/2/4 slots, fail 0.15/0.30, tip x2.
    let variants: Vec<(String, CostsV5)> = vec![
        ("latency1".into(), CostsV5 { latency_slots: 1, ..o.costs }),
        ("latency4".into(), CostsV5 { latency_slots: 4, ..o.costs }),
        ("fail030".into(), CostsV5 { buy_fail_prob: 0.30, ..o.costs }),
        ("tip_x2".into(), CostsV5 { tip_lamports: o.costs.tip_lamports * 2, ..o.costs }),
        ("latency4_fail030".into(), CostsV5 { latency_slots: 4, buy_fail_prob: 0.30, ..o.costs }),
    ];
    let sens: Vec<(String, Summary)> = variants.par_iter().map(|(n, c)| run_walk_forward(&all, &data, &tests, &o.models, rp, &chosen, c).map(|r| (n.clone(), summarize(&r)))).collect::<Result<_>>()?;
    let stress = sens.iter().find(|(n, _)| n == "latency4_fail030").map(|x| x.1.clone()).expect("stress variant");
    let acc = acceptance(&base_sum, &stress);
    let mc = if o.mc_runs > 0 { Some(monte_carlo(&all, &data, &tests, &o.models, rp, &chosen, &o.costs, &base, o.mc_runs, o.mc_resamples)?) } else { None };
    let rep = serde_json::json!({
        "label": "EXPLORATORY V5 walk-forward backtest (pre-gate, user-approved); pump.fun curve + PumpSwap; paper only",
        "strategy": chosen, "costs": o.costs, "test_windows": tests.iter().map(|i| &all[*i].name).collect::<Vec<_>>(),
        "summary": base_sum, "acceptance": acc, "sensitivity": sens.iter().map(|(n, s)| serde_json::json!({"variant": n, "summary": s})).collect::<Vec<_>>(),
        "monte_carlo": mc, "grid_sorted_by_median_return": grid_rows,
        "windows": base.iter().map(|w| serde_json::json!({ "window": w.window, "rk": w.rk, "rk_trades": w.rk_trades,
            "report": { "return_pct": w.report.return_pct, "trades": w.report.trades, "win_rate": w.report.win_rate, "profit_factor": w.report.profit_factor,
                "expectancy_sol": w.report.expectancy_sol, "expectancy_excl_top1pct_sol": w.report.expectancy_excl_top1pct_sol, "max_drawdown_pct": w.report.max_drawdown_pct,
                "costs_sol": w.report.costs_sol, "failed_buys": w.report.failed_buys, "exits_by_reason": w.report.exits_by_reason, "final_reasons": w.report.final_reasons,
                "runners_10x": w.report.runners_10x, "captured_5x": w.report.runners_10x_captured_5x, "signals": w.report.signals_in_window, "blocked": w.report.blocked,
                "sol_usd": w.report.sol_usd, "data_s": w.report.data_s } })).collect::<Vec<_>>(),
        "trades": base.iter().flat_map(|w| w.report.trades_list.iter().map(move |t| serde_json::json!({"window": w.window, "trade": t}))).collect::<Vec<_>>(),
        "elapsed_s": t0.elapsed().as_secs_f64(),
    });
    if let Some(d) = o.out.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&o.out, serde_json::to_string_pretty(&rep)?)?;
    std::fs::write(o.out.with_extension("md"), markdown(&rep))?;
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "summary": rep["summary"], "acceptance": rep["acceptance"], "out": o.out }))?);
    Ok(())
}

/// Human-readable report (reports/V5_RESULTS.md).
pub fn markdown(rep: &serde_json::Value) -> String {
    let mut s = String::new();
    let f = |v: &serde_json::Value| v.as_f64().map(|x| format!("{x:.3}")).unwrap_or_else(|| v.to_string());
    s += "# V5 results (EXPLORATORY, pre-gate, paper only)\n\n";
    s += &format!("Test windows (walk-forward, out-of-sample models): {}\n\n", rep["test_windows"]);
    s += "## Summary (latency 2 slots, base costs)\n\n| metric | value |\n|---|---|\n";
    if let Some(o) = rep["summary"].as_object() {
        for (k, v) in o {
            s += &format!("| {k} | {} |\n", f(v));
        }
    }
    s += "\n## Acceptance (V5 §9)\n\n| criterion | pass |\n|---|---|\n";
    if let Some(o) = rep["acceptance"].as_object() {
        for (k, v) in o {
            s += &format!("| {k} | {v} |\n");
        }
    }
    s += "\n## Per window\n\n| window | return % | trades | win rate | PF | exp. SOL | exp. excl top1% | max DD % | costs SOL | 10x runners | caught >=5x | final exits |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n";
    for w in rep["windows"].as_array().into_iter().flatten() {
        let r = &w["report"];
        s += &format!("| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n", w["window"].as_str().unwrap_or(""), f(&r["return_pct"]), r["trades"], f(&r["win_rate"]),
            f(&r["profit_factor"]), f(&r["expectancy_sol"]), f(&r["expectancy_excl_top1pct_sol"]), f(&r["max_drawdown_pct"]), f(&r["costs_sol"]), r["runners_10x"], r["captured_5x"], r["final_reasons"]);
    }
    s += "\n## Sensitivity\n\n| variant | total PnL SOL | PF | median window % | win rate |\n|---|---|---|---|---|\n";
    for v in rep["sensitivity"].as_array().into_iter().flatten() {
        let m = &v["summary"];
        s += &format!("| {} | {} | {} | {} | {} |\n", v["variant"].as_str().unwrap_or(""), f(&m["total_pnl_sol"]), f(&m["profit_factor"]), f(&m["median_return_pct"]), f(&m["win_rate"]));
    }
    if !rep["monte_carlo"].is_null() {
        s += "\n## Monte Carlo\n\n| quantity | p5 | p25 | p50 | p75 | p95 | mean | P(<=0) |\n|---|---|---|---|---|---|---|---|\n";
        if let Some(o) = rep["monte_carlo"].as_object() {
            for (k, d) in o {
                if d.is_object() {
                    s += &format!("| {k} | {} | {} | {} | {} | {} | {} | {} |\n", f(&d["p5"]), f(&d["p25"]), f(&d["p50"]), f(&d["p75"]), f(&d["p95"]), f(&d["mean"]), f(&d["prob_le_zero"]));
                }
            }
            s += &format!("\nexecution reruns: {}, bootstrap resamples: {}\n", o.get("execution_runs").cloned().unwrap_or_default(), o.get("bootstrap_resamples").cloned().unwrap_or_default());
        }
    }
    s += &format!("\nStrategy: `{}`\n\nCosts: `{}`\n", rep["strategy"], rep["costs"]);
    s
}

/// Side-by-side test on the same windows and costs, no ML on either side (no V5 models exist yet):
/// V4 rule strategy (frozen config/strategy_params.json, V4 exits) vs the V5 corrections that need no model
/// (radar signal entry with V5 entry filters, slots / exposure, TP1 / TP2 / trailing ladder, no window-end sell).
/// Plus a per-runner table: every coin the radar signalled in the window that later reached >= 10x within the
/// available data, and what each strategy did with it.
pub fn compare(recs: &[PathBuf], rp: RadarParams, v4: &crate::backtest::Strategy, st: &StratV5, out: &Path) -> Result<()> {
    let all = load_windows(recs)?;
    let rows: Vec<serde_json::Value> = all.par_iter().map(|l| -> Result<serde_json::Value> {
        let d = wd(l)?;
        let c4 = crate::backtest::Costs::default();
        let r4 = crate::backtest::simulate(&l.ev, v4, &c4, rp, 100.0 / l.sol_usd, l.sol_usd);
        // V5 without a model: every signal has EV = 1 >= ev_min, rug/hold/hazard models silent.
        let c5 = CostsV5 { latency_slots: c4.latency_slots, buy_fail_prob: c4.buy_fail_prob, default_fee_bps: c4.default_fee_bps, ..Default::default() };
        let r5 = s5::simulate(&d, rp, &mut s5::NoScores, st, &c5, &[1.0; 7], false);
        // Runners: peak multiple after the coin's FIRST in-window signal (labels over the available data).
        let mut first_sig: rustc_hash::FxHashMap<u32, i64> = Default::default();
        {
            let mut eng = crate::ml_sim::Engine::new(&l.ev, rp);
            for e in &l.ev.evs {
                if eng.apply(e).signal && e.time <= l.ev.window.1 {
                    first_sig.entry(e.coin).or_insert(e.time);
                }
            }
        }
        let mut runners = Vec::new();
        for (&coin, &t) in &first_sig {
            let ci = coin as usize;
            let Some(y) = ml_sim::labels(&d.paths[ci], t, &d.deaths[ci], l.ev.cover_end[ci]) else { continue };
            let peak = (y.logmax as f64).exp();
            if peak < 10.0 {
                continue;
            }
            let mint = &l.ev.mints[ci];
            let t4: Vec<_> = r4.trades_list.iter().filter(|x| &x.mint == mint).map(|x| serde_json::json!({"ret": x.ret, "max_mult": x.max_mult_held, "exit": x.reason})).collect();
            let t5: Vec<_> = r5.trades_list.iter().filter(|x| &x.mint == mint).map(|x| serde_json::json!({"ret": x.ret, "max_mult": x.max_mult, "exit": x.final_reason, "legs": x.legs.iter().map(|g| g.reason).collect::<Vec<_>>()})).collect();
            let mut b5: std::collections::BTreeMap<&str, usize> = Default::default();
            for (c, r) in &r5.signal_outcomes {
                if *c == coin {
                    *b5.entry(r).or_default() += 1;
                }
            }
            runners.push(serde_json::json!({ "mint": mint, "peak_after_signal": peak, "signal_t": t - l.ev.window.0,
                "v4": { "trades": t4, "blocked": r4.diag[ci].blocked, "first_hot_t": r4.diag[ci].first_hot_time },
                "v5": { "trades": t5, "signal_outcomes": b5 } }));
        }
        runners.sort_by(|a, b| b["peak_after_signal"].as_f64().unwrap_or(0.0).total_cmp(&a["peak_after_signal"].as_f64().unwrap_or(0.0)));
        let pf = |gp: f64, gl: f64| if gl > 0.0 { gp / gl } else if gp > 0.0 { f64::INFINITY } else { 0.0 };
        let gp4: f64 = r4.trades_list.iter().filter(|t| t.pnl_sol > 0.0).map(|t| t.pnl_sol).sum();
        let gl4: f64 = -r4.trades_list.iter().filter(|t| t.pnl_sol < 0.0).map(|t| t.pnl_sol).sum::<f64>();
        let caught4 = runners.iter().filter(|r| r["v4"]["trades"].as_array().is_some_and(|a| a.iter().any(|t| t["max_mult"].as_f64().unwrap_or(0.0) >= 5.0))).count();
        Ok(serde_json::json!({
            "window": l.name, "window_s": l.ev.window.1 - l.ev.window.0, "data_s": l.ev.data_end() - l.ev.window.0, "followed_coins": l.ev.followed.iter().filter(|x| **x).count(),
            "sol_usd": l.sol_usd,
            "v4": { "final_usd": r4.final_usd, "pnl_usd": r4.final_usd - 100.0, "trades": r4.trades, "win_rate": r4.win_rate, "profit_factor": pf(gp4, gl4),
                    "max_drawdown_pct": r4.max_drawdown_pct, "exits": r4.exits_by_reason, "held_to_5x": r4.trades_list.iter().filter(|t| t.max_mult_held >= 5.0).count(),
                    "runners_caught_5x": caught4 },
            "v5_no_model": { "final_usd": r5.final_sol * l.sol_usd, "pnl_usd": (r5.final_sol - r5.start_sol) * l.sol_usd, "trades": r5.trades, "win_rate": r5.win_rate,
                    "profit_factor": r5.profit_factor, "max_drawdown_pct": r5.max_drawdown_pct, "exits": r5.exits_by_reason, "final_exits": r5.final_reasons,
                    "held_to_5x": r5.trades_list.iter().filter(|t| t.max_mult >= 5.0).count(), "runners_caught_5x": r5.runners_10x_captured_5x,
                    "signals": r5.signals_in_window, "blocked": r5.blocked },
            "runners_10x": runners.len(), "runners": runners,
        }))
    }).collect::<Result<_>>()?;
    let rep = serde_json::json!({ "label": "EXPLORATORY comparison V4 rule strategy vs V5 corrections without ML (pre-gate, paper only)", "windows": rows });
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(out, serde_json::to_string_pretty(&rep)?)?;
    for w in rep["windows"].as_array().into_iter().flatten() {
        println!("{}: data {}s | V4 $100 -> ${:.2} trades {} win {:.0}% PF {:.2} | V5(no ML) $100 -> ${:.2} trades {} win {:.0}% PF {:.2} | 10x runners {} caught>=5x V4 {} V5 {}",
            w["window"].as_str().unwrap_or(""), w["data_s"], w["v4"]["final_usd"].as_f64().unwrap_or(0.0), w["v4"]["trades"], w["v4"]["win_rate"].as_f64().unwrap_or(0.0) * 100.0,
            w["v4"]["profit_factor"].as_f64().unwrap_or(0.0), w["v5_no_model"]["final_usd"].as_f64().unwrap_or(0.0), w["v5_no_model"]["trades"],
            w["v5_no_model"]["win_rate"].as_f64().unwrap_or(0.0) * 100.0, w["v5_no_model"]["profit_factor"].as_f64().unwrap_or(0.0), w["runners_10x"],
            w["v4"]["runners_caught_5x"], w["v5_no_model"]["runners_caught_5x"]);
    }
    Ok(())
}
