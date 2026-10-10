//! Rust inference for CatBoost models exported with `save_model(P + ".cbm.json", format="json")`.
//!
//! Model files share a prefix `P`:
//! - `P.cbm.json`  CatBoost JSON model (float features, oblivious trees only)
//! - `P.meta.json` `{"name","kind":"binary"|"multiclass","n_classes","features":[..],"target","train_windows","calibrated"}`
//! - `P.iso.json`  isotonic tables (required when `calibrated` is true, ignored otherwise):
//!   binary `{"x":[..],"y":[..]}`, multiclass `{"classes":[{"x","y"},..]}`
//!
//! Evaluation matches CatBoost `RawFormulaVal`: for every tree, bit `j` of the leaf index is
//! `x[feature(split_j)] > border_j` (split order as listed in the JSON, first split = bit 0);
//! leaf values are summed, then `scale * sum + bias`. Comparisons are done in f32 like CatBoost.
//! NaN: `AsIs` / `AsFalse` -> the comparison is false (CatBoost `nan_mode=Min`), `AsTrue` -> true (`Max`).

use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

/// Output kind of a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Binary,
    Multiclass,
}

#[derive(Debug, Clone, Copy)]
struct Split {
    feat: u32,
    border: f32,
}

#[derive(Debug, Clone, Copy)]
struct Tree {
    split_off: u32,
    depth: u32,
    leaf_off: u32,
}

/// Piecewise-linear table, increasing x, clamped at both ends.
#[derive(Debug, Clone)]
pub struct IsoTable {
    x: Vec<f64>,
    y: Vec<f64>,
}

impl IsoTable {
    pub fn new(x: Vec<f64>, y: Vec<f64>) -> Result<IsoTable, String> {
        if x.is_empty() || x.len() != y.len() {
            return Err(format!("isotonic table: len(x)={} len(y)={}", x.len(), y.len()));
        }
        if x.iter().chain(y.iter()).any(|v| !v.is_finite()) {
            return Err("isotonic table: non-finite value".into());
        }
        if x.windows(2).any(|w| w[1] < w[0]) {
            return Err("isotonic table: x not increasing".into());
        }
        Ok(IsoTable { x, y })
    }

    /// Same as numpy.interp / sklearn IsotonicRegression(out_of_bounds="clip").predict.
    #[inline]
    pub fn apply(&self, p: f64) -> f64 {
        let (x, y) = (&self.x, &self.y);
        let n = x.len();
        if p.is_nan() {
            return p;
        }
        if p <= x[0] {
            return y[0];
        }
        if p >= x[n - 1] {
            return y[n - 1];
        }
        // first index with x[i] > p; 1 <= i <= n-1
        let i = x.partition_point(|&v| v <= p);
        let (x0, x1, y0, y1) = (x[i - 1], x[i], y[i - 1], y[i]);
        if x1 <= x0 {
            return y1;
        }
        y0 + (p - x0) * (y1 - y0) / (x1 - x0)
    }
}

/// A loaded CatBoost model plus metadata and optional calibration. `Send + Sync`, immutable after load.
#[derive(Debug, Clone)]
pub struct Model {
    name: String,
    target: String,
    kind: Kind,
    n_classes: usize,
    /// leaf dimension: 1 for binary, n_classes for multiclass
    dim: usize,
    features: Vec<String>,
    /// every float feature in the cbm carries a feature_id that matched meta.features
    names_verified: bool,
    /// model_info.params.loss_function.type, when the cbm records it
    loss: Option<String>,
    train_windows: Vec<Value>,
    splits: Vec<Split>,
    /// per split: NaN goes right (AsTrue). Empty when no split needs it.
    nan_true: Vec<bool>,
    trees: Vec<Tree>,
    /// depth shared by every tree (0 = mixed); enables the unrolled binary path
    uniform_depth: usize,
    leaves: Vec<f64>,
    scale: f64,
    bias: Vec<f64>,
    iso: Vec<IsoTable>,
}

// ---------------------------------------------------------------- JSON schema

