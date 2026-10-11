//! V5-9 diagnostic (no label change): how often do runners touch the -65% `y_bin` barrier (and wider barriers)
//! before their peak? Rows = in-window radar Promote rows (the V5 decision rows). Per window and pooled, split into
//! rows whose 10 h label horizon is covered by data (`covered`) and all rows (`all_censored`, direction only).

use crate::backtest::Events;
use crate::ml_sim::{self, BarrierPath, Engine, BARRIERS_DIAG};
use anyhow::Result;
use radar::Params as RadarParams;
use rustc_hash::FxHashSet;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub struct Row {
    pub first_signal: bool,
    pub covered: bool,
    pub b: BarrierPath,
}

pub fn rows(ev: &Events, rp: RadarParams) -> Vec<Row> {
    let paths = ml_sim::paths(ev);
    let deaths = ml_sim::death_times(ev);
    let mut eng = Engine::new(ev, rp);
    let mut seen: FxHashSet<u32> = FxHashSet::default();
    let mut out = Vec::new();
    for e in &ev.evs {
        if !eng.apply(e).signal || e.time > ev.window.1 {
            continue;
        }
        let ci = e.coin as usize;
        let Some(b) = ml_sim::barrier_path(&paths[ci], e.time, ev.cover_end[ci]) else { continue };
        out.push(Row { first_signal: seen.insert(e.coin), covered: ml_sim::label_covered(e.time, &deaths[ci], ev.cover_end[ci]), b });
    }
    out
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Tier {
    pub min_peak: f64,
    pub n: usize,
    /// fewer than 30 runners: direction only, not evidence
    pub direction_only: bool,
    pub touched_before_peak: [usize; 4],
    pub share_touched: [f64; 4],
    /// runners whose peak before barrier k is still >= min_peak
    pub keeps_tier: [usize; 4],
    pub min_before_peak_p10_p25_p50: [f64; 3],
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Summary {
    pub barriers: [f64; 4],
    pub rows: usize,
    pub tiers: Vec<Tier>,
    /// y_bin class mix (7 bins) under each barrier, all rows
    pub bin_mix_by_barrier: [[usize; 7]; 4],
}

pub fn summarize(rows: &[&Row]) -> Summary {
    let mut mix = [[0usize; 7]; 4];
    for r in rows {
        for k in 0..4 {
            mix[k][ml_sim::bin_of(r.b.peak_before[k]) as usize] += 1;
        }
    }
    let tiers = [10.0, 5.0]
        .iter()
        .map(|&mp| {
            let run: Vec<&&Row> = rows.iter().filter(|r| r.b.peak >= mp).collect();
            let n = run.len();
            let mut touched = [0usize; 4];
            let mut keeps = [0usize; 4];
            for r in &run {
                for k in 0..4 {
                    touched[k] += (r.b.min_before_peak <= BARRIERS_DIAG[k]) as usize;
                    keeps[k] += (r.b.peak_before[k] >= mp) as usize;
                }
            }
            let mut mins: Vec<f64> = run.iter().map(|r| r.b.min_before_peak).collect();
            mins.sort_by(|a, b| a.total_cmp(b));
            let q = |f: f64| if mins.is_empty() { f64::NAN } else { mins[((mins.len() - 1) as f64 * f).round() as usize] };
            Tier {
                min_peak: mp,
                n,
                direction_only: n < 30,
                touched_before_peak: touched,
                share_touched: touched.map(|x| if n > 0 { x as f64 / n as f64 } else { f64::NAN }),
                keeps_tier: keeps,
                min_before_peak_p10_p25_p50: [q(0.1), q(0.25), q(0.5)],
            }
        })
        .collect();
    Summary { barriers: BARRIERS_DIAG, rows: rows.len(), tiers, bin_mix_by_barrier: mix }
}

fn block(rows: &[Row]) -> serde_json::Value {
    let pick = |cov: bool| {
        let all: Vec<&Row> = rows.iter().filter(|r| !cov || r.covered).collect();
        let first: Vec<&Row> = all.iter().copied().filter(|r| r.first_signal).collect();
        serde_json::json!({ "first_signal_coins": summarize(&first), "rows": summarize(&all) })
    };
    serde_json::json!({ "covered": pick(true), "all_censored": pick(false) })
}

pub fn run(recs: &[PathBuf], rp: RadarParams, out: &Path) -> Result<()> {
    use rayon::prelude::*;
    let per: Vec<(String, Vec<Row>)> = recs
        .par_iter()
        .map(|r| -> Result<(String, Vec<Row>)> { Ok((crate::v5_eval::dir_name(r), rows(&crate::backtest::load(r)?, rp))) })
        .collect::<Result<_>>()?;
    let pooled: Vec<Row> = per.iter().flat_map(|(_, v)| v.iter().copied()).collect();
    let rep = serde_json::json!({
        "label": "V5-9 barrier diagnostic (no label change). Decision rule fixed before results: keep -65% if < 10% of covered >=10x first-signal runners touch it before their peak; otherwise propose the smallest tested barrier keeping >= 90% of them in bin >= 5 (new appended label, y_bin unchanged).",
        "windows": per.iter().map(|(n, v)| serde_json::json!({ "window": n, "summary": block(v) })).collect::<Vec<_>>(),
        "pooled": block(&pooled),
    });
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(out, serde_json::to_string_pretty(&rep)?)?;
    println!("{}", serde_json::to_string_pretty(&rep["pooled"]["covered"]["first_signal_coins"])?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(peak: f64, mbp: f64, pb: [f64; 4], covered: bool) -> Row {
        Row { first_signal: true, covered, b: BarrierPath { peak, t_peak: 10, min_before_peak: mbp, peak_before: pb } }
    }

    #[test]
    fn label_diag_summary_counts() {
        let rs = [
            row(12.0, 0.3, [0.3, 12.0, 12.0, 12.0], true), // touched -65% before peak; kept by wider barriers
            row(20.0, 0.8, [20.0; 4], true),
            row(6.0, 0.2, [0.2, 0.2, 6.0, 6.0], false),
            row(1.5, 1.0, [1.5; 4], true),
        ];
        let refs: Vec<&Row> = rs.iter().collect();
        let s = summarize(&refs);
        assert_eq!(s.rows, 4);
        let t10 = &s.tiers[0];
        assert_eq!((t10.n, t10.direction_only), (2, true));
        assert_eq!(t10.touched_before_peak, [1, 0, 0, 0]);
        assert_eq!(t10.keeps_tier, [1, 2, 2, 2]);
        let t5 = &s.tiers[1];
        assert_eq!(t5.n, 3);
        assert_eq!(t5.touched_before_peak, [2, 1, 0, 0]);
        assert_eq!(t5.keeps_tier, [1, 2, 3, 3]);
        assert_eq!(s.bin_mix_by_barrier[0], [2, 0, 1, 0, 0, 1, 0]);
    }
}
