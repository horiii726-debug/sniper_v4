"""Model-family comparison, one family at a time, on the same features / labels / walk-forward folds (EXPLORATORY).

usage: python3 python/ml/model_families.py DIR_A DIR_B [--horizon 600] [--out reports/v6/MODEL_FAMILIES.json]

Data: <dir>/snapshots_short_<h>.csv.zst from `sniper snapshots-short` (same Rust Engine features as V5 snapshots,
labels over (t, t+h] only where the data covers t+h: no censoring). Folds: train on one window, test on the other
(both directions). Families: logistic regression (linear), PCA + logistic regression (linear algebra), random forest,
CatBoost (GPU when available). Rows are weighted 1 / rows-per-coin. Every rate carries its positive count; fewer than
30 positives = direction only. Nothing here touches holdout windows.
"""
import argparse
import json
import os
import sys
import time

import numpy as np
import pandas as pd
from sklearn.decomposition import PCA
from sklearn.ensemble import RandomForestClassifier
from sklearn.impute import SimpleImputer
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import average_precision_score, brier_score_loss, roc_auc_score
from sklearn.pipeline import make_pipeline
from sklearn.preprocessing import StandardScaler

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from v5_common import rust_str_array  # noqa: E402

TARGETS = ["y_s_2x", "y_s_5x", "y_s_dump"]
FEATURES = rust_str_array("FEATURES")


def load(d, h):
    df = pd.read_csv(os.path.join(d, f"snapshots_short_{h}.csv.zst"), compression="zstd")
    X = df[FEATURES].to_numpy(np.float64)
    X[~np.isfinite(X)] = np.nan
    w = 1.0 / df.groupby("coin")["coin"].transform("size").to_numpy(np.float64)
    return df, X.astype(np.float32), w


def families(seed=0):
    def logreg():
        return make_pipeline(SimpleImputer(strategy="median", add_indicator=True, keep_empty_features=True), StandardScaler(),
                             LogisticRegression(C=0.5, max_iter=3000))

    def pca_logreg():
        return make_pipeline(SimpleImputer(strategy="median", keep_empty_features=True), StandardScaler(), PCA(n_components=20, random_state=seed),
                             LogisticRegression(C=0.5, max_iter=3000))

    def rf():
        return make_pipeline(SimpleImputer(strategy="median", keep_empty_features=True),
                             RandomForestClassifier(n_estimators=400, min_samples_leaf=20, max_features="sqrt", n_jobs=-1, random_state=seed))

    def catboost():
        from catboost import CatBoostClassifier
        try:
            import subprocess
            gpu = subprocess.run(["nvidia-smi", "-L"], capture_output=True).returncode == 0
        except OSError:
            gpu = False
        kw = dict(task_type="GPU", devices="0") if gpu else dict(thread_count=-1)
        return CatBoostClassifier(iterations=800, depth=6, learning_rate=0.05, loss_function="Logloss", verbose=False, random_seed=seed, **kw)

    return {"logistic_regression": logreg, "pca20_logistic": pca_logreg, "random_forest": rf, "catboost": catboost}


def metrics(y, p, logmax, w=None):
    n_pos = int(y.sum())
    out = {"rows": int(len(y)), "positives": n_pos, "base_rate": float(y.mean()) if len(y) else float("nan"),
           "direction_only": n_pos < 30}
    if n_pos == 0 or n_pos == len(y):
        return out
    out["auc"] = float(roc_auc_score(y, p))
    out["pr_auc"] = float(average_precision_score(y, p))
    out["brier"] = float(brier_score_loss(y, np.clip(p, 0, 1)))
    order = np.argsort(-p, kind="stable")
    for frac in (0.01, 0.05):
        k = max(1, int(round(frac * len(y))))
        top = order[:k]
        out[f"precision_top{int(frac * 100)}pct"] = float(y[top].mean())
        out[f"median_max_mult_top{int(frac * 100)}pct"] = float(np.exp(np.median(logmax[top])))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("a")
    ap.add_argument("b")
    ap.add_argument("--horizon", type=int, default=600)
    ap.add_argument("--out", default="reports/v6/MODEL_FAMILIES.json")
    args = ap.parse_args()
    data = {d: load(d, args.horizon) for d in (args.a, args.b)}
    res = {"label": "EXPLORATORY model-family comparison (short horizon, 2 windows, direction only)", "horizon_s": args.horizon,
           "features": len(FEATURES), "folds": []}
    for tr, te in ((args.a, args.b), (args.b, args.a)):
        dtr, Xtr, wtr = data[tr]
        dte, Xte, _ = data[te]
        dec = ((dte["is_signal"] == 1) & (dte["in_window"] == 1)).to_numpy()
        fold = {"train": tr, "test": te, "results": {}}
        for tgt in TARGETS:
            ytr, yte = dtr[tgt].to_numpy(int), dte[tgt].to_numpy(int)
            lm = dte["y_s_logmax"].to_numpy(np.float64)
            fold["results"][tgt] = {}
            for name, make in families().items():
                t0 = time.time()
                m = make()
                if name == "catboost":
                    m.fit(Xtr, ytr, sample_weight=wtr)
                else:
                    last = m.steps[-1][0]
                    m.fit(Xtr, ytr, **{f"{last}__sample_weight": wtr})
                p = m.predict_proba(Xte)[:, 1]
                fold["results"][tgt][name] = {"all_rows": metrics(yte, p, lm), "decision_rows": metrics(yte[dec], p[dec], lm[dec]),
                                              "fit_s": round(time.time() - t0, 1)}
                r = fold["results"][tgt][name]["all_rows"]
                print(f"{os.path.basename(tr)}->{os.path.basename(te)} {tgt:9} {name:20} auc {r.get('auc', float('nan')):.3f} "
                      f"pr {r.get('pr_auc', float('nan')):.3f} base {r['base_rate']:.3f} top1% {r.get('precision_top1pct', float('nan')):.3f} "
                      f"pos {r['positives']}", flush=True)
        res["folds"].append(fold)
    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with open(args.out, "w") as f:
        json.dump(res, f, indent=1)
    print("written", args.out)


if __name__ == "__main__":
    main()
