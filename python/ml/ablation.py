"""LEGACY V4 (reads V4 snapshots.csv; see legacy_v4_guard.py). OOS ablation of candidate feature groups (V4 §28: KEEP only with walk-forward OOS value).

For each config (base, base+F1, ..., base+all) a model is trained on the two dev windows and evaluated on
each untouched test window. KEEP rule (fixed before running): PR-AUC of p_up2 improves on BOTH test windows
and PR-AUC of p_rug does not drop by more than 0.005 on either.
"""
import os as _os, sys as _sys
_sys.path.insert(0, _os.path.dirname(_os.path.abspath(__file__)))
from legacy_v4_guard import require_v4_snapshots  # noqa: E402
import json, sys
import numpy as np, pandas as pd
from catboost import CatBoostClassifier
from sklearn.metrics import average_precision_score, roc_auc_score

DEV, TEST = sys.argv[1].split(","), sys.argv[2].split(",")
META = {"coin", "order", "time", "y_up2", "y_up5", "y_up10", "y_cont", "y_rug", "y_fwd_max", "y_peak2", "y_peak5", "y_peak10", "y_end30", "y_dump30s", "y_devsell10m"}
data = {d: pd.read_csv(require_v4_snapshots(d)) for d in DEV + TEST}
ALL = [c for c in data[DEV[0]].columns if c not in META]
BASE = [c for c in ALL if not c.startswith("f")]
GROUPS = ["f1_", "f2_", "f4_", "f6_", "f7_", "f8_"]
TARGETS = {"p_up2": "y_up2", "p_rug": "y_rug", "p_cont": "y_cont", "p_up5": "y_up5"}
train = pd.concat([data[d] for d in DEV], ignore_index=True)

def evaluate(feats):
    out = {}
    for p, y in TARGETS.items():
        m = CatBoostClassifier(iterations=500, depth=6, learning_rate=0.06, task_type="GPU", devices="0", verbose=0, random_seed=7)
        m.fit(train[feats].values, train[y].values)
        for te in TEST:
            s = m.predict_proba(data[te][feats].values)[:, 1]
            yt = data[te][y].values
            out.setdefault(te, {})[p] = {"auc": round(roc_auc_score(yt, s), 4), "pr": round(average_precision_score(yt, s), 4)}
            if p == "p_up2":
                k = len(s) // 100
                top = data[te].iloc[np.argsort(-s)[:k]]
                out[te]["top1pct_up2"] = round(top.y_up2.mean(), 3)
    return out

res = {"base": evaluate(BASE)}
for g in GROUPS:
    res["+" + g] = evaluate(BASE + [c for c in ALL if c.startswith(g)])
res["+all"] = evaluate(ALL)
decisions = {}
for g in GROUPS:
    r, b = res["+" + g], res["base"]
    up = all(r[te]["p_up2"]["pr"] > b[te]["p_up2"]["pr"] for te in TEST)
    rug_ok = all(r[te]["p_rug"]["pr"] >= b[te]["p_rug"]["pr"] - 0.005 for te in TEST)
    decisions[g] = "KEEP" if up and rug_ok else "REJECT"
json.dump({"results": res, "decisions": decisions}, open("reports/ablation_literature_features.json", "w"), indent=1)
for k, v in res.items():
    print(f"{k:7s} " + " | ".join(f"{te.split('/')[-1]}: up2 AUC {v[te]['p_up2']['auc']:.3f} PR {v[te]['p_up2']['pr']:.3f} top1% {v[te]['top1pct_up2']:.3f}; rug PR {v[te]['p_rug']['pr']:.3f}; cont PR {v[te]['p_cont']['pr']:.3f}" for te in TEST))
print("DECISIONS:", decisions)