#[derive(Deserialize)]
struct CbModel {
    features_info: CbFeaturesInfo,
    oblivious_trees: Option<Vec<CbTree>>,
    #[serde(default)]
    scale_and_bias: Option<Value>,
    #[serde(default)]
    ctr_data: Option<Value>,
    #[serde(default)]
    trees: Option<Value>,
    #[serde(default)]
    model_info: Option<Value>,
}

#[derive(Deserialize)]
struct CbFeaturesInfo {
    #[serde(default)]
    float_features: Vec<CbFloatFeature>,
    #[serde(default)]
    categorical_features: Option<Vec<Value>>,
    #[serde(default)]
    text_features: Option<Vec<Value>>,
    #[serde(default)]
    embedding_features: Option<Vec<Value>>,
}

#[derive(Deserialize)]
struct CbFloatFeature {
    feature_index: usize,
    flat_feature_index: usize,
    #[serde(default)]
    feature_id: String,
    #[serde(default)]
    nan_value_treatment: Option<String>,
}

#[derive(Deserialize)]
struct CbTree {
    leaf_values: Vec<f64>,
    splits: Vec<CbSplit>,
}

#[derive(Deserialize)]
struct CbSplit {
    #[serde(default)]
    border: Option<f64>,
    #[serde(default)]
    float_feature_index: Option<usize>,
    #[serde(default)]
    split_type: Option<String>,
}

#[derive(Deserialize)]
struct Meta {
    name: String,
    kind: String,
    n_classes: usize,
    features: Vec<String>,
    #[serde(default)]
    target: String,
    #[serde(default)]
    train_windows: Vec<Value>,
    #[serde(default)]
    calibrated: bool,
}

#[derive(Deserialize)]
struct IsoJson {
    #[serde(default)]
    x: Option<Vec<f64>>,
    #[serde(default)]
    y: Option<Vec<f64>>,
    #[serde(default)]
    classes: Option<Vec<IsoXY>>,
}

#[derive(Deserialize)]
struct IsoXY {
    x: Vec<f64>,
    y: Vec<f64>,
}

fn with_suffix(prefix: &Path, suffix: &str) -> std::path::PathBuf {
    let mut s = prefix.as_os_str().to_owned();
    s.push(suffix);
    s.into()
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// `model_info.params.loss_function.type` (params may be an object or a JSON string).
/// `Ok(None)` when the model does not record it (hand-written or `sum_models` output).
fn loss_function(model_info: Option<&Value>) -> Result<Option<String>, String> {
    let params = match model_info.and_then(|m| m.get("params")) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).map_err(|e| format!("model_info.params: {e}"))?,
        Some(v) => v.clone(),
    };
    match params.get("loss_function") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.split(':').next().unwrap_or("").to_string())),
        Some(lf) => match lf.get("type") {
            Some(Value::String(s)) => Ok(Some(s.clone())),
            _ => Err(format!("model_info.params.loss_function: unexpected {lf}")),
        },
    }
}

fn nonempty(v: &Option<Vec<Value>>) -> bool {
    v.as_ref().is_some_and(|v| !v.is_empty())
}

impl Model {
    /// Load `P.cbm.json`, `P.meta.json` and (when meta says calibrated) `P.iso.json`.
    pub fn load(prefix: &Path) -> Result<Model, String> {
        let cbm = read(&with_suffix(prefix, ".cbm.json"))?;
        let meta = read(&with_suffix(prefix, ".meta.json"))?;
        let iso_path = with_suffix(prefix, ".iso.json");
        let iso = if iso_path.exists() { Some(read(&iso_path)?) } else { None };
        Model::from_json(&cbm, &meta, iso.as_deref()).map_err(|e| format!("{}: {e}", prefix.display()))
    }

