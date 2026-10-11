//! V5 Phases 7–8 (EXPLORATORY): walk-forward strategy evaluation on out-of-sample model scores, a small grid
//! (<= 24 points, selected by the MEDIAN dev-window return), cost sensitivity, the acceptance check, and Monte
//! Carlo (execution-randomness reruns + trade / window bootstrap). All simulations run in parallel (rayon).
//!
//! Fold layout (python/ml/v5_train.py): <models>/fold_<W>/scores_<W>.csv.zst are scores of window W from models
//! trained only on earlier windows (purged by H); <models>/fold_<W>/entry_bin.meta.json lists those training
//! windows. R_k for W is measured on exactly those training windows.

use crate::ml_sim::{self, label_h};
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
            let sol_usd = crate::units::sol_usd();
            Ok(Loaded { name: dir_name(r), dir: r.clone(), ev, sol_usd })
        })
        .collect()
}

pub fn dir_name(p: &Path) -> String {
    // same window name as python/ml/v5_common.py (the "v5_" directory prefix is dropped)
    let b = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
    b.strip_prefix("v5_").unwrap_or(b).to_string()
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
    pub hold: Option<s5::HoldTable>,
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
            if d.ev.data_end() > w.ev.window.0 - label_h() {
                return Err(anyhow!("fold {}: training window {} ends within H of the test start", w.name, d.name));
            }
        }
        // Out-of-sample scores of each training window (from its own earlier fold), when that fold exists.
        let own: Vec<Option<FileScores>> = train.iter().map(|d| scores(models, &d.name, &d.name).ok()).collect();
        let pairs: Vec<(&WindowData, Option<&FileScores>)> = train.iter().zip(own.iter()).map(|(d, s)| (*d, s.as_ref())).collect();
        let (rk, rk_trades, hold, kelly) = s5::estimate_rk_kelly(&pairs, rp, st, c);
        let mut sc = scores(models, &w.name, &w.name)?;
        let report = s5::simulate_k(&data[ti], rp, &mut sc, st, c, &rk, hold.as_ref(), kelly.as_ref(), false);
        Ok(WindowResult { window: w.name.clone(), rk, rk_trades, hold, report })
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

