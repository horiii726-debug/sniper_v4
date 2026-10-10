"""Phase 5 parity fixtures: train toy CatBoost models on GPU, export per the shared model-file interface,
and write 10,000 rows + Python predictions for the Rust parity test (crates/model/tests/parity.rs).

Models (trained concurrently, one per GPU, so both cards are busy):
  binary      y_up2 (Logloss), depth 8, 1000 trees, border_count 254, nan_mode Min   -> GPU 0
  multiclass  7 classes from y_fwd_max (MultiClass), depth 7, 600 trees               -> GPU 1
  binary_nanmax  y_up5, depth 6, 300 trees, nan_mode Max (exercises AsTrue)          -> GPU 1
Each model gets P.cbm.json, P.meta.json, P.iso.json (sklearn IsotonicRegression fitted on a calibration holdout).

Output: target/model_parity/  (not committed)
  rows.csv              10,000 rows, header = available feature names (float32 values, '%.9g', NaN as 'nan')
  <name>.cbm.json / .meta.json / .iso.json
  <name>.preds.csv      raw_0..raw_{k-1}, cal_0..cal_{k-1}   (k = 1 binary, n_classes multiclass)
The multiclass model uses a shuffled subset of the columns, so the Rust test must go through FeatureMap.

Usage: python3 python/ml/model_parity.py [snapshots.csv.zst] [out_dir]
"""
import json
import multiprocessing as mp
import os
import sys
import time
from concurrent.futures import ProcessPoolExecutor

