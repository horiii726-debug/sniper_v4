"""Nested leave-one-window-out scores for ONE model family (run CatBoost on GPU and RandomForest on CPU in parallel).
usage: nested_lowo_model.py W1,W2,W3,W4 catboost|rf
writes <W>/scores_outer_<m>.csv and <V>/scores_inner_excl_<W>_<m>.csv"""
import sys, time
import numpy as np, pandas as pd

WINS, MODEL = sys.argv[1].split(","), sys.argv[2]
META = {"coin", "order", "time", "y_up2", "y_up5", "y_up10", "y_cont", "y_rug", "y_fwd_max", "y_peak2", "y_peak5", "y_peak10",
        "y_end30", "y_dump30s", "y_devsell10m", "y_rugdead", "y_devdump"}
data = {}
for w in WINS:
    d = pd.read_csv(f"{w}/snapshots.csv")
    d["y_rugdead"] = ((d.y_end30 <= 0.2) & (d.y_peak2 == 0)).astype(int)
    d["y_devdump"] = ((d.y_devsell10m == 1) & (d.y_end30 <= 0.3) & (d.y_peak2 == 0)).astype(int)
    data[w] = d
FEATS = [c for c in data[WINS[0]].columns if c not in META]
TARGETS = {"p_up2": "y_peak2", "p_up5": "y_peak5", "p_cont": "y_cont", "p_rug": "y_rugdead", "p_dump": "y_dump30s", "p_devsell": "y_devdump"}


def model():
    if MODEL == "catboost":
        from catboost import CatBoostClassifier
        return CatBoostClassifier(iterations=500, depth=6, learning_rate=0.08, l2_leaf_reg=3, task_type="GPU", devices="0", verbose=0, random_seed=7)
    from sklearn.ensemble import RandomForestClassifier
    return RandomForestClassifier(n_estimators=150, max_depth=18, min_samples_leaf=20, max_features="sqrt", max_samples=0.5,
                                  n_jobs=14, random_state=7)


def scores(train_wins, score_win, path):
    tr = pd.concat([data[w] for w in train_wins], ignore_index=True)
    te = data[score_win]
    out = pd.DataFrame({"coin": te.coin, "order": te.order})
    X, Xt = tr[FEATS].fillna(0).values, te[FEATS].fillna(0).values
    for p, y in TARGETS.items():
        out[p] = model().fit(X, tr[y].values).predict_proba(Xt)[:, 1]
    out.to_csv(path, index=False)


t0 = time.time()
for w in WINS:
    others = [x for x in WINS if x != w]
    scores(others, w, f"{w}/scores_outer_{MODEL}.csv")
    for v in others:
        scores([x for x in others if x != v], v, f"{v}/scores_inner_excl_{w.split('/')[-1]}_{MODEL}.csv")
    print(f"{MODEL} {w} done ({time.time() - t0:.0f}s)", flush=True)
print("ALL_DONE", flush=True)
