//! Hand-written tiny CatBoost JSON models (2 trees of depth 2 / 1 multiclass tree, NaN features).
//! Expected raw values were derived by hand and confirmed with catboost 1.2.10:
//!   CatBoost().load_model("tiny.cbm.json", format="json").predict(X, prediction_type="RawFormulaVal")
//!   tiny         -> [0.6, 1.2, 1.8, -0.9, 1.8]
//!   tiny_nantrue -> [0.6, 1.2, 0.45, -0.9, 1.8]
//!   tiny_mc      -> [[1,.125,-1.25],[.5,.625,-.25],[0,2.125,-.25],[-.5,.125,0],[.5,.625,-.25]]
//!
//! tiny: f0 borders AsIs, f1 has NaNs (AsFalse = nan_mode Min). scale 2, bias -0.1.
//!   tree0 splits: bit0 = f0 > 0.5, bit1 = f1 > 1.0,  leaves [0.1, 0.2, 0.3, 0.4]
//!   tree1 splits: bit0 = f1 > -1,  bit1 = f0 > 2.0,  leaves [-0.5, 0.25, 0.75, -0.125]
//!   (0,0):     t0 idx0 0.1 + t1 idx1 0.25 = 0.35 -> 2*0.35-0.1 = 0.6
//!   (1,2):     t0 idx3 0.4 + t1 idx1 0.25 = 0.65 -> 1.2
//!   (3,NaN):   AsFalse: t0 idx1 0.2 + t1 idx2 0.75 = 0.95 -> 1.8 ; AsTrue: t0 idx3 0.4 + t1 idx3 -0.125 -> 0.45
//!   (0.5,-1):  strict '>' at the borders: t0 idx0 0.1 + t1 idx0 -0.5 = -0.4 -> -0.9
//!   (2.5,-3):  t0 idx1 0.2 + t1 idx2 0.75 -> 1.8
use model::{FeatureMap, Kind, Model};
use std::path::PathBuf;

fn p(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
}

fn sig(z: f64) -> f64 {
    1.0 / (1.0 + (-z).exp())
}

const ROWS: [[f32; 2]; 5] = [[0.0, 0.0], [1.0, 2.0], [3.0, f32::NAN], [0.5, -1.0], [2.5, -3.0]];

fn close(a: f64, b: f64) {
    assert!((a - b).abs() <= 1e-12, "{a} vs {b}");
}

#[test]
fn tiny_binary_raw_and_calibrated() {
    let m = Model::load(&p("tiny")).unwrap();
    assert_eq!(m.kind(), Kind::Binary);
    assert_eq!(m.n_out(), 1);
    assert_eq!(m.features(), &["f0".to_string(), "f1".to_string()]);
    assert!(m.is_calibrated());
    let want = [0.6, 1.2, 1.8, -0.9, 1.8];
    // iso table x=[0.5,0.75,0.875] y=[0,0.5,0.75]
    let iso = |q: f64| {
        if q <= 0.5 {
            0.0
        } else if q <= 0.75 {
            (q - 0.5) * 2.0
        } else if q <= 0.875 {
            0.5 + (q - 0.75) * 2.0
        } else {
            0.75
        }
    };
    let mut buf = [0.0f64; 1];
    for (r, w) in ROWS.iter().zip(want) {
        close(m.predict_raw(r)[0], w);
        close(m.predict_proba(r)[0], iso(sig(w)));
        m.predict_proba_uncalibrated_into(r, &mut buf);
        close(buf[0], sig(w));
    }
    // spot values: sigmoid(0.6)=0.645656.. -> 0.291312..; sigmoid(-0.9)=0.289 -> 0
    assert!((m.predict_proba(&ROWS[0])[0] - 0.2913126124515908).abs() < 1e-12);
    assert_eq!(m.predict_proba(&ROWS[3])[0], 0.0);
}

#[test]
fn tiny_binary_nan_as_true() {
    let m = Model::load(&p("tiny_nantrue")).unwrap();
    assert!(!m.is_calibrated());
    let want = [0.6, 1.2, 0.45, -0.9, 1.8];
    for (r, w) in ROWS.iter().zip(want) {
        close(m.predict_raw(r)[0], w);
        close(m.predict_proba(r)[0], sig(w));
    }
}

