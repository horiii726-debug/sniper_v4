"""LightGBM hyperparameter selection without touching the test window (EXPLORATORY).

usage: python3 python/ml/v7_tune.py --train A [B ...] --val V --test T --target y_10x [--out reports/v7/TUNE_<target>.json]

Grid is fitted on --train, scored on --val (PR-AUC of decision rows); the best setting is frozen, refitted on
train + val and scored ONCE on --test. Every grid point is reported (number of trials matters for overfitting).
Windows must be chronological: train < val < test.
"""
import argparse
import itertools
import json
import os
import sys

import numpy as np
import pandas as pd
from sklearn.metrics import average_precision_score, roc_auc_score

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from model_families import FEATURES  # noqa: E402
from v7_eval import TARGETS, load  # noqa: E402


def rows(dirs, decision):
    out = []
    for d in dirs:
        df, _ = load(d)
        df = df[df["labelled"]]
        if decision:
            df = df[(df["is_signal"] == 1) & (df["in_window"] == 1)]
        out.append(df.assign(win=os.path.basename(d)))
    df = pd.concat(out, ignore_index=True)
    df["w"] = 1.0 / df.groupby(["win", "coin"])["coin"].transform("size")
    return df


def xmat(df):
    m = np.array(df[FEATURES].to_numpy(np.float64), copy=True)
    m[~np.isfinite(m)] = np.nan
    return m


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--train", nargs="+", required=True)
    ap.add_argument("--val", required=True)
    ap.add_argument("--test", required=True)
    ap.add_argument("--target", default="y_10x")
    ap.add_argument("--out")
    a = ap.parse_args()
    import lightgbm as lgb
    f = TARGETS[a.target]
    tr, va, te = rows(a.train, False), rows([a.val], True), rows([a.test], True)
    tr_full = rows(a.train, False)
    va_full = rows([a.val], False)
    grid = {"num_leaves": [15, 31, 63], "min_child_samples": [50, 200], "learning_rate": [0.03], "n_estimators": [300, 800],
            "colsample_bytree": [0.5, 0.8], "scale_pos": [1.0, 5.0]}
    keys = list(grid)
    trials = []
    ytr, yva = f(tr).to_numpy(int), f(va).to_numpy(int)
    for vals in itertools.product(*grid.values()):
        p = dict(zip(keys, vals))
        sw = tr["w"].to_numpy() * np.where(ytr == 1, p["scale_pos"], 1.0)
        m = lgb.LGBMClassifier(num_leaves=p["num_leaves"], min_child_samples=p["min_child_samples"], learning_rate=p["learning_rate"],
                               n_estimators=p["n_estimators"], colsample_bytree=p["colsample_bytree"], subsample=0.8, subsample_freq=1,
                               verbose=-1, random_state=0, n_jobs=3)
        m.fit(xmat(tr), ytr, sample_weight=sw)
        pv = m.predict_proba(xmat(va))[:, 1]
        trials.append({**p, "val_pr_auc": float(average_precision_score(yva, pv)), "val_auc": float(roc_auc_score(yva, pv))})
        print(trials[-1], flush=True)
    best = max(trials, key=lambda t: t["val_pr_auc"])
    both = pd.concat([tr_full, va_full], ignore_index=True)
    yb = f(both).to_numpy(int)
    m = lgb.LGBMClassifier(num_leaves=best["num_leaves"], min_child_samples=best["min_child_samples"], learning_rate=best["learning_rate"],
                           n_estimators=best["n_estimators"], colsample_bytree=best["colsample_bytree"], subsample=0.8, subsample_freq=1,
                           verbose=-1, random_state=0, n_jobs=3)
    m.fit(xmat(both), yb, sample_weight=both["w"].to_numpy() * np.where(yb == 1, best["scale_pos"], 1.0))
    yt = f(te).to_numpy(int)
    pt = m.predict_proba(xmat(te))[:, 1]
    k = max(1, int(round(0.01 * len(pt))))
    res = {"target": a.target, "train": a.train, "val": a.val, "test": a.test, "n_trials": len(trials), "best": best,
           "test": {"rows": int(len(yt)), "positives": int(yt.sum()), "base_rate": float(yt.mean()),
                    "auc": float(roc_auc_score(yt, pt)), "pr_auc": float(average_precision_score(yt, pt)),
                    "precision_top1pct": float(yt[np.argsort(-pt)[:k]].mean())},
           "trials": trials}
    print("BEST", best, "TEST", res["test"], flush=True)
    out = a.out or f"reports/v7/TUNE_{a.target}.json"
    os.makedirs(os.path.dirname(out), exist_ok=True)
    json.dump(res, open(out, "w"), indent=1)


if __name__ == "__main__":
    main()
