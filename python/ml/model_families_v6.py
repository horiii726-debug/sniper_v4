"""Model-family comparison on V6 windows with 1 h labels, one family at a time (EXPLORATORY).

usage: LABEL_H=3600 python3 python/ml/model_families_v6.py data/v5_A data/v5_B data/v5_C ... [--out reports/v6/MODEL_FAMILIES_V6.json]

Data: <dir>/snapshots_v5.csv.zst (`sniper --label-horizon-s 3600 snapshots`): Rust Engine features, labels over
(t, t + 1 h] only where the recording (window + extension backfill) covers t + 1 h.
Folds: leave one window out (windows are hours / days apart, so no overlap within H). Train rows: every snapshot row of
the other windows, weighted 1 / rows-per-coin, subsampled to --max-train rows. Test rows: the decision rows
(radar Promote / Watch / Decide inside the decision window) of the held-out window = the rows the strategy acts on.
Targets (peak before the -65 % barrier, y_bin): >= 2x, >= 5x, >= 10x, >= 50x; y_rugdead (ends <= 0.2x, never 2x).
Families: logistic regression, PCA(20) + logistic (linear algebra), random forest, CatBoost (GPU).
Every rate carries its positive count; < 30 positives = direction only. Holdout windows must not be passed here.
"""
import argparse
import json
import os
import sys
import time

import numpy as np
import pandas as pd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from model_families import FEATURES, families, metrics  # noqa: E402

TARGETS = {"y_2x": lambda d: d["y_bin"] >= 3, "y_5x": lambda d: d["y_bin"] >= 4, "y_10x": lambda d: d["y_bin"] >= 5,
           "y_50x": lambda d: d["y_bin"] >= 6, "y_rugdead": lambda d: d["y_rugdead"] == 1}


def load(d):
    df = pd.read_csv(os.path.join(d, "snapshots_v5.csv.zst"), compression="zstd")
    X = df[FEATURES].to_numpy(np.float64)
    X[~np.isfinite(X)] = np.nan
    w = 1.0 / df.groupby("coin")["coin"].transform("size").to_numpy(np.float64)
    ys = {k: f(df).to_numpy(int) for k, f in TARGETS.items()}
    dec = ((df["is_signal"] == 1) & (df["in_window"] == 1)).to_numpy()
    return {"X": X.astype(np.float32), "w": w, "y": ys, "dec": dec, "logmax": df["y_logmax"].to_numpy(np.float64),
            "coin": df["coin"].to_numpy(), "n": len(df)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--max-train", type=int, default=1_500_000)
    ap.add_argument("--families", default="logistic_regression,pca20_logistic,random_forest,catboost")
    ap.add_argument("--out", default="reports/v6/MODEL_FAMILIES_V6.json")
    a = ap.parse_args()
    assert os.environ.get("LABEL_H") == "3600", "set LABEL_H=3600 (1 h labels)"
    data = {d: load(d) for d in a.dirs}
    for d, v in data.items():
        print(f"{os.path.basename(d)} rows {v['n']} decision {v['dec'].sum()} " + " ".join(f"{k}={v['y'][k][v['dec']].sum()}" for k in TARGETS), flush=True)
    fams = {k: v for k, v in families().items() if k in a.families.split(",")}
    rng = np.random.default_rng(0)
    res = {"label": "EXPLORATORY model families, 1 h labels, leave-one-window-out", "windows": [os.path.basename(d) for d in a.dirs],
           "features": len(FEATURES), "folds": []}
    agg = {t: {f: [] for f in fams} for t in TARGETS}
    for te in a.dirs:
        tr = [d for d in a.dirs if d != te]
        X = np.concatenate([data[d]["X"] for d in tr])
        w = np.concatenate([data[d]["w"] for d in tr])
        ys = {t: np.concatenate([data[d]["y"][t] for d in tr]) for t in TARGETS}
        idx = np.arange(len(X))
        if len(idx) > a.max_train:
            idx = np.sort(rng.choice(idx, a.max_train, replace=False, p=w / w.sum()))
            wsub = np.ones(len(idx))
        else:
            wsub = w
        D = data[te]
        Xte, dec = D["X"][D["dec"]], D["dec"]
        fold = {"test": os.path.basename(te), "train": [os.path.basename(d) for d in tr], "train_rows": int(len(idx)), "results": {}}
        for t in TARGETS:
            ytr = ys[t][idx]
            yte = D["y"][t][dec]
            fold["results"][t] = {}
            if ytr.sum() < 5 or ytr.sum() == len(ytr):
                continue
            for name, make in fams.items():
                t0 = time.time()
                m = make()
                if name == "catboost":
                    m.fit(X[idx], ytr, sample_weight=wsub)
                else:
                    m.fit(X[idx], ytr, **{f"{m.steps[-1][0]}__sample_weight": wsub})
                p = m.predict_proba(Xte)[:, 1]
                r = metrics(yte, p, D["logmax"][dec])
                r["fit_s"] = round(time.time() - t0, 1)
                fold["results"][t][name] = r
                agg[t][name].append(r)
                print(f"test {os.path.basename(te)} {t:9} {name:20} auc {r.get('auc', float('nan')):.3f} pr {r.get('pr_auc', float('nan')):.3f} "
                      f"base {r['base_rate']:.4f} top1% {r.get('precision_top1pct', float('nan')):.3f} top5% {r.get('precision_top5pct', float('nan')):.3f} "
                      f"pos {r['positives']}", flush=True)
        res["folds"].append(fold)
    summ = {}
    for t in TARGETS:
        summ[t] = {}
        for f, rs in agg[t].items():
            rs = [r for r in rs if "auc" in r]
            if not rs:
                continue
            summ[t][f] = {k: float(np.median([r[k] for r in rs])) for k in ("auc", "pr_auc", "base_rate", "precision_top1pct", "precision_top5pct")}
            summ[t][f]["lift_top5pct"] = summ[t][f]["precision_top5pct"] / max(summ[t][f]["base_rate"], 1e-9)
            summ[t][f]["positives_total"] = int(sum(r["positives"] for r in rs))
            summ[t][f]["folds"] = len(rs)
            summ[t][f]["auc_min"] = float(min(r["auc"] for r in rs))
    res["median_over_folds"] = summ
    os.makedirs(os.path.dirname(a.out), exist_ok=True)
    json.dump(res, open(a.out, "w"), indent=1)
    print(json.dumps(summ, indent=1))


if __name__ == "__main__":
    main()
