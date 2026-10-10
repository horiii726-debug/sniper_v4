"""Experiment: target the value we can actually monetise. Leave-one-window-out CatBoost (GPU):
  A) classifier P(end30 >= 1.5)   B) regressor E[log(end30)]
Then one bet per coin at the first snapshot whose score passes a threshold; outcome end30 net of costs."""
import sys
import numpy as np, pandas as pd
from catboost import CatBoostClassifier, CatBoostRegressor
WINS = sys.argv[1].split(",")
META = {"coin", "order", "time", "y_up2", "y_up5", "y_up10", "y_cont", "y_rug", "y_fwd_max", "y_peak2", "y_peak5", "y_peak10",
        "y_end30", "y_dump30s", "y_devsell10m"}
data = {w: pd.read_csv(f"{w}/snapshots.csv").sort_values("order") for w in WINS}
FEATS = [c for c in data[WINS[0]].columns if c not in META]
rows = []
for w in WINS:
    tr = pd.concat([data[x] for x in WINS if x != w], ignore_index=True)
    te = data[w].copy()
    ya = (tr.y_end30 >= 1.5).astype(int)
    clf = CatBoostClassifier(iterations=600, depth=6, learning_rate=0.06, task_type="GPU", devices="0", verbose=0, random_seed=7).fit(tr[FEATS].values, ya)
    reg = CatBoostRegressor(iterations=600, depth=6, learning_rate=0.06, task_type="GPU", devices="0", verbose=0, random_seed=7,
                            loss_function="RMSE").fit(tr[FEATS].values, np.log(tr.y_end30.clip(lower=0.01)))
    te["p_end"] = clf.predict_proba(te[FEATS].values)[:, 1]
    te["e_log"] = reg.predict(te[FEATS].values)
    te[["coin", "order", "p_end", "e_log"]].to_csv(f"{w}/scores_endvalue_outer.csv", index=False)
    for col, ths in [("p_end", [0.2, 0.3, 0.4, 0.5]), ("e_log", [-0.3, -0.1, 0.0, 0.1])]:
        for T in ths:
            first = te[te[col] >= T].groupby("coin").first()
            if len(first) < 3:
                rows.append({"window": w.split("/")[-1], "score": col, "thr": T, "bets": len(first)}); continue
            m = first.y_end30.clip(lower=0); net = m * 0.975 - 1 - 0.02
            rows.append({"window": w.split("/")[-1], "score": col, "thr": T, "bets": len(first), "mean_end30": round(m.mean(), 3),
                         "median_end30": round(m.median(), 3), "p_le0.2": round((m <= 0.2).mean(), 3), "mean_net": round(net.mean(), 3),
                         "mean_net_excl_top1": round(net.sort_values().iloc[:-1].mean(), 3)})
pd.set_option("display.width", 220, "display.max_rows", 200)
print(pd.DataFrame(rows).to_string(index=False))