#[test]
fn tiny_multiclass() {
    let m = Model::load(&p("tiny_mc")).unwrap();
    assert_eq!(m.kind(), Kind::Multiclass);
    assert_eq!(m.n_out(), 3);
    let rows: [[f32; 2]; 5] = [[0.0, 0.0], [0.0, 1.0], [1.0, 0.0], [1.0, 1.0], [f32::NAN, 1.0]];
    let want = [
        [1.0, 0.125, -1.25],
        [0.5, 0.625, -0.25],
        [0.0, 2.125, -0.25],
        [-0.5, 0.125, 0.0],
        [0.5, 0.625, -0.25],
    ];
    for (r, w) in rows.iter().zip(want) {
        let raw = m.predict_raw(r);
        for k in 0..3 {
            close(raw[k], w[k]);
        }
        let e: Vec<f64> = w.iter().map(|v| v.exp()).collect();
        let s: f64 = e.iter().sum();
        let sm: Vec<f64> = e.iter().map(|v| v / s).collect();
        // iso: class0 identity, class1 0.5*p, class2 clamp((p-0.5)*2, 0, 1); then renormalize
        let c = [sm[0], 0.5 * sm[1], ((sm[2] - 0.5) * 2.0).clamp(0.0, 1.0)];
        let cs: f64 = c.iter().sum();
        let pr = m.predict_proba(r);
        for k in 0..3 {
            close(pr[k], c[k] / cs);
        }
        close(pr.iter().sum::<f64>(), 1.0);
    }
}

#[test]
fn multiclass_zero_sum_falls_back_to_softmax() {
    let cbm = std::fs::read_to_string(p("tiny_mc.cbm.json")).unwrap();
    let meta = std::fs::read_to_string(p("tiny_mc.meta.json")).unwrap();
    let iso = r#"{"classes":[{"x":[0,1],"y":[0,0]},{"x":[0,1],"y":[0,0]},{"x":[0,1],"y":[0,0]}]}"#;
    let m = Model::from_json(&cbm, &meta, Some(iso)).unwrap();
    let pr = m.predict_proba(&[0.0, 0.0]);
    let e = [1.0f64.exp(), 0.125f64.exp(), (-1.25f64).exp()];
    let s: f64 = e.iter().sum();
    for k in 0..3 {
        close(pr[k], e[k] / s);
    }
}

#[test]
fn errors() {
    let cbm = std::fs::read_to_string(p("tiny.cbm.json")).unwrap();
    let meta = std::fs::read_to_string(p("tiny.meta.json")).unwrap();
    // calibrated without iso
    assert!(Model::from_json(&cbm, &meta, None).unwrap_err().contains("iso"));
    // feature name mismatch with model feature_id
    let bad = meta.replace("\"f1\"", "\"zz\"");
    assert!(Model::from_json(&cbm, &bad, Some("{\"x\":[0],\"y\":[0]}")).unwrap_err().contains("mismatch"));
    // categorical features
    let cat = cbm.replace("\"float_features\"", "\"categorical_features\": [{\"feature_index\": 0}], \"float_features\"");
    assert!(Model::from_json(&cat, &meta, Some("{\"x\":[0],\"y\":[0]}")).is_err());
    // non-oblivious trees
    let nos = cbm.replace("\"oblivious_trees\"", "\"trees\": [], \"oblivious_trees\"");
    assert!(Model::from_json(&nos, &meta, Some("{\"x\":[0],\"y\":[0]}")).is_err());
    // missing feature in FeatureMap
    let m = Model::load(&p("tiny")).unwrap();
    assert!(FeatureMap::for_model(&m, &["f0"]).is_err());
    let fm = FeatureMap::for_model(&m, &["x", "f1", "f0"]).unwrap();
    let mut row = Vec::new();
    fm.gather(&[9.0, 2.0, 1.0], &mut row);
    close(m.predict_raw(&row)[0], 1.2);
}
