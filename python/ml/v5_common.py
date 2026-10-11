"""Shared helpers for the V5 model pipeline (offline only).

Model file interface (consumed by crates/model in Rust):
  P.cbm.json  CatBoost model, save_model(format="json")
  P.meta.json {"name", "kind": "binary"|"multiclass", "n_classes", "features", "target", "train_windows", "calibrated"}
  P.iso.json  binary: {"x": [...], "y": [...]}; multiclass: {"classes": [{"x", "y"}, ...]} (piecewise linear, clamped)
"""
import json
import os
import re

import numpy as np

# label horizon / purge gap: must equal the --label-horizon-s used for the snapshots (ml_sim.rs::label_h)
H_S = int(os.environ.get("LABEL_H", "36000"))
ID_COLS = ["coin", "order", "time", "is_signal", "in_window"]
REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
ML_SIM = os.path.join(REPO, "crates", "app", "src", "ml_sim.rs")
TEN_X = float(np.log(10.0))


def rust_str_array(name, path=ML_SIM):
    """Parse `pub const NAME: [&str; N] = [...]` from ml_sim.rs (read only)."""
    src = open(path).read()
    m = re.search(r"pub const " + name + r": \[&str; (\d+)\] = \[(.*?)\];", src, re.S)
    if not m:
        raise ValueError(f"{name} not found in {path}")
    body = "\n".join(line.split("//", 1)[0] for line in m.group(2).splitlines())
    vals = re.findall(r'"([^"]+)"', body)
    assert len(vals) == int(m.group(1)), (name, len(vals), m.group(1))
    return vals


def window_label(path):
    """data/v5_s05d_11h -> s05d_11h (used in models/v5/fold_<W>)."""
    b = os.path.basename(os.path.normpath(path))
    return b[3:] if b.startswith("v5_") else b


def feature_columns(header):
    """Exactly the columns between in_window and y_tp1."""
    i0, i1 = header.index("in_window"), header.index("y_tp1")
    feats = header[i0 + 1:i1]
    bad = [c for c in feats if c.startswith("y_") or c in ID_COLS]
    assert not bad, f"forbidden feature columns: {bad}"
    return feats


# ---------------------------------------------------------------- isotonic tables
def fit_iso_table(p, y):
    from sklearn.isotonic import IsotonicRegression
    ir = IsotonicRegression(y_min=0.0, y_max=1.0, increasing=True, out_of_bounds="clip")
    ir.fit(np.asarray(p, dtype=np.float64), np.asarray(y, dtype=np.float64))
    x, yy = np.asarray(ir.X_thresholds_, float), np.asarray(ir.y_thresholds_, float)
    if len(x) == 1:  # degenerate: constant
        x, yy = np.array([0.0, 1.0]), np.array([yy[0], yy[0]])
    return {"x": x.tolist(), "y": yy.tolist()}


IDENTITY = {"x": [0.0, 1.0], "y": [0.0, 1.0]}


def apply_iso_binary(p, tab):
    return np.interp(np.asarray(p, float), tab["x"], tab["y"])  # np.interp clamps at the ends


def apply_iso_multi(P, tabs):
    P = np.asarray(P, float)
    Q = np.column_stack([np.interp(P[:, k], t["x"], t["y"]) for k, t in enumerate(tabs["classes"])])
    s = Q.sum(axis=1, keepdims=True)
    zero = s[:, 0] <= 0
    Q = np.divide(Q, s, out=np.zeros_like(Q), where=s > 0)
    Q[zero] = P[zero]
    return Q


# ---------------------------------------------------------------- metrics
def reliability(p, y, bins=10):
    p, y = np.asarray(p, float), np.asarray(y, float)
    idx = np.clip((p * bins).astype(int), 0, bins - 1)
    out = []
    for b in range(bins):
        m = idx == b
        n = int(m.sum())
        out.append({"bin": f"{b / bins:.1f}-{(b + 1) / bins:.1f}", "mean_pred": float(p[m].mean()) if n else None,
                    "obs_rate": float(y[m].mean()) if n else None, "count": n})
    return out


def binary_metrics(p, y):
    from sklearn.metrics import average_precision_score, brier_score_loss, log_loss, roc_auc_score
    p, y = np.clip(np.asarray(p, float), 0, 1), np.asarray(y).astype(int)
    r = {"rows": int(len(y)), "positives": int(y.sum()), "base_rate": float(y.mean()) if len(y) else None}
    if len(y) == 0:
        return r
    two = 0 < y.sum() < len(y)
    r["pr_auc"] = float(average_precision_score(y, p)) if two else None
    r["roc_auc"] = float(roc_auc_score(y, p)) if two else None
    r["brier"] = float(brier_score_loss(y, p))
    r["log_loss"] = float(log_loss(y, np.clip(p, 1e-7, 1 - 1e-7), labels=[0, 1]))
    r["reliability"] = reliability(p, y)
    return r


def multiclass_logloss(P, y):
    P = np.clip(np.asarray(P, float), 1e-7, 1.0)
    P = P / P.sum(axis=1, keepdims=True)
    y = np.asarray(y).astype(int)
    return float(-np.mean(np.log(P[np.arange(len(y)), y]))) if len(y) else None


def top_precision(score, hit, frac=0.01):
    score, hit = np.asarray(score, float), np.asarray(hit).astype(int)
    if len(score) == 0:
        return {"n": 0}
    k = max(1, int(round(len(score) * frac)))
    top = np.argsort(-score, kind="stable")[:k]
    prec, base = float(hit[top].mean()), float(hit.mean())
    return {"n": k, "precision": prec, "base_rate": base, "lift": (prec / base) if base > 0 else None}


def write_json(path, obj):
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(obj, f, indent=1)
    os.replace(tmp, path)
