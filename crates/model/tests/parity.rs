//! Rust vs Python parity on fixtures written by `python3 python/ml/model_parity.py` into
//! target/model_parity/ (not committed). Passes with a note if the fixtures are absent.
use model::{FeatureMap, Model};
use std::path::{Path, PathBuf};

const TOL: f64 = 1e-6;

fn dir() -> PathBuf {
    std::env::var_os("MODEL_PARITY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/model_parity"))
}

fn parse(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("nan") {
        f64::NAN
    } else {
        s.parse().unwrap_or_else(|e| panic!("bad number {s:?}: {e}"))
    }
}

fn read_csv(path: &Path) -> (Vec<String>, Vec<Vec<String>>) {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut lines = text.lines();
    let header = lines.next().unwrap().split(',').map(str::to_string).collect();
    let rows = lines.filter(|l| !l.is_empty()).map(|l| l.split(',').map(str::to_string).collect()).collect();
    (header, rows)
}

#[test]
fn parity_with_python() {
    let d = dir();
    let rows_path = d.join("rows.csv");
    if !rows_path.exists() {
        eprintln!("NOTE: {} not found; run python3 python/ml/model_parity.py. Skipping parity.", rows_path.display());
        return;
    }
    let (avail, rows) = read_csv(&rows_path);
    // f32 parse straight from the 9-significant-digit text = the exact float32 Python fed CatBoost
    let rows: Vec<Vec<f32>> = rows
        .iter()
        .map(|r| {
            r.iter()
                .map(|s| {
                    let s = s.trim();
                    if s.eq_ignore_ascii_case("nan") { f32::NAN } else { s.parse::<f32>().unwrap() }
                })
                .collect()
        })
        .collect();
    assert!(rows.len() >= 10_000, "expected 10,000 parity rows, got {}", rows.len());
    let mut n_models = 0;
    for name in ["binary", "multiclass", "binary_nanmax"] {
        let prefix = d.join(name);
        if !d.join(format!("{name}.cbm.json")).exists() {
            assert!(name == "binary_nanmax", "missing fixture model {name}");
            continue;
        }
        let m = Model::load(&prefix).unwrap();
        let fm = FeatureMap::for_model(&m, &avail).unwrap();
        let (hdr, preds) = read_csv(&d.join(format!("{name}.preds.csv")));
        let k = m.n_out();
        assert_eq!(hdr.len(), 2 * k);
        assert_eq!(preds.len(), rows.len());
        let (mut max_raw, mut max_cal) = (0.0f64, 0.0f64);
        let mut x = Vec::new();
        let mut raw = vec![0.0; k];
        let mut cal = vec![0.0; k];
        for (row, pr) in rows.iter().zip(&preds) {
            fm.gather(row, &mut x);
            m.predict_raw_into(&x, &mut raw);
            m.predict_proba_into(&x, &mut cal);
            for c in 0..k {
                max_raw = max_raw.max((raw[c] - parse(&pr[c])).abs());
                max_cal = max_cal.max((cal[c] - parse(&pr[k + c])).abs());
            }
        }
        println!(
            "parity {name}: rows={} trees={} features={} n_out={k}  max|raw diff|={max_raw:.3e}  max|cal diff|={max_cal:.3e}",
            rows.len(),
            m.n_trees(),
            m.features().len()
        );
        assert!(max_raw.is_finite() && max_raw <= TOL, "{name}: raw max abs diff {max_raw}");
        assert!(max_cal.is_finite() && max_cal <= TOL, "{name}: calibrated max abs diff {max_cal}");
        n_models += 1;
    }
    assert!(n_models >= 2);
}
