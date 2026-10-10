//! Regression tests for loader validation (review findings): unsupported loss functions,
//! meta/feature count and duplicate checks, malformed indices (must be Err, never panic),
//! and ambiguous columns in FeatureMap.
use model::{FeatureMap, Model};

/// Depth-1 single-tree model over `n` float features; `loss` = Some(type) adds model_info.params.
fn cbm(n: usize, dim: usize, loss: Option<&str>, ids: bool) -> String {
    let ff: Vec<String> = (0..n)
        .map(|i| {
            let id = if ids { format!("f{i}") } else { String::new() };
            format!(r#"{{"feature_index":{i},"flat_feature_index":{i},"feature_id":"{id}","borders":[0.5],"nan_value_treatment":"AsIs"}}"#)
        })
        .collect();
    let leaves: Vec<String> = (0..2 * dim).map(|i| format!("{}", i as f64 * 0.25 - 0.3)).collect();
    let mi = match loss {
        Some(l) => format!(r#","model_info":{{"params":{{"loss_function":{{"type":"{l}","params":{{}}}}}}}}"#),
        None => String::new(),
    };
    let bias = if dim == 1 { "0.0".to_string() } else { format!("[{}]", vec!["0.0"; dim].join(",")) };
    format!(
        r#"{{"features_info":{{"float_features":[{}]}},"oblivious_trees":[{{"leaf_values":[{}],"splits":[{{"border":0.5,"float_feature_index":0,"split_type":"FloatFeature"}}]}}],"scale_and_bias":[1.0,{bias}]{mi}}}"#,
        ff.join(","),
        leaves.join(",")
    )
}

fn meta(kind: &str, n_classes: usize, feats: &[&str]) -> String {
    let f: Vec<String> = feats.iter().map(|s| format!("\"{s}\"")).collect();
    format!(r#"{{"name":"t","kind":"{kind}","n_classes":{n_classes},"features":[{}]}}"#, f.join(","))
}

#[test]
fn multiclass_one_vs_all_is_rejected() {
    let e = Model::from_json(&cbm(2, 3, Some("MultiClassOneVsAll"), true), &meta("multiclass", 3, &["f0", "f1"]), None)
        .unwrap_err();
    assert!(e.contains("MultiClassOneVsAll"), "{e}");
}

#[test]
fn regression_and_other_losses_are_rejected() {
    for l in ["RMSE", "Quantile", "MultiClass", "YetiRank"] {
        let e = Model::from_json(&cbm(2, 1, Some(l), true), &meta("binary", 2, &["f0", "f1"]), None).unwrap_err();
        assert!(e.contains("loss_function"), "{l}: {e}");
    }
    for l in ["Logloss", "RMSE", "MultiClassOneVsAll"] {
        assert!(Model::from_json(&cbm(2, 3, Some(l), true), &meta("multiclass", 3, &["f0", "f1"]), None).is_err(), "{l}");
    }
}

#[test]
fn supported_losses_and_missing_loss_load() {
    for l in [Some("Logloss"), Some("CrossEntropy"), None] {
        let m = Model::from_json(&cbm(2, 1, l, true), &meta("binary", 2, &["f0", "f1"]), None).unwrap();
        assert_eq!(m.loss_function(), l);
    }
    let m = Model::from_json(&cbm(2, 3, Some("MultiClass"), true), &meta("multiclass", 3, &["f0", "f1"]), None).unwrap();
    assert_eq!(m.loss_function(), Some("MultiClass"));
    // params stored as a JSON string and the "Name:opt=v" short form are understood too
    let s = cbm(2, 1, None, true).replacen(
        r#""scale_and_bias""#,
        r#""model_info":{"params":"{\"loss_function\":\"Logloss:border=0.5\"}"},"scale_and_bias""#,
        1,
    );
    assert_eq!(Model::from_json(&s, &meta("binary", 2, &["f0", "f1"]), None).unwrap().loss_function(), Some("Logloss"));
}

#[test]
fn meta_feature_count_must_match_model() {
    // extra name in meta (numpy-trained model: no feature ids)
    let e = Model::from_json(&cbm(2, 1, Some("Logloss"), false), &meta("binary", 2, &["a", "b", "extra"]), None)
        .unwrap_err();
    assert!(e.contains("2 float features") && e.contains("3 features"), "{e}");
    // too few names
    assert!(Model::from_json(&cbm(3, 1, None, false), &meta("binary", 2, &["a", "b"]), None).is_err());
    // exact count loads; names cannot be verified without feature ids
    let m = Model::from_json(&cbm(2, 1, None, false), &meta("binary", 2, &["a", "b"]), None).unwrap();
    assert_eq!(m.features().len(), 2);
    assert!(!m.feature_names_verified());
    // with ids the order is verified, so a permuted meta fails
    assert!(Model::from_json(&cbm(2, 1, None, true), &meta("binary", 2, &["f0", "f1"]), None).unwrap().feature_names_verified());
    assert!(Model::from_json(&cbm(2, 1, None, true), &meta("binary", 2, &["f1", "f0"]), None).is_err());
}

#[test]
fn duplicate_meta_features_rejected() {
    let e = Model::from_json(&cbm(2, 1, None, false), &meta("binary", 2, &["a", "a"]), None).unwrap_err();
    assert!(e.contains("more than once"), "{e}");
}

#[test]
fn malformed_indices_error_instead_of_panic() {
    let m1 = meta("binary", 2, &["a"]);
    for fi in ["18446744073709551615", "2305843009213693952", "1"] {
        let j = format!(
            r#"{{"features_info":{{"float_features":[{{"feature_index":{fi},"flat_feature_index":0}}]}},"oblivious_trees":[],"scale_and_bias":[1,[0]]}}"#
        );
        let r = std::panic::catch_unwind(|| Model::from_json(&j, &m1, None));
        let r = r.unwrap_or_else(|_| panic!("feature_index {fi} panicked"));
        assert!(r.unwrap_err().contains("out of range"), "{fi}");
    }
    // duplicate flat index
    let j = r#"{"features_info":{"float_features":[{"feature_index":0,"flat_feature_index":0},{"feature_index":1,"flat_feature_index":0}]},"oblivious_trees":[],"scale_and_bias":[1,[0]]}"#;
    assert!(Model::from_json(j, &meta("binary", 2, &["a", "b"]), None).unwrap_err().contains("flat_feature_index"));
    // flat index past meta
    let j = r#"{"features_info":{"float_features":[{"feature_index":0,"flat_feature_index":18446744073709551615}]},"oblivious_trees":[],"scale_and_bias":[1,[0]]}"#;
    assert!(Model::from_json(j, &m1, None).is_err());
    // split referencing an unknown float feature
    let j = r#"{"features_info":{"float_features":[{"feature_index":0,"flat_feature_index":0}]},"oblivious_trees":[{"leaf_values":[0,1],"splits":[{"border":0.5,"float_feature_index":18446744073709551615}]}],"scale_and_bias":[1,[0]]}"#;
    assert!(Model::from_json(j, &m1, None).is_err());
}

#[test]
fn feature_map_rejects_ambiguous_columns() {
    let mf = vec!["a".to_string()];
    let e = FeatureMap::new(&mf, &["a", "b", "a"]).unwrap_err();
    assert!(e.contains("ambiguous") && e.contains('a'), "{e}");
    // a repeated column the model does not need is harmless
    let fm = FeatureMap::new(&mf, &["a", "b", "b"]).unwrap();
    let mut out = Vec::new();
    fm.gather(&[1.0, 2.0, 3.0], &mut out);
    assert_eq!(out, vec![1.0]);
    // repeated model feature names
    assert!(FeatureMap::new(&["a".to_string(), "a".to_string()], &["a", "b"]).is_err());
}