import numpy as np
import pandas as pd

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SRC = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "datasets/bf_1h/ml_snapshots.csv.zst")
OUT = sys.argv[2] if len(sys.argv) > 2 else os.path.join(ROOT, "target/model_parity")
N_PARITY = 10_000
SEED = 7
MC_EDGES = [1.01, 1.03, 1.1, 1.3, 2.0, 5.0]  # 7 classes of y_fwd_max (toy stand-in for y_bin)
CPU_THREADS = max(1, (os.cpu_count() or 2) // 2)


def load():
    df = pd.read_csv(SRC, compression="zstd")
    feats = [c for c in df.columns if c not in ("coin", "order", "time") and not c.startswith("y_")]
    X = df[feats].to_numpy(dtype=np.float32)
    rng = np.random.default_rng(SEED)
    # NaN injection: 40% of columns get 5-30% random NaNs, plus a few structured all-NaN blocks
    for j in rng.choice(len(feats), size=int(0.4 * len(feats)), replace=False):
        X[rng.random(len(X)) < rng.uniform(0.05, 0.30), j] = np.nan
    for j in rng.choice(len(feats), size=4, replace=False):
        X[X[:, 0] > np.nanmedian(X[:, 0]), j] = np.nan
    y_mc = np.searchsorted(np.array(MC_EDGES), df["y_fwd_max"].to_numpy(), side="right").astype(int)
    return feats, X, {"y_up2": df["y_up2"].to_numpy().astype(int), "y_up5": df["y_up5"].to_numpy().astype(int),
                      "y_mc7": y_mc}


def train_one(job):
    """Runs in a spawned process: train on one GPU, save P.cbm.json."""
    from catboost import CatBoostClassifier, Pool
    name, device, ram_part, params, feat_names, Xtr, ytr = job
    t = time.time()
    # boosting_type="Plain" explicitly: on small datasets CatBoost auto-switches to Ordered boosting once
    # iterations is large, which is ~10x slower on GPU (measured: 600 trees 48.5 s Ordered vs ~5 s Plain).
    m = CatBoostClassifier(task_type="GPU", devices=str(device), verbose=0, random_seed=SEED, boosting_type="Plain",
                           thread_count=CPU_THREADS, gpu_ram_part=ram_part, **params)
    m.fit(Pool(Xtr, ytr, feature_names=feat_names))
    m.save_model(os.path.join(OUT, name + ".cbm.json"), format="json")
    return name, time.time() - t, m.tree_count_


def fmt_rows(path, header, X):
    with open(path, "w") as f:
        f.write(",".join(header) + "\n")
        np.savetxt(f, X.astype(np.float64), fmt="%.9g", delimiter=",")  # 9 sig. digits round-trip float32


def main():
    os.makedirs(OUT, exist_ok=True)
    from catboost import CatBoost
    from sklearn.isotonic import IsotonicRegression

    feats, X, ys = load()
    n = len(X)
    rng = np.random.default_rng(SEED)
    perm = rng.permutation(n)
    n_par = min(N_PARITY, n // 4)
    par, rest = perm[:n_par], perm[n_par:]
    n_cal = len(rest) // 4
    cal, tr = rest[:n_cal], rest[n_cal:]

    mc_feats = list(rng.permutation(feats)[: int(0.8 * len(feats))])  # shuffled subset -> FeatureMap path
    mc_idx = [feats.index(f) for f in mc_feats]
    models = {
        "binary": dict(device=0, target="y_up2", feats=feats, kind="binary", n_classes=2,
                       params=dict(iterations=1000, depth=8, learning_rate=0.05, loss_function="Logloss",
                                   border_count=254)),
        "multiclass": dict(device=1, target="y_mc7", feats=mc_feats, kind="multiclass", n_classes=7,
                           params=dict(iterations=600, depth=7, learning_rate=0.08, loss_function="MultiClass")),
        "binary_nanmax": dict(device=1, target="y_up5", feats=feats, kind="binary", n_classes=2,
                              params=dict(iterations=300, depth=6, learning_rate=0.08, loss_function="Logloss",
                                          nan_mode="Max")),
    }
    jobs = []
    per_dev = {}
    for spec in models.values():
        per_dev[spec["device"]] = per_dev.get(spec["device"], 0) + 1
    for name, spec in models.items():
        cols = [feats.index(f) for f in spec["feats"]]
        ram_part = round(0.9 / per_dev[spec["device"]], 2)  # GPU memory split between jobs sharing a card
        jobs.append((name, spec["device"], ram_part, spec["params"], spec["feats"], X[np.ix_(tr, cols)], ys[spec["target"]][tr]))
    t0 = time.time()
    with ProcessPoolExecutor(max_workers=len(jobs), mp_context=mp.get_context("spawn")) as ex:
        for name, dt, nt in ex.map(train_one, jobs):
            print(f"trained {name}: {nt} trees in {dt:.1f}s", flush=True)
    print(f"all training (parallel, 2 GPUs) {time.time() - t0:.1f}s", flush=True)

    fmt_rows(os.path.join(OUT, "rows.csv"), feats, X[par])
    # Re-read the exact float32 values that Rust will see (guards against any formatting drift).
    Xpar = pd.read_csv(os.path.join(OUT, "rows.csv")).to_numpy(dtype=np.float32)
    assert np.array_equal(np.isnan(Xpar), np.isnan(X[par])) and np.allclose(np.nan_to_num(Xpar), np.nan_to_num(X[par]), 0, 0)

    report = {}
    for name, spec in models.items():
        P = os.path.join(OUT, name)
        cols = [feats.index(f) for f in spec["feats"]]
        m = CatBoost()
        m.load_model(P + ".cbm.json", format="json")  # predict from the exported JSON, exactly what Rust loads
        raw_cal = np.asarray(m.predict(X[np.ix_(cal, cols)], prediction_type="RawFormulaVal"), dtype=np.float64)
        raw_par = np.asarray(m.predict(Xpar[:, cols], prediction_type="RawFormulaVal"), dtype=np.float64)
        y_cal = ys[spec["target"]][cal]
        if spec["kind"] == "binary":
            p_cal = 1.0 / (1.0 + np.exp(-raw_cal))
            iso = IsotonicRegression(out_of_bounds="clip", y_min=0.0, y_max=1.0).fit(p_cal, y_cal)
            iso_json = {"x": iso.X_thresholds_.tolist(), "y": iso.y_thresholds_.tolist()}
            p_par = 1.0 / (1.0 + np.exp(-raw_par))
            cal_par = iso.predict(p_par)[:, None]
            raw_par = raw_par[:, None]
        else:
            def softmax(r):
                e = np.exp(r - r.max(axis=1, keepdims=True))
                return e / e.sum(axis=1, keepdims=True)
            p_cal, p_par = softmax(raw_cal), softmax(raw_par)
            isos = [IsotonicRegression(out_of_bounds="clip", y_min=0.0, y_max=1.0).fit(p_cal[:, k], (y_cal == k).astype(float))
                    for k in range(spec["n_classes"])]
            iso_json = {"classes": [{"x": i.X_thresholds_.tolist(), "y": i.y_thresholds_.tolist()} for i in isos]}
            c = np.column_stack([isos[k].predict(p_par[:, k]) for k in range(spec["n_classes"])])
            s = c.sum(axis=1, keepdims=True)
            cal_par = np.where(s > 0, c / np.where(s > 0, s, 1.0), p_par)
        meta = {"name": name, "kind": spec["kind"], "n_classes": spec["n_classes"], "features": spec["feats"],
                "target": spec["target"], "train_windows": ["toy:datasets/bf_1h"], "calibrated": True}
        json.dump(meta, open(P + ".meta.json", "w"), indent=1)
        json.dump(iso_json, open(P + ".iso.json", "w"))
        k = raw_par.shape[1]
        hdr = [f"raw_{i}" for i in range(k)] + [f"cal_{i}" for i in range(k)]
        with open(P + ".preds.csv", "w") as f:
            f.write(",".join(hdr) + "\n")
            np.savetxt(f, np.hstack([raw_par, cal_par]), fmt="%.17g", delimiter=",")
        report[name] = {"trees": m.tree_count_, "features": len(spec["feats"]),
                        "nan_share_rows": float(np.isnan(Xpar[:, cols]).mean()),
                        "raw_range": [float(raw_par.min()), float(raw_par.max())]}
    json.dump(report, open(os.path.join(OUT, "report.json"), "w"), indent=1)
    print(json.dumps(report))
    print(f"fixtures -> {OUT}")


if __name__ == "__main__":
    main()
