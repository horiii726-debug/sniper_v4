"""LEGACY V4 (reads V4 snapshots.csv; see legacy_v4_guard.py). Nested leave-one-window-out score files (CatBoost GPU).
For each window W: scores_outer.csv in W (model trained on all other windows);
for each other window V: scores_inner_excl_<W>.csv in V (model trained on windows other than W and V)."""
import os as _os, sys as _sys
_sys.path.insert(0, _os.path.dirname(_os.path.abspath(__file__)))
from legacy_v4_guard import require_v4_snapshots  # noqa: E402
import sys, time
import numpy as np, pandas as pd
from catboost import CatBoostClassifier

WINS = sys.argv[1].split(",")
META = {"coin", "order", "time", "y_up2", "y_up5", "y_up10", "y_cont", "y_rug", "y_fwd_max", "y_peak2", "y_peak5", "y_peak10",
        "y_end30", "y_dump30s", "y_devsell10m", "y_rugdead", "y_devdump"}
data = {}
for w in WINS:
    d = pd.read_csv(require_v4_snapshots(w))
    d["y_rugdead"] = ((d.y_end30 <= 0.2) & (d.y_peak2 == 0)).astype(int)
    d["y_devdump"] = ((d.y_devsell10m == 1) & (d.y_end30 <= 0.3) & (d.y_peak2 == 0)).astype(int)
    data[w] = d
FEATS = [c for c in data[WINS[0]].columns if c not in META]
TARGETS = {"p_up2": "y_peak2", "p_up5": "y_peak5", "p_cont": "y_cont", "p_rug": "y_rugdead", "p_dump": "y_dump30s", "p_devsell": "y_devdump"}

def scores(train_wins, score_win, path):
    tr = pd.concat([data[w] for w in train_wins], ignore_index=True)
    te = data[score_win]
    out = pd.DataFrame({"coin": te.coin, "order": te.order})
    for p, y in TARGETS.items():
        m = CatBoostClassifier(iterations=500, depth=6, learning_rate=0.08, l2_leaf_reg=3, task_type="GPU", devices="0", verbose=0, random_seed=7)
        m.fit(tr[FEATS].values, tr[y].values)
        out[p] = m.predict_proba(te[FEATS].values)[:, 1]
    out["p_up5"] = np.minimum(out.p_up5, out.p_up2)
    out.to_csv(path, index=False)

t0 = time.time()
for w in WINS:
    others = [x for x in WINS if x != w]
    scores(others, w, f"{w}/scores_outer.csv")
    for v in others:
        scores([x for x in others if x != v], v, f"{v}/scores_inner_excl_{w.split('/')[-1]}.csv")
    print(f"{w} done ({time.time() - t0:.0f}s)", flush=True)
print("ALL_DONE", flush=True)
