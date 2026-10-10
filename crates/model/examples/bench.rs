//! Per-row inference latency on the parity fixtures (python3 python/ml/model_parity.py first).
//! cargo run --release -p model --example bench [-- <fixture_dir>]
use model::{FeatureMap, Model};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let d = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/model_parity")
    });
    let text = std::fs::read_to_string(d.join("rows.csv")).expect("rows.csv (run python/ml/model_parity.py)");
    let mut lines = text.lines();
    let avail: Vec<&str> = lines.next().unwrap().split(',').collect();
    let rows: Vec<Vec<f32>> = lines
        .filter(|l| !l.is_empty())
        .map(|l| l.split(',').map(|s| if s == "nan" { f32::NAN } else { s.parse().unwrap() }).collect())
        .collect();
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    for name in ["binary", "multiclass", "binary_nanmax"] {
        if !d.join(format!("{name}.cbm.json")).exists() {
            continue;
        }
        let t = Instant::now();
        let m = Model::load(&d.join(name)).unwrap();
        let load_ms = t.elapsed().as_secs_f64() * 1e3;
        let fm = FeatureMap::for_model(&m, &avail).unwrap();
        // model-order rows prepared up front; gather timed separately
        let xs: Vec<Vec<f32>> = rows
            .iter()
            .map(|r| {
                let mut x = Vec::new();
                fm.gather(r, &mut x);
                x
            })
            .collect();
        let mut out = vec![0.0; m.n_out()];
        let mut sink = 0.0;
        for x in xs.iter().take(2000) {
            m.predict_proba_into(x, &mut out);
            sink += out[0];
        }
        let passes = 5;
        let mut ns = Vec::with_capacity(xs.len() * passes);
        for _ in 0..passes {
            for x in &xs {
                let t = Instant::now();
                m.predict_proba_into(x, &mut out);
                ns.push(t.elapsed().as_nanos() as u64);
                sink += out[0];
            }
        }
        ns.sort_unstable();
        let q = |p: f64| ns[((ns.len() - 1) as f64 * p) as usize] as f64 / 1e3;
        // gather cost
        let mut x = Vec::with_capacity(m.features().len());
        let t = Instant::now();
        for r in &rows {
            fm.gather(r, &mut x);
            sink += x[0] as f64;
        }
        let gather_ns = t.elapsed().as_nanos() as f64 / rows.len() as f64;
        // multi-thread throughput (independent rows, shared &Model)
        let t = Instant::now();
        let reps = 20;
        std::thread::scope(|s| {
            for tid in 0..threads {
                let (m, xs) = (&m, &xs);
                s.spawn(move || {
                    let mut out = vec![0.0; m.n_out()];
                    let mut acc = 0.0;
                    for _ in 0..reps {
                        for x in xs.iter().skip(tid).step_by(threads) {
                            m.predict_proba_into(x, &mut out);
                            acc += out[0];
                        }
                    }
                    std::hint::black_box(acc);
                });
            }
        });
        let mt_rows_s = (xs.len() * reps) as f64 / t.elapsed().as_secs_f64();
        println!(
            "{name:14} trees={:4} n_out={} feats={:3} load={load_ms:6.1}ms  predict_proba/row: p50={:6.2}us p99={:6.2}us max={:7.2}us mean={:6.2}us  gather={gather_ns:.0}ns  {threads}-thread throughput={:.2}M rows/s",
            m.n_trees(),
            m.n_out(),
            m.features().len(),
            q(0.50),
            q(0.99),
            q(1.0),
            ns.iter().sum::<u64>() as f64 / ns.len() as f64 / 1e3,
            mt_rows_s / 1e6
        );
        std::hint::black_box(sink);
    }
}
