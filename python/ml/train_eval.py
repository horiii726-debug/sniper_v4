"""EXPLORATORY CatBoost training (GPU) on causal snapshots; walk-forward protocol.

dev windows D1, D2: model A (train D1) scores D2, model B (train D2) scores D1  -> scores_cross.csv
final model (train D1+D2) scores each test window                             -> scores_final.csv
Test windows are never used for training or threshold selection.
"""
import json
import sys

import numpy as np
import pandas as pd
from catboost import CatBoostClassifier
from sklearn.metrics import average_precision_score, roc_auc_score

DEV = sys.argv[1].split(",")
TEST = sys.argv[2].split(",")
TARGETS = {"p_up2": "y_up2", "p_up5": "y_up5", "p_cont": "y_cont", "p_rug": "y_rug"}
META = {"coin", "order", "time", "y_up2", "y_up5", "y_up10", "y_cont", "y_rug", "y_fwd_max"}


def load(d):
    return pd.read_csv(f"{d}/snapshots.csv")


def fit(df, y):
    m = CatBoostClassifier(iterations=800, depth=6, learning_rate=0.05, l2_leaf_reg=3, loss_function="Logloss",
                           task_type="GPU", devices="0", verbose=0, random_seed=7)
    m.fit(df[FEATS].values, df[y].values)
    return m


def score(models, df):
    out = pd.DataFrame({"coin": df["coin"], "order": df["order"]})
    for p, m in models.items():
        out[p] = m.predict_proba(df[FEATS].values)[:, 1]
    out["p_up5"] = np.minimum(out["p_up5"], out["p_up2"])  # monotonicity P(>=5x) <= P(>=2x)
    return out[["coin", "order", "p_up2", "p_up5", "p_cont", "p_rug"]]


def metrics(name, df, sc):
    res = {}
    for p, y in TARGETS.items():
        yt = df[y].values
        if yt.min() == yt.max():
            continue
        res[p] = {"auc": round(roc_auc_score(yt, sc[p]), 4), "pr_auc": round(average_precision_score(yt, sc[p]), 4),
                  "base_rate": round(float(yt.mean()), 4)}
    # top-k lift for entry: share of >=2x / >=5x / >=10x among the top 1% scores
    k = max(1, len(df) // 100)
    top = df.iloc[np.argsort(-sc["p_up2"].values)[:k]]
    res["top1pct_by_p_up2"] = {"n": k, "up2": round(top.y_up2.mean(), 3), "up5": round(top.y_up5.mean(), 3),
                               "up10": round(top.y_up10.mean(), 4)}
    print(name, json.dumps(res))
    return res


data = {d: load(d) for d in DEV + TEST}
FEATS = [c for c in data[DEV[0]].columns if c not in META]
report = {"features": FEATS, "oos": {}}

# Cross models on the two dev windows.
assert len(DEV) == 2, "expects exactly two dev windows"
for tr, te in [(DEV[0], DEV[1]), (DEV[1], DEV[0])]:
    models = {p: fit(data[tr], y) for p, y in TARGETS.items()}
    sc = score(models, data[te])
    sc.to_csv(f"{te}/scores_cross.csv", index=False)
    report["oos"][f"train {tr} -> {te}"] = metrics(f"cross {tr}->{te}", data[te], sc)

# Final model on all dev data, scored on the untouched test windows.
full = pd.concat([data[d] for d in DEV], ignore_index=True)
models = {p: fit(full, y) for p, y in TARGETS.items()}
for te in TEST:
    sc = score(models, data[te])
    sc.to_csv(f"{te}/scores_final.csv", index=False)
    report["oos"][f"final -> {te}"] = metrics(f"final->{te}", data[te], sc)

imp = models["p_up2"].get_feature_importance()
report["importance_p_up2_top15"] = sorted(zip(FEATS, map(float, imp)), key=lambda x: -x[1])[:15]
imp = models["p_cont"].get_feature_importance()
report["importance_p_cont_top10"] = sorted(zip(FEATS, map(float, imp)), key=lambda x: -x[1])[:10]
json.dump(report, open("reports/ml_exploratory_models.json", "w"), indent=1)
print("importance p_up2:", [(f, round(v, 1)) for f, v in report["importance_p_up2_top15"]])
print("importance p_cont:", [(f, round(v, 1)) for f, v in report["importance_p_cont_top10"]])