/// Strategy grid (32 points, file 01 §9.3 + ev_min 0) for the base strategy's exit mode, always 4 ev_min values x 8:
/// Momentum Fade: fade_persist_s x fade_rate_frac x idle_s; Fade+drawdown: fade_persist_s x trail_k x fade_rate_frac;
/// Trail: trail_fixed (4) x price guard on/off; Ladder (V5 §9): bag_trail x tp1_mult x pre_tp_stop.
pub fn grid(base: &StratV5) -> Vec<StratV5> {
    use s5::ExitMode;
    let mut g = Vec::new();
    for &ev_min in &[0.0, 0.05, 0.15, 0.30] {
        let b = StratV5 { ev_min, ..*base };
        match base.exit_mode {
            ExitMode::MomentumFade => {
                for &fade_persist_s in &[5i64, 15] {
                    for &fade_rate_frac in &[0.25, 0.40] {
                        for &idle_s in &[60i64, 180] {
                            g.push(StratV5 { fade_persist_s, fade_rate_frac, idle_s, ..b });
                        }
                    }
                }
            }
            ExitMode::FadeAndDrawdown => {
                for &fade_persist_s in &[5i64, 15] {
                    for &trail_k in &[2.0, 4.0] {
                        for &fade_rate_frac in &[0.25, 0.40] {
                            g.push(StratV5 { fade_persist_s, trail_k, fade_rate_frac, ..b });
                        }
                    }
                }
            }
            ExitMode::Trail => {
                for &trail_fixed in &[0.20, 0.30, 0.45, 0.60] {
                    for &max_entry_ret in &[Some(0.10), None] {
                        g.push(StratV5 { trail_fixed, max_entry_ret, ..b });
                    }
                }
            }
            ExitMode::AdaptiveTrail => {
                for &trail_k in &[2.0, 4.0] {
                    for &d_max in &[0.45, 0.60] {
                        for &max_entry_ret in &[Some(0.10), None] {
                            g.push(StratV5 { trail_k, d_max, max_entry_ret, ..b });
                        }
                    }
                }
            }
            ExitMode::Ladder => {
                for &bag_trail in &[0.40, 0.55] {
                    for &tp1_mult in &[2.0, 3.0] {
                        for &pre_tp_stop in &[0.35, 0.50] {
                            g.push(StratV5 { bag_trail, tp1_mult, pre_tp_stop, ..b });
                        }
                    }
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

/// Variants {"name": {partial StratV5}} merged onto `base` (keys of the partial object override the base).
pub fn load_variants(path: &Path, base: &StratV5) -> Result<Vec<(String, StratV5)>> {
    let v: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let b = serde_json::to_value(base)?;
    v.into_iter().map(|(name, over)| {
        let mut m = b.clone();
        if let (Some(mo), Some(oo)) = (m.as_object_mut(), over.as_object()) {
            for (k, x) in oo {
                if !mo.contains_key(k) {
                    return Err(anyhow!("variant {name}: unknown strategy field {k}"));
                }
                mo.insert(k.clone(), x.clone());
            }
        }
        Ok((name, serde_json::from_value(m)?))
    }).collect()
}

/// Component sweep: each variant walk-forward on the same loaded windows (in parallel), with per-window reports and the
/// entry funnel. No grid, sensitivity or Monte Carlo (use `run` for the full evaluation of a chosen strategy).
pub fn run_variants(recs: &[PathBuf], rp: RadarParams, o: &EvalOpts, variants: &[(String, StratV5)]) -> Result<()> {
    let t0 = std::time::Instant::now();
    let all = load_windows(recs)?;
    let data: Vec<WindowData> = all.iter().map(wd).collect::<Result<_>>()?;
    let tests: Vec<usize> = (0..all.len()).filter(|&i| o.models.join(format!("fold_{}", all[i].name)).join(format!("scores_{}.csv.zst", all[i].name)).exists()).collect();
    if tests.is_empty() {
        return Err(anyhow!("no fold score files under {}", o.models.display()));
    }
    let rows: Vec<serde_json::Value> = variants.par_iter().map(|(name, st)| -> Result<serde_json::Value> {
        let r = run_walk_forward(&all, &data, &tests, &o.models, rp, st, &o.costs)?;
        let s = summarize(&r);
        Ok(serde_json::json!({ "variant": name, "strategy": st, "summary": s,
            "windows": r.iter().map(|w| serde_json::json!({ "window": w.window, "rk": w.rk, "rk_trades": w.rk_trades,
                "return_pct": w.report.return_pct, "final_sol": w.report.final_sol, "trades": w.report.trades, "win_rate": w.report.win_rate,
                "profit_factor": w.report.profit_factor, "max_drawdown_pct": w.report.max_drawdown_pct, "costs_sol": w.report.costs_sol,
                "failed_buys": w.report.failed_buys, "slippage_failed_buys": w.report.slippage_failed_buys, "sell_retries": w.report.sell_retries,
                "runners_10x": w.report.runners_10x, "captured_5x": w.report.runners_10x_captured_5x, "window_s": w.report.window_s,
                "exits_by_reason": w.report.exits_by_reason, "blocked": w.report.blocked, "funnel": w.report.funnel,
                "trades_list": w.report.trades_list })).collect::<Vec<_>>() }))
    }).collect::<Result<_>>()?;
    let rep = serde_json::json!({ "label": "EXPLORATORY component sweep (walk-forward, paper only)", "costs": o.costs,
        "test_windows": tests.iter().map(|i| &all[*i].name).collect::<Vec<_>>(), "variants": rows, "elapsed_s": t0.elapsed().as_secs_f64() });
    if let Some(d) = o.out.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&o.out, serde_json::to_string_pretty(&rep)?)?;
    for v in rep["variants"].as_array().into_iter().flatten() {
        let s = &v["summary"];
        println!("{:28} median return {:>8.2}%  trades {:>5}  win {:.2}  PF {:.2}", v["variant"].as_str().unwrap_or(""),
            s["median_return_pct"].as_f64().unwrap_or(0.0), s["trades"], s["win_rate"].as_f64().unwrap_or(0.0), s["profit_factor"].as_f64().unwrap_or(0.0));
    }
    Ok(())
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
        "label": "EXPLORATORY V5 walk-forward backtest (pre-gate, user-approved); venues per strategy.c18_venues; paper only",
        "strategy": chosen, "costs": o.costs, "test_windows": tests.iter().map(|i| &all[*i].name).collect::<Vec<_>>(),
        "summary": base_sum, "acceptance": acc, "sensitivity": sens.iter().map(|(n, s)| serde_json::json!({"variant": n, "summary": s})).collect::<Vec<_>>(),
        "monte_carlo": mc, "grid_sorted_by_median_return": grid_rows,
        "windows": base.iter().map(|w| serde_json::json!({ "window": w.window, "rk": w.rk, "rk_trades": w.rk_trades, "hold": w.hold,
            "report": { "return_pct": w.report.return_pct, "trades": w.report.trades, "win_rate": w.report.win_rate, "profit_factor": w.report.profit_factor,
                "expectancy_sol": w.report.expectancy_sol, "expectancy_excl_top1pct_sol": w.report.expectancy_excl_top1pct_sol, "max_drawdown_pct": w.report.max_drawdown_pct,
                "costs_sol": w.report.costs_sol, "failed_buys": w.report.failed_buys, "exits_by_reason": w.report.exits_by_reason, "final_reasons": w.report.final_reasons,
                "runners_10x": w.report.runners_10x, "captured_5x": w.report.runners_10x_captured_5x, "signals": w.report.signals_in_window, "blocked": w.report.blocked,
                "sol_usd": w.report.sol_usd, "data_s": w.report.data_s, "window_s": w.report.window_s, "slippage_failed_buys": w.report.slippage_failed_buys,
                "sell_retries": w.report.sell_retries, "funnel": w.report.funnel } })).collect::<Vec<_>>(),
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
        let r4 = crate::backtest::simulate(&l.ev, v4, &c4, rp, crate::units::start_sol(), l.sol_usd);
        // V5 without a model: every signal has EV = 1 >= ev_min, rug/hold/hazard models silent.
        let c5 = CostsV5 { latency_slots: c4.latency_slots, buy_fail_prob: c4.buy_fail_prob, default_fee_bps: c4.default_fee_bps, ..Default::default() };
        let r5 = s5::simulate(&d, rp, &mut s5::NoScores, st, &c5, &[1.0; 7], None, false);
        // Exit variants A-D (prediction spec Bagian 16) on the same window, same entries, no model.
        let variants: Vec<serde_json::Value> = [s5::ExitMode::MomentumFade, s5::ExitMode::FadeAndDrawdown, s5::ExitMode::Trail, s5::ExitMode::AdaptiveTrail, s5::ExitMode::Ladder]
            .iter()
            .map(|m| {
                let r = s5::simulate(&d, rp, &mut s5::NoScores, &StratV5 { exit_mode: *m, ..*st }, &c5, &[1.0; 7], None, false);
                serde_json::json!({ "exit_mode": m, "pnl_sol": r.final_sol - r.start_sol, "trades": r.trades, "win_rate": r.win_rate,
                    "profit_factor": r.profit_factor, "expectancy_excl_top1pct_sol": r.expectancy_excl_top1pct_sol,
                    "held_to_5x": r.trades_list.iter().filter(|t| t.max_mult >= 5.0).count(), "runners_caught_5x": r.runners_10x_captured_5x,
                    "runners_10x": r.runners_10x, "final_exits": r.final_reasons })
            })
            .collect();
        // Runners: peak multiple after the coin's FIRST in-window signal (labels over the available data).
        let mut first_sig: rustc_hash::FxHashMap<u32, i64> = Default::default();
        {
            let mut eng = crate::ml_sim::Engine::new(&l.ev, rp).with_ctx(d.ctx.clone());
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
            for (c, r, _) in &r5.signal_outcomes {
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
            "v4": { "start_sol": r4.start_sol, "final_sol": r4.final_sol, "pnl_sol": r4.final_sol - r4.start_sol, "final_usd": r4.final_usd, "pnl_usd": (r4.final_sol - r4.start_sol) * l.sol_usd, "trades": r4.trades, "win_rate": r4.win_rate, "profit_factor": pf(gp4, gl4),
                    "max_drawdown_pct": r4.max_drawdown_pct, "exits": r4.exits_by_reason, "held_to_5x": r4.trades_list.iter().filter(|t| t.max_mult_held >= 5.0).count(),
                    "runners_caught_5x": caught4 },
            "v5_no_model": { "start_sol": r5.start_sol, "final_sol": r5.final_sol, "pnl_sol": r5.final_sol - r5.start_sol, "final_usd": r5.final_sol * l.sol_usd, "pnl_usd": (r5.final_sol - r5.start_sol) * l.sol_usd, "trades": r5.trades, "win_rate": r5.win_rate,
                    "profit_factor": r5.profit_factor, "max_drawdown_pct": r5.max_drawdown_pct, "exits": r5.exits_by_reason, "final_exits": r5.final_reasons,
                    "held_to_5x": r5.trades_list.iter().filter(|t| t.max_mult >= 5.0).count(), "runners_caught_5x": r5.runners_10x_captured_5x,
                    "signals": r5.signals_in_window, "blocked": r5.blocked, "replacements": r5.replacements, "merged_in_tick": r5.merged_in_tick },
            "v5_exit_variants": variants,
            "runners_10x": runners.len(), "runners": runners,
        }))
    }).collect::<Result<_>>()?;
    let rep = serde_json::json!({ "label": "EXPLORATORY comparison V4 rule strategy vs V6 strategy without ML, plus exit variants A-D (pre-gate, paper only)", "windows": rows });
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(out, serde_json::to_string_pretty(&rep)?)?;
    for w in rep["windows"].as_array().into_iter().flatten() {
        println!("{}: data {}s | V4 {:.4} -> {:.4} SOL trades {} win {:.0}% PF {:.2} | V5(no ML) -> {:.4} SOL trades {} win {:.0}% PF {:.2} | 10x runners {} caught>=5x V4 {} V5 {}",
            w["window"].as_str().unwrap_or(""), w["data_s"], w["v4"]["start_sol"].as_f64().unwrap_or(0.0), w["v4"]["final_sol"].as_f64().unwrap_or(0.0), w["v4"]["trades"], w["v4"]["win_rate"].as_f64().unwrap_or(0.0) * 100.0,
            w["v4"]["profit_factor"].as_f64().unwrap_or(0.0), w["v5_no_model"]["final_sol"].as_f64().unwrap_or(0.0), w["v5_no_model"]["trades"],
            w["v5_no_model"]["win_rate"].as_f64().unwrap_or(0.0) * 100.0, w["v5_no_model"]["profit_factor"].as_f64().unwrap_or(0.0), w["runners_10x"],
            w["v4"]["runners_caught_5x"], w["v5_no_model"]["runners_caught_5x"]);
    }
    Ok(())
}

/// One component variant: (name, radar params, strategy).
type Variant = (String, RadarParams, StratV5);

/// Component ablation (user rule + prediction spec Bagian 18.4): every radar / strategy component is switched off (or
/// replaced) alone, on the same windows and costs, and compared with the base. Radar variants are scored with
/// radar_eval (recall before +10% / at +30%, HOT per minute, precision vs naive); strategy variants with the no-model
/// simulator (entries are not ranked without a model, so strategy results are direction only until models exist).
pub fn ablate(recs: &[PathBuf], rp: RadarParams, st: &StratV5, out: &Path) -> Result<()> {
    let all = load_windows(recs)?;
    let data: Vec<WindowData> = all.iter().map(wd).collect::<Result<_>>()?;
    let legacy = crate::radar_feed::load_params(Path::new("config/radar_params_v4_legacy.json"))?;
    let start = crate::units::start_sol();
    let radar_variants: Vec<(String, RadarParams)> = vec![
        ("base".into(), rp),
        ("legacy_v4_radar".into(), legacy),
        ("no_cusum".into(), RadarParams { cusum: radar::CusumParams { enabled: false, ..rp.cusum }, ..rp }),
        ("no_flow_rule".into(), RadarParams { old_flow_rule: false, ..rp }),
        ("no_watch".into(), RadarParams { watch: false, ..rp }),
        ("old_median_block_1.0".into(), RadarParams { old_max_median_buy_sol: Some(1.0), ..rp }),
        ("no_old_buy_sell_cap".into(), RadarParams { old_max_buy_sell_ratio: None, ..rp }),
        ("wallets_not_clusters".into(), RadarParams { count_clusters: false, ..rp }),
        ("creator_counted".into(), RadarParams { exclude_creator: false, ..rp }),
        ("no_new_coin_age_limit".into(), RadarParams { new_max_age_s: None, ..rp }),
    ];
    let radar_rows: Vec<serde_json::Value> = radar_variants
        .par_iter()
        .map(|(n, p)| {
            let per: Vec<serde_json::Value> = all.iter().map(|l| {
                let ds = crate::radar_eval::from_events(&l.ev, p);
                let prep = crate::radar_eval::prepare(&ds);
                let e = crate::radar_eval::evaluate(&ds, &prep, *p);
                let t5 = |v: &[crate::radar_eval::TierResult]| v.iter().find(|t| t.tier == 5.0).map(|t| serde_json::json!({
                    "n": t.runners, "hot_before_10": t.caught_hot_before_10, "hot_at_30": t.caught, "armed_before_10": t.caught_armed_before_10 }));
                serde_json::json!({ "window": l.name, "hot_per_min": e.hot_per_min, "watch_per_min": e.watch_per_min, "precision_2x": e.precision_2x,
                    "naive_precision_2x": e.naive_precision_2x, "new_5x": t5(&e.tiers_newborn), "old_5x": t5(&e.tiers_existing),
                    "cusum_alarmed_before_10": e.cusum.alarmed_before_10, "cusum_runners_5x": e.cusum.runners_5x, "cusum_false_per_coin_hour": e.cusum.alarms_per_coin_hour })
            }).collect();
            serde_json::json!({ "variant": n, "windows": per })
        })
        .collect();
    use s5::ExitMode;
    let strat_variants: Vec<Variant> = vec![
        ("base".into(), rp, *st),
        ("exit_fade_and_drawdown".into(), rp, StratV5 { exit_mode: ExitMode::FadeAndDrawdown, ..*st }),
        ("exit_trail".into(), rp, StratV5 { exit_mode: ExitMode::Trail, ..*st }),
        ("exit_adaptive_trail".into(), rp, StratV5 { exit_mode: ExitMode::AdaptiveTrail, ..*st }),
        ("exit_ladder".into(), rp, StratV5 { exit_mode: ExitMode::Ladder, ..*st }),
        ("no_price_guard".into(), rp, StratV5 { max_entry_ret: None, ..*st }),
        ("promote_rows_only".into(), rp, StratV5 { decision_rows: false, ..*st }),
        ("tick_per_slot".into(), rp, StratV5 { decision_tick_s: 0, ..*st }),
        ("no_time_stop".into(), rp, StratV5 { time_stop_s: 0, ..*st }),
        ("time_stop_classic".into(), rp, StratV5 { time_stop_s: 300, time_stop_max_buyers60: f64::INFINITY, ..*st }),
        ("time_stop_flow_le1".into(), rp, StratV5 { time_stop_s: 300, time_stop_max_buyers60: 1.0, ..*st }),
        ("time_stop_flow_le3".into(), rp, StratV5 { time_stop_s: 300, time_stop_max_buyers60: 3.0, ..*st }),
        ("no_entries_per_min_cap".into(), rp, StratV5 { max_entries_per_min: 1_000_000, ..*st }),
        ("no_reentry_cooldown".into(), rp, StratV5 { reentry_cooldown_s: 0, ..*st }),
        ("no_mint_gate".into(), rp, StratV5 { mint_gate: false, ..*st }),
        ("risk_loss_limit_30pct".into(), rp, StratV5 { max_loss_frac: Some(0.30), ..*st }),
        ("risk_loss_streak_5".into(), rp, StratV5 { loss_streak: 5, ..*st }),
        ("entry_on_watch".into(), rp, StratV5 { entry_on_watch: true, ..*st }),
        ("c18_venues_open".into(), rp, StratV5 { c18_venues: true, ..*st }),
        ("compounding_1_18".into(), rp, StratV5 { max_positions: 18, bet_equity_frac: Some(1.0 / 18.0), max_total_exposure: 1.0, max_entries_per_min: 1_000_000, ..*st }),
        ("max_hold_5m".into(), rp, StratV5 { max_hold_s: 300, ..*st }),
        ("max_hold_10m".into(), rp, StratV5 { max_hold_s: 600, ..*st }),
        ("max_hold_20m".into(), rp, StratV5 { max_hold_s: 1200, ..*st }),
        ("max_hold_30m".into(), rp, StratV5 { max_hold_s: 1800, ..*st }),
        ("max_hold_60m".into(), rp, StratV5 { max_hold_s: 3600, ..*st }),
        ("trail_05pct".into(), rp, StratV5 { exit_mode: s5::ExitMode::Trail, trail_fixed: 0.05, ..*st }),
        ("trail_10pct".into(), rp, StratV5 { exit_mode: s5::ExitMode::Trail, trail_fixed: 0.10, ..*st }),
        ("trail_15pct".into(), rp, StratV5 { exit_mode: s5::ExitMode::Trail, trail_fixed: 0.15, ..*st }),
        ("trail_20pct".into(), rp, StratV5 { exit_mode: s5::ExitMode::Trail, trail_fixed: 0.20, ..*st }),
        ("trail_45pct".into(), rp, StratV5 { exit_mode: s5::ExitMode::Trail, trail_fixed: 0.45, ..*st }),
        ("c18_venues_open_pos18".into(), rp, StratV5 { c18_venues: true, max_positions: 18, bet_sol: start / 18.0, max_total_exposure: 1.0, max_entries_per_min: 1_000_000, ..*st }),
        // user variant: only a position-count limit; each position = start capital / max_positions (no Kelly, no all-in)
        ("only_position_limit_10".into(), rp, StratV5 { max_positions: 10, bet_sol: start / 10.0, max_total_exposure: 1.0, max_entries_per_min: 1_000_000, ..*st }),
        ("only_position_limit_18".into(), rp, StratV5 { max_positions: 18, bet_sol: start / 18.0, max_total_exposure: 1.0, max_entries_per_min: 1_000_000, ..*st }),
        ("only_position_limit_30".into(), rp, StratV5 { max_positions: 30, bet_sol: start / 30.0, max_total_exposure: 1.0, max_entries_per_min: 1_000_000, ..*st }),
        ("legacy_v4_radar".into(), legacy, *st),
    ];
    let strat_rows: Vec<serde_json::Value> = strat_variants
        .par_iter()
        .map(|(n, p, s)| {
            let c5 = CostsV5::default();
            let reps: Vec<WindowResult> = data.iter().map(|d| WindowResult {
                window: d.name.clone(), rk: [1.0; 7], rk_trades: [0; 7], hold: None,
                report: s5::simulate(d, *p, &mut s5::NoScores, s, &c5, &[1.0; 7], None, false),
            }).collect();
            let sum = summarize(&reps);
            serde_json::json!({ "variant": n, "summary": sum,
                "windows": reps.iter().map(|r| serde_json::json!({ "window": r.window, "pnl_sol": r.report.final_sol - r.report.start_sol,
                    "trades": r.report.trades, "held_to_5x": r.report.trades_list.iter().filter(|t| t.max_mult >= 5.0).count(),
                    "final_exits": r.report.final_reasons, "signals_in_window": r.report.signals_in_window, "blocked": r.report.blocked,
                    "failed_buys": r.report.failed_buys })).collect::<Vec<_>>() })
        })
        .collect();
    let rep = serde_json::json!({
        "label": "EXPLORATORY component ablation (paper only). Strategy rows run WITHOUT a model (direction only); a component is kept only if it improves out-of-sample expectancy after costs on most test windows.",
        "windows": all.iter().map(|l| &l.name).collect::<Vec<_>>(), "radar": radar_rows, "strategy": strat_rows,
    });
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(out, serde_json::to_string_pretty(&rep)?)?;
    println!("written {}", out.display());
    Ok(())
}
