"""Online (self-updating) entry model: the model learns again every time the outcomes of N more coins are known.

usage: LABEL_H=3600 python3 python/ml/online_learn.py data/v5_A data/v5_B ... [--every 10] [--target y_2x]
       [--snap snapshots_v5.csv.zst] [--delay 3600] [--out reports/v6/ONLINE_LEARN.json]

Stream: all rows of the given windows in time order (windows are hours / days apart; each window's rows in their own
time order). A row's label is KNOWN only at row_time + delay (the label horizon): nothing is learned before that
(no leakage). The first window is the warm-up (base model, not scored). Afterwards:
  - static model: trained once on the warm-up window, never updated;
  - walk-forward model: retrained at the start of every window on all labels known by then (= v5_train folds);
  - online model: the walk-forward model, plus an update every time `--every` more coins have newly known labels
    inside the window (CatBoost continued training on the new rows + an equal-size replay sample of older rows).
Each row is scored by the model state at its own time. Metrics on the decision rows (is_signal & in_window) of every
scored window: ROC-AUC, PR-AUC, precision top 5 %, and the number of online updates. Acceptance (C17): online beats
walk-forward on PR-AUC on >= 60 % of scored windows; otherwise it is not integrated.
"""
import argparse
import json
import os
import sys
import time

import numpy as np
import pandas as pd
from sklearn.metrics import average_precision_score, roc_auc_score

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from v5_common import rust_str_array  # noqa: E402

FEATURES = rust_str_array("FEATURES")
TARGETS = {"y_2x": lambda d: d["y_bin"] >= 3, "y_5x": lambda d: d["y_bin"] >= 4, "y_10x": lambda d: d["y_bin"] >= 5,
           "y_s_2x": lambda d: d["y_s_2x"] == 1, "y_s_5x": lambda d: d["y_s_5x"] == 1}


def load(d, snap, target):
    df = pd.read_csv(os.path.join(d, snap), compression="zstd")
    st = json.load(open(os.path.join(d, "snapshots_v5_stats.json")))
    df["t_abs"] = df["time"] + st["window"][0]
    df = df.sort_values(["t_abs", "order"], kind="stable").reset_index(drop=True)
    X = df[FEATURES].to_numpy(np.float64)
    X[~np.isfinite(X)] = np.nan
    return {"name": os.path.basename(d.rstrip("/")), "X": X.astype(np.float32), "y": TARGETS[target](df).to_numpy(int),
            "t": df["t_abs"].to_numpy(np.int64), "coin": df["coin"].to_numpy(), "dec": ((df["is_signal"] == 1) & (df["in_window"] == 1)).to_numpy()}


def cb(iters, seed=0):
    from catboost import CatBoostClassifier
    return CatBoostClassifier(iterations=iters, depth=6, learning_rate=0.08, loss_function="Logloss", verbose=False, random_seed=seed,
                              thread_count=-1, allow_writing_files=False)


def weights(coins):
    _, inv, cnt = np.unique(coins, return_inverse=True, return_counts=True)
    return 1.0 / cnt[inv]


