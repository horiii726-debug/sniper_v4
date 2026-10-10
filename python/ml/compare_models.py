"""CatBoost (GPU) / RandomForest / ensemble on base or all features, same walk-forward protocol.
usage: compare_models.py DEV1,DEV2 TEST1,TEST2 MODEL FEATSET   (MODEL: catboost|randomforest|ensemble, FEATSET: base|all)
Writes scores_cross_<MODEL>_<FEATSET>.csv (dev) and scores_final_<MODEL>_<FEATSET>.csv (test)."""
import json, sys
import numpy as np, pandas as pd
from sklearn.metrics import average_precision_score, roc_auc_score

DEV, TEST, MODEL, FS = sys.argv[1].split(","), sys.argv[2].split(","), sys.argv[3], sys.argv[4]
META = {"coin", "order", "time", "y_up2", "y_up5", "y_up10", "y_cont", "y_rug", "y_fwd_max", "y_peak2", "y_peak5", "y_peak10", "y_end30", "y_dump30s", "y_devsell10m", "y_rugdead", "y_devdump"}
data = {d: pd.read_csv(f"{d}/snapshots.csv") for d in DEV + TEST}
# Rug = dies and never recovers within 30 min (a dump that later runs is NOT a rug: forensics show moonshots dump first).
for d in data.values():
    d["y_rugdead"] = ((d.y_end30 <= 0.2) & (d.y_peak2 == 0)).astype(int)
    # Dev risk = creator sells within 10 min AND the coin dies (devs of runners also sell some).
    d["y_devdump"] = ((d.y_devsell10m == 1) & (d.y_end30 <= 0.3) & (d.y_peak2 == 0)).astype(int)
FEATS = [c for c in data[DEV[0]].columns if c not in META and (FS == "all" or not c.startswith("f"))]
import os
LABEL = os.environ.get("LABEL", "barrier")
TARGETS = {"p_up2": "y_peak2", "p_up5": "y_peak5", "p_cont": "y_cont", "p_rug": "y_rugdead", "p_dump": "y_dump30s", "p_devsell": "y_devdump"} if LABEL == "peak" else {"p_up2": "y_up2", "p_up5": "y_up5", "p_cont": "y_cont", "p_rug": "y_rug"}

def fit_predict(name, tr, te_list, y):
    if name == "catboost":
        from catboost import CatBoostClassifier
        m = CatBoostClassifier(iterations=800, depth=6, learning_rate=0.05, l2_leaf_reg=3, task_type="GPU", devices="0", verbose=0, random_seed=7)
    else:
        from sklearn.ensemble import RandomForestClassifier
        m = RandomForestClassifier(n_estimators=300, min_samples_leaf=20, max_features="sqrt", n_jobs=12, random_state=7)
    m.fit(tr[FEATS].values, tr[y].values)
    return [m.predict_proba(te[FEATS].values)[:, 1] for te in te_list]

def predict(tr, te_list):
    outs = [pd.DataFrame({"coin": te.coin, "order": te.order}) for te in te_list]
    for p, y in TARGETS.items():
        if MODEL == "ensemble":
            a = fit_predict("catboost", tr, te_list, y); b = fit_predict("randomforest", tr, te_list, y)
            preds = [(x + z) / 2 for x, z in zip(a, b)]
        else:
            preds = fit_predict(MODEL, tr, te_list, y)
        for o, pr in zip(outs, preds):
            o[p] = pr
    for o in outs:
        o["p_up5"] = np.minimum(o.p_up5, o.p_up2)
    return [o[["coin", "order"] + list(TARGETS)] for o in outs]

tag = f"{MODEL}_{FS}" + ("_peak" if LABEL == "peak" else "") + "_v2"
for tr, te in [(DEV[0], DEV[1]), (DEV[1], DEV[0])]:
    predict(data[tr], [data[te]])[0].to_csv(f"{te}/scores_cross_{tag}.csv", index=False)
full = pd.concat([data[d] for d in DEV], ignore_index=True)
for te, sc in zip(TEST, predict(full, [data[t] for t in TEST])):
    sc.to_csv(f"{te}/scores_final_{tag}.csv", index=False)
    r = {p: (round(roc_auc_score(data[te][y], sc[p]), 3), round(average_precision_score(data[te][y], sc[p]), 3)) for p, y in TARGETS.items()}
    k = len(sc) // 100
    top = round(data[te].iloc[np.argsort(-sc.p_up2.values)[:k]].y_up2.mean(), 3)
    print(f"{tag:22s} {te.split('/')[-1]:5s} AUC/PR up2 {r['p_up2']} top1% {top} | rug {r['p_rug']} | cont {r['p_cont']}", flush=True)
