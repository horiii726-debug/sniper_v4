//! Accounting units. Every simulation accounts in SOL; nothing is fetched from an exchange (the system only reads
//! the chain). Starting capital is `--start-sol`. USD values in reports are display-only: they use the rate the user
//! passes with `--sol-usd`, and are NaN (JSON `null`) when no rate is given.

use std::sync::OnceLock;

pub const DEFAULT_START_SOL: f64 = 1.0;

#[derive(Debug, Clone, Copy)]
pub struct Units {
    pub start_sol: f64,
    pub sol_usd: Option<f64>,
}

static UNITS: OnceLock<Units> = OnceLock::new();

/// Set once from the CLI (later calls are ignored).
pub fn set(u: Units) {
    let _ = UNITS.set(u);
}

fn get() -> Units {
    *UNITS.get_or_init(|| Units { start_sol: DEFAULT_START_SOL, sol_usd: None })
}

/// Starting capital of every simulated account, SOL.
pub fn start_sol() -> f64 {
    get().start_sol
}

/// Display-only SOL/USD rate given by the user; NaN when absent (never fetched).
pub fn sol_usd() -> f64 {
    get().sol_usd.unwrap_or(f64::NAN)
}

/// Optional `<rec>/sol_usd_1m.csv` (`close_time_unix_s,close_usd` per line), used only by the `sol_ret_1h` regime
/// feature. Absent file -> empty series (the feature is NaN). Never downloaded.
pub fn read_sol_usd_series(rec: &std::path::Path) -> anyhow::Result<Vec<(i64, f64)>> {
    let p = rec.join("sol_usd_1m.csv");
    if !p.exists() {
        return Ok(Vec::new());
    }
    let mut v: Vec<(i64, f64)> = std::fs::read_to_string(&p)?
        .lines()
        .filter_map(|l| {
            let (a, b) = l.split_once(',')?;
            let (t, x) = (a.trim().parse::<i64>().ok()?, b.trim().parse::<f64>().ok()?);
            (x.is_finite() && x > 0.0).then_some((t, x))
        })
        .collect();
    v.sort_by_key(|x| x.0);
    v.dedup_by_key(|x| x.0);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_file_optional_sorted_and_filtered() {
        let d = std::env::temp_dir().join(format!("units_test_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(read_sol_usd_series(&d).unwrap().is_empty());
        std::fs::write(d.join("sol_usd_1m.csv"), "120,2.0\n60,1.0\nbad\n180,NaN\n240,-1\n60,9\n").unwrap();
        assert_eq!(read_sol_usd_series(&d).unwrap(), vec![(60, 1.0), (120, 2.0)]);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