def metrics(y, p):
    out = {"rows": int(len(y)), "positives": int(y.sum())}
    if 0 < y.sum() < len(y):
        k = max(1, int(round(0.05 * len(y))))
        out.update(auc=float(roc_auc_score(y, p)), pr_auc=float(average_precision_score(y, p)), base=float(y.mean()),
                   top5=float(y[np.argsort(-p, kind="stable")[:k]].mean()))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--every", type=int, default=10, help="update after this many coins with newly known labels")
    ap.add_argument("--target", default="y_2x", choices=list(TARGETS))
    ap.add_argument("--snap", default="snapshots_v5.csv.zst")
    ap.add_argument("--delay", type=int, default=int(os.environ.get("LABEL_H", "3600")), help="label known at row time + delay (s)")
    ap.add_argument("--base-iters", type=int, default=400)
    ap.add_argument("--update-iters", type=int, default=20)
    ap.add_argument("--out", default="reports/v6/ONLINE_LEARN.json")
    a = ap.parse_args()
    W = [load(d, a.snap, a.target) for d in a.dirs]
    W.sort(key=lambda w: w["t"][0])
    rng = np.random.default_rng(0)
    # warm-up window: base model
    static = cb(a.base_iters).fit(W[0]["X"], W[0]["y"], sample_weight=weights(W[0]["coin"]))
    hist_X, hist_y, hist_c = [W[0]["X"]], [W[0]["y"]], [W[0]["coin"].astype(np.int64)]
    res = {"label": "EXPLORATORY online-learning gate C17", "target": a.target, "every_coins": a.every, "delay_s": a.delay, "windows": []}
    for wi, w in enumerate(W[1:], 1):
        t0 = time.time()
        # walk-forward: retrain on every label known at the window start (all earlier windows: they end > delay before)
        Xh, yh, ch = np.concatenate(hist_X), np.concatenate(hist_y), np.concatenate(hist_c)
        wf = cb(a.base_iters).fit(Xh, yh, sample_weight=weights(ch))
        p_static = static.predict_proba(w["X"])[:, 1]
        p_wf = wf.predict_proba(w["X"])[:, 1]
        # online: stream the window; a row's label is known at t + delay. Updates happen at the time the
        # `every`-th coin with newly known labels appears; rows are scored by the model state at their own time.
        p_on = p_wf.astype(np.float32).copy()
        model, n_upd = wf, 0
        seg_start = 0                      # rows [seg_start, next update time) are scored by the current model, once
        known_t = w["t"] + a.delay
        order_known = np.argsort(known_t, kind="stable")
        buf, coins = [], set()
        for r_i in order_known:
            tk = known_t[r_i]
            if tk > w["t"][-1]:
                break                      # known only after the last row of the window: useless inside it
            buf.append(r_i)
            coins.add(int(w["coin"][r_i]))
            if len(coins) < a.every:
                continue
            rows = np.array(buf)
            buf, coins = [], set()
            rep = rng.choice(len(yh), min(len(yh), len(rows)), replace=False)
            Xu = np.concatenate([w["X"][rows], Xh[rep]])
            yu = np.concatenate([w["y"][rows], yh[rep]])
            if not 0 < yu.sum() < len(yu):
                continue
            seg_end = int(np.searchsorted(w["t"], tk, side="left"))   # rows before the update time keep the old model
            if seg_end > seg_start:
                p_on[seg_start:seg_end] = model.predict_proba(w["X"][seg_start:seg_end])[:, 1]
            seg_start = max(seg_start, seg_end)
            model = cb(a.update_iters).fit(Xu, yu, init_model=model)
            n_upd += 1
            if n_upd % 100 == 0:
                print(f"  {w['name']}: {n_upd} updates, {time.time() - t0:.0f} s", flush=True)
        if seg_start < len(w["y"]):
            p_on[seg_start:] = model.predict_proba(w["X"][seg_start:])[:, 1]
        d = w["dec"]
        r = {"window": w["name"], "decision_rows": int(d.sum()), "online_updates": n_upd,
             "static": metrics(w["y"][d], p_static[d]), "walk_forward": metrics(w["y"][d], p_wf[d]), "online": metrics(w["y"][d], p_on[d]),
             "elapsed_s": round(time.time() - t0, 1)}
        res["windows"].append(r)
        print(json.dumps({k: (v if not isinstance(v, dict) else {kk: round(vv, 4) if isinstance(vv, float) else vv for kk, vv in v.items()})
                          for k, v in r.items()}), flush=True)
        hist_X.append(w["X"])
        hist_y.append(w["y"])
        hist_c.append(w["coin"].astype(np.int64) + 10_000_000 * wi)
    scored = [r for r in res["windows"] if "pr_auc" in r["online"] and "pr_auc" in r["walk_forward"]]
    wins = sum(r["online"]["pr_auc"] > r["walk_forward"]["pr_auc"] for r in scored)
    res["gate_C17"] = {"scored_windows": len(scored), "online_beats_walk_forward": wins,
                       "pass": bool(scored) and wins >= 0.6 * len(scored) and sum(r["online_updates"] for r in scored) > 0}
    os.makedirs(os.path.dirname(a.out), exist_ok=True)
    json.dump(res, open(a.out, "w"), indent=1)
    print(json.dumps(res["gate_C17"]))


if __name__ == "__main__":
    main()