    /// Build from in-memory JSON documents (`iso` used only when meta.calibrated is true).
    pub fn from_json(cbm: &str, meta: &str, iso: Option<&str>) -> Result<Model, String> {
        let meta: Meta = serde_json::from_str(meta).map_err(|e| format!("meta.json: {e}"))?;
        let cb: CbModel = serde_json::from_str(cbm).map_err(|e| format!("cbm.json: {e}"))?;
        let loss = loss_function(cb.model_info.as_ref())?;

        let kind = match meta.kind.as_str() {
            "binary" => Kind::Binary,
            "multiclass" => Kind::Multiclass,
            k => return Err(format!("meta.kind {k:?} not supported")),
        };
        let dim = match kind {
            Kind::Binary => {
                if meta.n_classes != 2 {
                    return Err(format!("binary model with n_classes={}", meta.n_classes));
                }
                1
            }
            Kind::Multiclass => {
                if meta.n_classes < 2 {
                    return Err(format!("multiclass model with n_classes={}", meta.n_classes));
                }
                meta.n_classes
            }
        };

        // Probabilities are sigmoid (binary) / softmax (multiclass); other losses map raw values
        // differently (e.g. MultiClassOneVsAll = per-class sigmoid), so refuse them.
        if let Some(l) = loss.as_deref() {
            let ok = match kind {
                Kind::Binary => matches!(l, "Logloss" | "CrossEntropy"),
                Kind::Multiclass => l == "MultiClass",
            };
            if !ok {
                return Err(format!("loss_function {l:?} not supported for meta.kind {:?}", meta.kind));
            }
        }

        // -------- features
        let fi = &cb.features_info;
        if nonempty(&fi.categorical_features) || nonempty(&fi.text_features) || nonempty(&fi.embedding_features) {
            return Err("categorical/text/embedding features are not supported".into());
        }
        if cb.ctr_data.as_ref().is_some_and(|v| !v.is_null() && v.as_object().is_none_or(|o| !o.is_empty())) {
            return Err("ctr_data present (categorical features) not supported".into());
        }
        if cb.trees.is_some() {
            return Err("non-oblivious trees are not supported".into());
        }
        let n_feat = meta.features.len();
        {
            let mut seen = std::collections::HashSet::with_capacity(n_feat);
            if let Some(d) = meta.features.iter().find(|f| !seen.insert(f.as_str())) {
                return Err(format!("meta.features lists {d:?} more than once"));
            }
        }
        // CatBoost lists every input column (used, unused or ignored) in float_features, so the
        // meta must name exactly that many columns and the flat indices must be a permutation.
        let n_ff = fi.float_features.len();
        if n_ff != n_feat {
            return Err(format!("model has {n_ff} float features but meta lists {n_feat} features"));
        }
        // float_feature_index (as used by splits) -> (flat index, nan goes right)
        let mut ff: Vec<Option<(u32, bool)>> = vec![None; n_ff];
        let mut flat_seen = vec![false; n_feat];
        for f in &fi.float_features {
            if f.feature_index >= n_ff {
                return Err(format!("float feature_index {} out of range (model has {n_ff} float features)", f.feature_index));
            }
            if f.flat_feature_index >= n_feat {
                return Err(format!(
                    "model uses flat feature {} but meta lists {} features",
                    f.flat_feature_index, n_feat
                ));
            }
            if std::mem::replace(&mut flat_seen[f.flat_feature_index], true) {
                return Err(format!("duplicate flat_feature_index {}", f.flat_feature_index));
            }
            if !f.feature_id.is_empty() && f.feature_id != meta.features[f.flat_feature_index] {
                return Err(format!(
                    "feature name mismatch at column {}: model {:?} vs meta {:?}",
                    f.flat_feature_index, f.feature_id, meta.features[f.flat_feature_index]
                ));
            }
            let nan_true = match f.nan_value_treatment.as_deref() {
                None | Some("AsIs") | Some("AsFalse") => false,
                Some("AsTrue") => true,
                Some(t) => return Err(format!("unknown nan_value_treatment {t:?}")),
            };
            if ff[f.feature_index].is_some() {
                return Err(format!("duplicate float feature_index {}", f.feature_index));
            }
            ff[f.feature_index] = Some((f.flat_feature_index as u32, nan_true));
        }
        let names_verified = n_feat > 0 && fi.float_features.iter().all(|f| !f.feature_id.is_empty());

        // -------- trees
        let cb_trees = cb.oblivious_trees.ok_or("no oblivious_trees in model")?;
        let mut splits = Vec::new();
        let mut nan_true = Vec::new();
        let mut trees = Vec::with_capacity(cb_trees.len());
        let mut leaves = Vec::new();
        for (ti, t) in cb_trees.iter().enumerate() {
            let depth = t.splits.len();
            if depth > 24 {
                return Err(format!("tree {ti}: depth {depth} too large"));
            }
            if t.leaf_values.len() != dim << depth {
                return Err(format!(
                    "tree {ti}: {} leaf values, expected {} (depth {depth}, dim {dim})",
                    t.leaf_values.len(),
                    dim << depth
                ));
            }
            trees.push(Tree { split_off: splits.len() as u32, depth: depth as u32, leaf_off: leaves.len() as u32 });
            for s in &t.splits {
                match s.split_type.as_deref() {
                    None | Some("FloatFeature") => {}
                    Some(other) => return Err(format!("tree {ti}: split type {other:?} not supported")),
                }
                let fidx = s.float_feature_index.ok_or_else(|| format!("tree {ti}: split without float_feature_index"))?;
                let border = s.border.ok_or_else(|| format!("tree {ti}: split without border"))?;
                let (flat, nt) = ff
                    .get(fidx)
                    .copied()
                    .flatten()
                    .ok_or_else(|| format!("tree {ti}: unknown float_feature_index {fidx}"))?;
                splits.push(Split { feat: flat, border: border as f32 });
                nan_true.push(nt);
            }
            leaves.extend_from_slice(&t.leaf_values);
        }
        if leaves.len() > u32::MAX as usize || splits.len() > u32::MAX as usize {
            return Err("model too large".into());
        }
        if !nan_true.iter().any(|&b| b) {
            nan_true.clear();
        }
        let uniform_depth = match trees.first() {
            Some(t0) if trees.iter().all(|t| t.depth == t0.depth) => t0.depth as usize,
            _ => 0,
        };

        // -------- scale and bias: [scale, bias] or [scale, [bias..]]
        let (scale, bias) = match &cb.scale_and_bias {
            None | Some(Value::Null) => (1.0, vec![0.0; dim]),
            Some(Value::Array(a)) if a.len() == 2 => {
                let scale = a[0].as_f64().ok_or("scale_and_bias: bad scale")?;
                let bias: Vec<f64> = match &a[1] {
                    Value::Array(b) => b.iter().map(|v| v.as_f64().ok_or("scale_and_bias: bad bias")).collect::<Result<_, _>>()?,
                    v => vec![v.as_f64().ok_or("scale_and_bias: bad bias")?],
                };
                let bias = if bias.len() == dim {
                    bias
                } else if bias.len() == 1 {
                    vec![bias[0]; dim]
                } else {
                    return Err(format!("scale_and_bias: bias len {} vs dim {dim}", bias.len()));
                };
                (scale, bias)
            }
            Some(v) => return Err(format!("scale_and_bias: unexpected {v}")),
        };

        // -------- calibration
        let iso = if meta.calibrated {
            let iso = iso.ok_or("meta.calibrated is true but iso.json is missing")?;
            let j: IsoJson = serde_json::from_str(iso).map_err(|e| format!("iso.json: {e}"))?;
            match kind {
                Kind::Binary => {
                    let (x, y) = j.x.zip(j.y).ok_or("iso.json: binary model needs x and y")?;
                    vec![IsoTable::new(x, y)?]
                }
                Kind::Multiclass => {
                    let c = j.classes.ok_or("iso.json: multiclass model needs classes")?;
                    if c.len() != meta.n_classes {
                        return Err(format!("iso.json: {} class tables vs n_classes {}", c.len(), meta.n_classes));
                    }
                    c.into_iter().map(|t| IsoTable::new(t.x, t.y)).collect::<Result<_, _>>()?
                }
            }
        } else {
            Vec::new()
        };

        Ok(Model {
            name: meta.name,
            target: meta.target,
            kind,
            n_classes: meta.n_classes,
            dim,
            features: meta.features,
            names_verified,
            loss,
            train_windows: meta.train_windows,
            splits,
            nan_true,
            trees,
            uniform_depth,
            leaves,
            scale,
            bias,
            iso,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn target(&self) -> &str {
        &self.target
    }
    pub fn kind(&self) -> Kind {
        self.kind
    }
    pub fn n_classes(&self) -> usize {
        self.n_classes
    }
    pub fn train_windows(&self) -> &[Value] {
        &self.train_windows
    }
    pub fn is_calibrated(&self) -> bool {
        !self.iso.is_empty()
    }
    /// True when the cbm stored feature names and all of them matched meta.features. False for
    /// models fitted on unnamed numpy arrays: then the column order in meta cannot be checked.
    pub fn feature_names_verified(&self) -> bool {
        self.names_verified
    }
    /// CatBoost loss function recorded in the cbm (None if absent).
    pub fn loss_function(&self) -> Option<&str> {
        self.loss.as_deref()
    }
    pub fn n_trees(&self) -> usize {
        self.trees.len()
    }
    /// Feature names in model column order (`x` passed to predict must follow this order).
    pub fn features(&self) -> &[String] {
        &self.features
    }
    /// Output length: 1 for binary (P(class 1) / raw logit), n_classes for multiclass.
    pub fn n_out(&self) -> usize {
        self.dim
    }

    /// CatBoost RawFormulaVal. Binary: `[logit]`, multiclass: `[raw_0..raw_{k-1}]`.
    pub fn predict_raw(&self, x: &[f32]) -> Vec<f64> {
        let mut out = vec![0.0; self.dim];
        self.predict_raw_into(x, &mut out);
        out
    }

    /// Allocation-free raw prediction; `out.len()` must equal `n_out()`.
    pub fn predict_raw_into(&self, x: &[f32], out: &mut [f64]) {
        assert_eq!(x.len(), self.features.len(), "model {}: feature vector length", self.name);
        assert_eq!(out.len(), self.dim, "model {}: output length", self.name);
        if self.dim == 1 {
            let s = if self.nan_true.is_empty() { self.sum1::<false>(x) } else { self.sum1::<true>(x) };
            out[0] = self.scale * s + self.bias[0];
        } else {
            out.fill(0.0);
            if self.nan_true.is_empty() {
                self.sum_k::<false>(x, out)
            } else {
                self.sum_k::<true>(x, out)
            }
            for (o, b) in out.iter_mut().zip(&self.bias) {
                *o = self.scale * *o + b;
            }
        }
    }

    #[inline(always)]
    fn leaf_index<const NAN_TRUE: bool>(&self, t: &Tree, x: &[f32]) -> usize {
        let off = t.split_off as usize;
        let sp = &self.splits[off..off + t.depth as usize];
        let mut idx = 0usize;
        for (j, s) in sp.iter().enumerate() {
            // Safe by construction: every split.feat < features.len() == x.len() (checked at load / call).
            let v = unsafe { *x.get_unchecked(s.feat as usize) };
            let mut bit = v > s.border;
            if NAN_TRUE {
                bit |= v.is_nan() & unsafe { *self.nan_true.get_unchecked(off + j) };
            }
            idx |= (bit as usize) << j;
        }
        idx
    }

    /// Leaf index for a tree of compile-time depth D (unrolled; used when all trees share one depth).
    #[inline(always)]
    fn leaf_index_d<const NAN_TRUE: bool, const D: usize>(&self, off: usize, x: &[f32]) -> usize {
        let sp: &[Split; D] = self.splits[off..off + D].try_into().unwrap();
        let mut idx = 0usize;
        for j in 0..D {
            let s = sp[j];
            let v = unsafe { *x.get_unchecked(s.feat as usize) };
            let mut bit = v > s.border;
            if NAN_TRUE {
                bit |= v.is_nan() & unsafe { *self.nan_true.get_unchecked(off + j) };
            }
            idx |= (bit as usize) << j;
        }
        idx
    }

    fn sum1_d<const NAN_TRUE: bool, const D: usize>(&self, x: &[f32]) -> f64 {
        // trees are contiguous: tree i has splits [i*D, i*D+D) and leaves [i<<D, (i+1)<<D)
        let n = self.trees.len();
        let (mut s0, mut s1) = (0.0f64, 0.0f64);
        let mut i = 0;
        while i + 1 < n {
            let a = self.leaf_index_d::<NAN_TRUE, D>(i * D, x);
            let b = self.leaf_index_d::<NAN_TRUE, D>((i + 1) * D, x);
            s0 += unsafe { *self.leaves.get_unchecked((i << D) + a) };
            s1 += unsafe { *self.leaves.get_unchecked(((i + 1) << D) + b) };
            i += 2;
        }
        if i < n {
            s0 += unsafe { *self.leaves.get_unchecked((i << D) + self.leaf_index_d::<NAN_TRUE, D>(i * D, x)) };
        }
        s0 + s1
    }

    fn sum1<const NAN_TRUE: bool>(&self, x: &[f32]) -> f64 {
        match self.uniform_depth {
            6 => return self.sum1_d::<NAN_TRUE, 6>(x),
            7 => return self.sum1_d::<NAN_TRUE, 7>(x),
            8 => return self.sum1_d::<NAN_TRUE, 8>(x),
            _ => {}
        }
        let mut s = 0.0f64;
        for t in &self.trees {
            let idx = self.leaf_index::<NAN_TRUE>(t, x);
            // leaf_off + idx < leaves.len(): idx < 2^depth and the tree owns 2^depth leaves.
            s += unsafe { *self.leaves.get_unchecked(t.leaf_off as usize + idx) };
        }
        s
    }

    fn sum_k<const NAN_TRUE: bool>(&self, x: &[f32], out: &mut [f64]) {
        let d = self.dim;
        for t in &self.trees {
            let idx = self.leaf_index::<NAN_TRUE>(t, x);
            let base = t.leaf_off as usize + idx * d;
            for (o, l) in out.iter_mut().zip(&self.leaves[base..base + d]) {
                *o += l;
            }
        }
    }

    /// Uncalibrated probabilities from raw values (sigmoid / softmax), in place.
    pub fn raw_to_proba(&self, out: &mut [f64]) {
        if self.dim == 1 {
            out[0] = sigmoid(out[0]);
        } else {
            softmax(out);
        }
    }

    /// Apply the isotonic tables in place (no-op if the model is not calibrated).
    pub fn calibrate(&self, p: &mut [f64]) {
        if self.iso.is_empty() {
            return;
        }
        if self.dim == 1 {
            p[0] = self.iso[0].apply(p[0]);
            return;
        }
        let mut sum = 0.0;
        for (c, t) in self.iso.iter().enumerate() {
            sum += t.apply(p[c]);
        }
        if sum > 0.0 {
            for (c, t) in self.iso.iter().enumerate() {
                p[c] = t.apply(p[c]) / sum;
            }
        } // sum == 0: keep the uncalibrated softmax
    }

    /// Calibrated probabilities (uncalibrated if the model has no iso tables).
    /// Binary: `[P(class 1)]`, multiclass: `[P_0..P_{k-1}]`.
    pub fn predict_proba(&self, x: &[f32]) -> Vec<f64> {
        let mut out = vec![0.0; self.dim];
        self.predict_proba_into(x, &mut out);
        out
    }

    /// Allocation-free calibrated probabilities; `out.len()` must equal `n_out()`.
    pub fn predict_proba_into(&self, x: &[f32], out: &mut [f64]) {
        self.predict_raw_into(x, out);
        self.raw_to_proba(out);
        self.calibrate(out);
    }

    /// Allocation-free uncalibrated probabilities.
    pub fn predict_proba_uncalibrated_into(&self, x: &[f32], out: &mut [f64]) {
        self.predict_raw_into(x, out);
        self.raw_to_proba(out);
    }
}

#[inline]
pub fn sigmoid(z: f64) -> f64 {
    1.0 / (1.0 + (-z).exp())
}

/// Numerically stable softmax in place.
pub fn softmax(v: &mut [f64]) {
    let m = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mut s = 0.0;
    for e in v.iter_mut() {
        *e = (*e - m).exp();
        s += *e;
    }
    for e in v.iter_mut() {
        *e /= s;
    }
}

/// Reorders rows from an "available" column order into a model's feature order.
#[derive(Debug, Clone)]
pub struct FeatureMap {
    idx: Vec<usize>,
    n_available: usize,
}

impl FeatureMap {
    /// Errors if any model feature is not among the available names, if a feature the model needs
    /// appears more than once in `available` (ambiguous column), or if `model_features` repeats a name.
    /// Repeated available names the model does not use are allowed.
    pub fn new<S: AsRef<str>>(model_features: &[String], available: &[S]) -> Result<FeatureMap, String> {
        let mut pos: std::collections::HashMap<&str, usize> = std::collections::HashMap::with_capacity(available.len());
        let mut dup: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (i, s) in available.iter().enumerate() {
            if pos.insert(s.as_ref(), i).is_some() {
                dup.insert(s.as_ref());
            }
        }
        let mut seen_model = std::collections::HashSet::with_capacity(model_features.len());
        for f in model_features {
            if !seen_model.insert(f.as_str()) {
                return Err(format!("model feature {f:?} listed more than once"));
            }
        }
        let mut ambiguous: Vec<&str> =
            model_features.iter().map(|f| f.as_str()).filter(|f| dup.contains(f)).collect();
        if !ambiguous.is_empty() {
            ambiguous.sort_unstable();
            return Err(format!("ambiguous features (column name repeated in input): {}", ambiguous.join(", ")));
        }
        let mut missing = Vec::new();
        let idx: Vec<usize> = model_features
            .iter()
            .map(|f| match pos.get(f.as_str()) {
                Some(&i) => i,
                None => {
                    missing.push(f.as_str());
                    0
                }
            })
            .collect();
        if !missing.is_empty() {
            return Err(format!("missing features: {}", missing.join(", ")));
        }
        Ok(FeatureMap { idx, n_available: available.len() })
    }

    pub fn for_model<S: AsRef<str>>(model: &Model, available: &[S]) -> Result<FeatureMap, String> {
        FeatureMap::new(model.features(), available)
    }

    /// Model-order length.
    pub fn len(&self) -> usize {
        self.idx.len()
    }
    pub fn is_empty(&self) -> bool {
        self.idx.is_empty()
    }

    /// `row` in available order -> `out` in model order (reuses `out`'s allocation).
    pub fn gather(&self, row: &[f32], out: &mut Vec<f32>) {
        out.clear();
        out.resize(self.idx.len(), 0.0);
        self.gather_into(row, out);
    }

    /// Allocation-free; `out.len()` must equal `len()`.
    pub fn gather_into(&self, row: &[f32], out: &mut [f32]) {
        assert_eq!(row.len(), self.n_available, "FeatureMap: row length");
        assert_eq!(out.len(), self.idx.len(), "FeatureMap: out length");
        for (o, &i) in out.iter_mut().zip(&self.idx) {
            *o = row[i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn model_is_send_sync() {
        assert_send_sync::<Model>();
        assert_send_sync::<FeatureMap>();
    }

    #[test]
    fn iso_interp_matches_numpy() {
        let t = IsoTable::new(vec![0.1, 0.2, 0.2, 0.5], vec![0.0, 0.1, 0.3, 0.9]).unwrap();
        assert_eq!(t.apply(0.0), 0.0);
        assert_eq!(t.apply(0.1), 0.0);
        assert!((t.apply(0.15) - 0.05).abs() < 1e-15);
        assert!((t.apply(0.35) - 0.6).abs() < 1e-15);
        assert_eq!(t.apply(0.7), 0.9);
        assert!(IsoTable::new(vec![0.2, 0.1], vec![0.0, 1.0]).is_err());
    }

    #[test]
    fn feature_map() {
        let mf: Vec<String> = vec!["c".into(), "a".into()];
        let fm = FeatureMap::new(&mf, &["a", "b", "c"]).unwrap();
        let mut out = Vec::new();
        fm.gather(&[1.0, 2.0, 3.0], &mut out);
        assert_eq!(out, vec![3.0, 1.0]);
        let e = FeatureMap::new(&mf, &["a", "b"]).unwrap_err();
        assert!(e.contains('c'), "{e}");
    }
}
