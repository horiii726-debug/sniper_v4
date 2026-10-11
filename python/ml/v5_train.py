#!/usr/bin/env python3
"""V5 Phase 4 model training (offline, EXPLORATORY research; V5_UPGRADE_PLAN.md section 5).

  python3 python/ml/v5_train.py data/v5_A data/v5_B ... [--min-train 3] [--out models/v5] [--quick]

Walk-forward with purge: test window W is scored by models trained only on windows whose data_end <= W.start - H.
A final fold is trained on all windows. Every (fold, model) fit runs as an independent task: CatBoost (task_type GPU)
and the XGBoost random forest (device cuda) are spread over both GPUs with several fits in flight per card, the
sklearn random forest / logistic baseline run on the CPU pool at the same time. Each task:
  1. inner split of the fold's training windows by time (last ~25% = calibration windows, purged by H),
  2. fit on the inner part (CatBoost early-stops on the calibration windows), isotonic fit on calibration predictions,
  3. refit on all training windows (CatBoost: early-stopped iteration count), save files, score the test window.
"""
import argparse
import gc
import json
import multiprocessing as mp
import os
import shutil
import subprocess
import sys
import threading
import time
import warnings
from concurrent.futures import ProcessPoolExecutor

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from v5_common import (H_S, IDENTITY, TEN_X, apply_iso_binary, apply_iso_multi, binary_metrics, feature_columns,  # noqa: E402
                       fit_iso_table, multiclass_logloss, top_precision, window_label, write_json)

LABELS = ["y_tp1", "y_bin", "y_peak_barrier", "y_hold", "y_dead_10m", "y_logmax", "y_end_h", "y_rugdead", "y_t2x"]
N_BIN = 7

# name -> (algo, kind, target, rows, weighted, depth)
MODELS = {
    "entry_bin": ("cb", "multiclass", "y_bin", "signal", True, 6),
    "entry_tp1": ("cb", "binary", "y_tp1", "signal", True, 6),
    "rug_cb": ("cb", "binary", "y_rugdead", "all", False, 8),
    "rug_rfgpu": ("xgbrf", "binary", "y_rugdead", "all", False, 12),
    "rug_rfcpu": ("skrf", "binary", "y_rugdead", "all", False, None),
    "hold": ("cb", "binary", "y_hold", "all", False, 8),
    "hazard": ("cb", "binary", "y_dead_10m", "hazard", False, 7),
    "baseline_lr": ("lr", "binary", "y_tp1", "signal", False, None),
}
COST = {"cb": 1.0, "xgbrf": 1.2, "skrf": 1.0, "lr": 0.1}


def log(*a):
    print(time.strftime("%H:%M:%S"), *a, flush=True)


# ============================================================== data cache (main process)
CACHE_VERSION = 2   # bump when the cached row definition / info keys change (V5-10)


def entry_rows(cols):
    """V5-10: entry-model rows = decision rows inside the backfill window (the strategy enters only there; after the
    window the regime features are -1). Rug / hold / hazard models keep all rows (they serve open positions)."""
    return (cols["is_signal"] == 1) & (cols["in_window"] == 1)


def load_window(path, cache_root):
    """Parse snapshots_v5.csv.zst once into an mmap-able cache: X.npy (float32) + cols.npz."""
    import pandas as pd
    lab = window_label(path)
    src = os.path.join(path, "snapshots_v5.csv.zst")
    stats = json.load(open(os.path.join(path, "snapshots_v5_stats.json")))
    st = os.stat(src)
    cdir = os.path.join(cache_root, lab)
    info_p = os.path.join(cdir, "info.json")
    if os.path.exists(info_p):
        info = json.load(open(info_p))
        if (info.get("cache_version") == CACHE_VERSION and info.get("src_size") == st.st_size
                and info.get("src_mtime") == st.st_mtime and info.get("src") == os.path.abspath(src)):
            return info
    t0 = time.time()
    with __import__("zstandard").open(src, "rt") as f:
        header = f.readline().strip().split(",")
    feats = feature_columns(header)
    assert header[:5] == ["coin", "order", "time", "is_signal", "in_window"], header[:5]
    i_y = header.index("y_tp1")
    # prefix check: later work packages may append label columns after the V5 labels
    assert header[i_y:i_y + len(LABELS)] == LABELS, header[i_y:]
    dt = {c: np.float32 for c in feats}
    dt.update({"coin": np.int64, "order": np.uint64, "time": np.int64, "is_signal": np.int8, "in_window": np.int8})
    dt.update({c: np.float64 for c in LABELS})
    df = pd.read_csv(src, compression="zstd", dtype=dt, engine="c")
    os.makedirs(cdir, exist_ok=True)
    np.save(os.path.join(cdir, "X.npy"), np.ascontiguousarray(df[feats].to_numpy(np.float32)))
    minute = np.floor_divide(df["time"].to_numpy(), 60)
    hazard = ~pd.DataFrame({"c": df["coin"].to_numpy(), "m": minute}).duplicated(keep="first").to_numpy()
    cols = {c: df[c].to_numpy() for c in ["coin", "order", "time", "is_signal", "in_window"] + LABELS}
    cols["hazard"] = hazard
    np.savez(os.path.join(cdir, "cols.npz"), **cols)
    assert len(np.unique(cols["order"])) == len(df), f"{path}: order is not unique"
    info = {"label": lab, "path": os.path.abspath(path), "src": os.path.abspath(src), "src_size": st.st_size,
            "src_mtime": st.st_mtime, "start": int(stats["window"][0]), "end": int(stats["window"][1]),
            "data_end": int(stats["data_end"]), "rows": int(len(df)), "signal_rows": int(entry_rows(cols).sum()),
            "signal_rows_all": int(cols["is_signal"].sum()), "cache_version": CACHE_VERSION,
            "hazard_rows": int(hazard.sum()), "features": feats, "load_s": round(time.time() - t0, 1)}
    write_json(info_p, info)
    return info


# ============================================================== worker side
_W = {"cache": None, "data": {}, "infos": {}, "dev": None, "threads": 4, "rf_jobs": -1, "gpu_total_mib": 24564}


def _init_worker(dev, cache_root, infos, threads, rf_jobs, gpu_total_mib):
    # Pin the process to one card before CUDA is touched: inside the worker that card is device 0.
    os.environ["CUDA_VISIBLE_DEVICES"] = "" if dev is None else str(dev)
    _W.update(cache=cache_root, infos=infos, dev=dev, threads=threads, rf_jobs=rf_jobs, gpu_total_mib=gpu_total_mib)
    warnings.filterwarnings("ignore")


def _proc_entry(task, dev, cache_root, infos, threads, rf_jobs, gpu_total_mib, result_path):
    """One task = one fresh process: GPU memory is fully released and a CUDA crash cannot take others down."""
    import pickle
    import traceback
    _init_worker(dev, cache_root, infos, threads, rf_jobs, gpu_total_mib)
    parent = os.getppid()

    def orphan_watch():  # exit if the scheduler dies, so no fit keeps a GPU busy forever
        while True:
            time.sleep(2)
            if os.getppid() != parent:
                os._exit(3)
    threading.Thread(target=orphan_watch, daemon=True).start()
    try:
        res = {"ok": True, "res": run_task(task)}
    except BaseException:
        res = {"ok": False, "error": traceback.format_exc()[-3000:]}
    with open(result_path + ".tmp", "wb") as f:
        pickle.dump(res, f)
    os.replace(result_path + ".tmp", result_path)


def _win(lab):
    if lab not in _W["data"]:
        d = os.path.join(_W["cache"], lab)
        X = np.load(os.path.join(d, "X.npy"), mmap_mode="r")
        cols = dict(np.load(os.path.join(d, "cols.npz")))
        _W["data"][lab] = (X, cols)
    return _W["data"][lab]


def _rows(cols, kind):
    if kind == "signal":
        return entry_rows(cols)
    if kind == "hazard":
        return cols["hazard"].astype(bool)
    return np.ones(len(cols["order"]), bool)


def gather(segments, kind, target):
    """segments: [(window, t_lo, t_hi)] -> X, y, group key (window, coin)."""
    Xs, ys, ks = [], [], []
    for wi, (lab, lo, hi) in enumerate(segments):
        X, cols = _win(lab)
        m = _rows(cols, kind) & (cols["time"] >= lo) & (cols["time"] < hi) & ~np.isnan(cols[target])
        idx = np.flatnonzero(m)
        Xs.append(np.asarray(X[idx], dtype=np.float32))
        ys.append(cols[target][idx].astype(np.int64))
        ks.append((np.int64(wi) << 40) + cols["coin"][idx])
    return np.concatenate(Xs), np.concatenate(ys), np.concatenate(ks)


def entry_weights(y, key, n_classes):
    """Balanced class weights (capped at 50x the smallest) x 1/rows of that (window, coin); mean 1."""
    _, inv, cnt = np.unique(key, return_inverse=True, return_counts=True)
    coin_w = 1.0 / cnt[inv]
    n_c = np.bincount(y, minlength=n_classes).astype(float)
    present = n_c > 0
    cw = np.zeros(n_classes)
    cw[present] = len(y) / (present.sum() * n_c[present])
    cw = np.minimum(cw, 50.0 * cw[present].min())
    w = cw[y] * coin_w
    return w / w.mean()


def ensure_classes(X, y, w, n_classes):
    """Append one tiny-weight dummy row (median features) per class missing from the training labels."""
    miss = [c for c in range(n_classes) if not (y == c).any()]
    if not miss:
        return X, y, w, []
    med = np.nanmedian(X, axis=0) if len(X) else np.zeros(X.shape[1], np.float32)
    med = np.where(np.isnan(med), 0.0, med).astype(np.float32)
    X = np.vstack([X, np.tile(med, (len(miss), 1))])
    y = np.concatenate([y, np.array(miss, dtype=y.dtype)])
    w = np.concatenate([w, np.full(len(miss), 1e-6)])
    return X, y, w, miss


def finite(X, fill_nan=None):
    X = np.where(np.isposinf(X), 1e30, np.where(np.isneginf(X), -1e30, X)).astype(np.float32)
    if fill_nan is not None:
        X = np.where(np.isnan(X), fill_nan, X).astype(np.float32)
    return X


class Fitted:
    def __init__(self, algo, model, best_iter=None):
        self.algo, self.model, self.best_iter = algo, model, best_iter

    def predict(self, X, kind):
        if self.algo == "cb":
            P = self.model.predict_proba(X)
        elif self.algo == "xgbrf":
            P = self.model.predict_proba(finite(X))
        elif self.algo == "skrf":
            P = self.model.predict_proba(finite(X, -999.0))
        else:
            P = self.model.predict_proba(finite(X))
        P = np.asarray(P, dtype=np.float64)
        return P if kind == "multiclass" else P[:, 1]


def need_mib(algo, rows, n_feat, n_classes):
    """GPU memory budget of one fit (measured: CatBoost 1.5M x 106 depth 8 fits in 2.4 GB, XGBRF peaks at 3.4 GB)."""
    if algo == "cb":
        return int(2048 + rows * (n_feat * 6 + 128 * n_classes) / 2 ** 20)
    if algo == "xgbrf":
        return int(2048 + rows * n_feat * 10 / 2 ** 20)
    return 0


def _cb_ram_part(rows, n_feat, n_classes):
    """CatBoost pre-allocates gpu_ram_part x total memory; reserve only what the data needs so several fits, the
    XGBoost forest and other users of the card fit side by side."""
    need = max(need_mib("cb", rows, n_feat, n_classes), _W.get("need_mib", 0))
    return float(min(0.6, max(0.04, need / _W["gpu_total_mib"])))


def fit_model(name, Xtr, ytr, wtr, Xev=None, yev=None, wev=None, iters=None, quick=False):
    algo, kind, target, rows, weighted, depth = MODELS[name]
    n_classes = N_BIN if kind == "multiclass" else 2
    Xtr, ytr, wtr, miss = ensure_classes(Xtr, ytr, wtr, n_classes)
    if algo == "cb":
        from catboost import CatBoostClassifier, Pool
        early = Xev is not None and len(yev) > 0
        p = dict(loss_function="MultiClass" if kind == "multiclass" else "Logloss", class_names=list(range(n_classes)),
                 iterations=iters or (200 if quick else 3000), depth=depth, learning_rate=0.05, l2_leaf_reg=3,
                 border_count=254, task_type="GPU", devices="0", random_seed=42, verbose=0,
                 gpu_ram_part=_cb_ram_part(len(ytr) + (len(yev) if early else 0), Xtr.shape[1], n_classes),
                 thread_count=_W["threads"], allow_writing_files=False)
        if early:
            p.update(early_stopping_rounds=50 if quick else 200, use_best_model=True)
        m = CatBoostClassifier(**p)
        m.fit(Pool(Xtr, ytr, weight=wtr), eval_set=Pool(Xev, yev, weight=wev) if early else None)
        best = m.get_best_iteration() if early else None
        return Fitted("cb", m, (best if best is not None else m.tree_count_ - 1)), miss
    if algo == "xgbrf":
        import xgboost as xgb
        pos = max(1.0, float((ytr == 1).sum()))
        m = xgb.XGBRFClassifier(device="cuda", tree_method="hist", n_estimators=50 if quick else 500, subsample=0.8,
                                colsample_bynode=0.8, max_depth=12, max_bin=256, learning_rate=1.0,
                                scale_pos_weight=float((ytr == 0).sum()) / pos, random_state=42, n_jobs=_W["threads"])
        m.fit(finite(Xtr), ytr, sample_weight=wtr)
        return Fitted("xgbrf", m), miss
    if algo == "skrf":
        from sklearn.ensemble import RandomForestClassifier
        m = RandomForestClassifier(n_estimators=50 if quick else 500, min_samples_leaf=20, class_weight="balanced",
                                   n_jobs=_W["rf_jobs"], random_state=42)
        m.fit(finite(Xtr, -999.0), ytr, sample_weight=wtr)
        return Fitted("skrf", m), miss
    from sklearn.impute import SimpleImputer
    from sklearn.linear_model import LogisticRegression
    from sklearn.pipeline import make_pipeline
    from sklearn.preprocessing import StandardScaler
    m = make_pipeline(SimpleImputer(strategy="median", keep_empty_features=True), StandardScaler(),
                      LogisticRegression(max_iter=2000))
    m.fit(finite(Xtr), ytr)
    return Fitted("lr", m), miss


def inner_split(train):
    """train: window labels in time order -> (inner_train segments, calibration segments, note)."""
    inf = float("inf")
    infos = _W["infos"]
    if len(train) == 1:  # single window: time-ordered row split inside it
        _, cols = _win(train[0])
        t75 = float(np.quantile(cols["time"], 0.75)) if len(cols["time"]) else 0.0
        return [(train[0], -inf, t75)], [(train[0], t75, inf)], "single window: time split at 75% quantile (no purge)"
    n_cal = min(len(train) - 1, max(1, int(round(0.25 * len(train)))))
    cal, rest = train[-n_cal:], train[:-n_cal]
    c0 = infos[cal[0]]["start"]
    inner = [w for w in rest if infos[w]["data_end"] <= c0 - H_S]
    note = f"inner train {len(inner)} windows (purged), calibration {cal}"
    if not inner:
        inner = rest
        note = f"inner purge relaxed (no window ends >= H before {cal[0]}); calibration {cal}"
    return [(w, -inf, inf) for w in inner], [(w, -inf, inf) for w in cal], note


def calibrate(kind, P, y):
    """Isotonic tables from inner out-of-sample predictions; None when the set is too small."""
    if kind == "binary":
        if len(y) < 200 or (y == 1).sum() < 10:
            return None, 0
        return fit_iso_table(P, (y == 1).astype(float)), 1
    if len(y) < 200:
        return None, 0
    tabs, fitted = [], 0
    for k in range(N_BIN):
        yk = (y == k).astype(float)
        if yk.sum() >= 10:
            tabs.append(fit_iso_table(P[:, k], yk))
            fitted += 1
        else:
            tabs.append(dict(IDENTITY))
    return ({"classes": tabs}, fitted) if fitted else (None, 0)


def run_task(t):
    t0 = time.time()
    name, fold, train, test, prefix, quick = t["model"], t["fold"], t["train"], t["test"], t["prefix"], t["quick"]
    algo, kind, target, rows, weighted, _ = MODELS[name]
    n_classes = N_BIN if kind == "multiclass" else 2
    inf = float("inf")
    res = {"model": name, "fold": fold, "algo": algo, "device": _W["dev"], "pid": os.getpid()}
    _W["need_mib"] = t.get("need_mib", 0)

    def weights(y, key):
        return entry_weights(y, key, n_classes) if weighted else np.ones(len(y))

    # 1-2. inner fit + isotonic calibration on the time-ordered calibration windows
    seg_in, seg_cal, note = inner_split(train)
    Xa, ya, ka = gather(seg_in, rows, target)
    Xc, yc, kc = gather(seg_cal, rows, target)
    res.update(inner_note=note, inner_rows=int(len(ya)), cal_rows=int(len(yc)))
    iso, best = None, None
    if len(ya) > 0:
        f, miss = fit_model(name, Xa, ya, weights(ya, ka), Xc, yc, weights(yc, kc) if len(yc) else None, quick=quick)
        best = f.best_iter
        if len(yc):
            iso, n_fit = calibrate(kind, f.predict(Xc, kind), yc)
            res["iso_classes_fitted"] = n_fit
        del f
    del Xa, Xc
    gc.collect()
    res["inner_s"] = round(time.time() - t0, 1)

    # 3. refit on all training windows
    t1 = time.time()
    Xt, yt, kt = gather([(w, -inf, inf) for w in train], rows, target)
    iters = (best + 1) if (algo == "cb" and best is not None) else None
    f, miss = fit_model(name, Xt, yt, weights(yt, kt), iters=iters, quick=quick)
    res.update(train_rows=int(len(yt)), train_pos=int((yt == 1).sum()) if kind == "binary" else
               np.bincount(yt, minlength=N_BIN).tolist(), dummy_classes=miss, best_iteration=best,
               refit_iterations=(int(f.model.tree_count_) if algo == "cb" else None), calibrated=iso is not None)
    del Xt
    feats = _W["infos"][train[0]]["features"]
    meta = {"name": name, "kind": kind, "n_classes": n_classes, "features": feats, "target": target,
            "train_windows": [os.path.basename(_W["infos"][w]["path"]) for w in train], "calibrated": iso is not None}
    os.makedirs(os.path.dirname(prefix), exist_ok=True)
    if algo == "cb":
        f.model.save_model(prefix + ".cbm.json", format="json")
        imp = f.model.get_feature_importance()
    elif algo == "xgbrf":
        f.model.save_model(prefix + ".rf.json")
        imp = f.model.feature_importances_
    elif algo == "skrf":
        import joblib
        joblib.dump(f.model, prefix + ".rf.joblib", compress=3)
        imp = f.model.feature_importances_
    else:
        import joblib
        joblib.dump(f.model, prefix + ".lr.joblib")
        imp = None
    write_json(prefix + ".meta.json", meta)
    if iso is not None:
        write_json(prefix + ".iso.json", iso)
    elif os.path.exists(prefix + ".iso.json"):
        os.remove(prefix + ".iso.json")
    res["importance"] = None if imp is None else [float(v) for v in imp]
    write_json(prefix + ".train.json", {k: v for k, v in res.items() if k != "importance"})
    res["refit_s"] = round(time.time() - t1, 1)

    # score every snapshot row of the test window
    if test is not None:
        X, cols = _win(test)
        raw = f.predict(np.asarray(X, dtype=np.float32), kind)
        cal = raw if iso is None else (apply_iso_multi(raw, iso) if kind == "multiclass" else apply_iso_binary(raw, iso))
        np.savez(os.path.join(os.path.dirname(prefix), "_parts", name + ".npz"), raw=raw, cal=cal)
    del f
    gc.collect()
    res["wall_s"] = round(time.time() - t0, 1)
    return res


# ============================================================== monitoring
class Sampler(threading.Thread):
    def __init__(self, period=1.0):
        super().__init__(daemon=True)
        self.period, self.samples, self.cpu, self.stop_ev = period, [], [], threading.Event()

    @staticmethod
    def _cpu_ticks():
        v = list(map(int, open("/proc/stat").readline().split()[1:]))
        return sum(v), v[3] + v[4]

    def run(self):
        t0 = time.time()
        last = self._cpu_ticks()
        while not self.stop_ev.is_set():
            try:
                out = subprocess.run(["nvidia-smi", "--query-gpu=index,utilization.gpu,memory.used",
                                      "--format=csv,noheader,nounits"], capture_output=True, text=True, timeout=5).stdout
                row = {"t": round(time.time() - t0, 1)}
                for line in out.strip().splitlines():
                    i, u, mem = [x.strip() for x in line.split(",")]
                    row[f"gpu{i}"], row[f"mem{i}"] = int(u), int(mem)
                self.samples.append(row)
                cur = self._cpu_ticks()
                tot, idle = cur[0] - last[0], cur[1] - last[1]
                self.cpu.append(round(100.0 * (1 - idle / tot), 1) if tot > 0 else 0.0)
                last = cur
            except Exception:
                pass
            self.stop_ev.wait(self.period)

    def summary(self):
        s = {"n_samples": len(self.samples), "period_s": self.period}
        for g in sorted({k for r in self.samples for k in r if k.startswith("gpu")}):
            u = np.array([r[g] for r in self.samples if g in r], float)
            m = np.array([r["mem" + g[3:]] for r in self.samples if g in r], float)
            s[g] = {"util_mean": round(float(u.mean()), 1), "util_p50": float(np.median(u)), "util_max": float(u.max()),
                    "share_ge50": round(float((u >= 50).mean()), 3), "share_ge90": round(float((u >= 90).mean()), 3),
                    "mem_max_mib": float(m.max())}
        if self.cpu:
            c = np.array(self.cpu)
            s["cpu"] = {"util_mean": round(float(c.mean()), 1), "util_max": float(c.max())}
        return s


# ============================================================== scheduler
def run_scheduler(ctx, tasks, gpus, gpu_slots, cpu_slots, cache, infos, threads, rf_jobs, gtot, tdir, sampler, max_attempts=5):
    """Largest task first to whichever card has a free slot (several fits in flight per card); CPU tasks in parallel.
    Each task runs in its own spawned process; a failed or crashed task is retried."""
    import pickle
    os.makedirs(tdir, exist_ok=True)
    gq = sorted([t for t in tasks if t["gpu"]], key=lambda x: -x["cost"])
    cq = sorted([t for t in tasks if not t["gpu"]], key=lambda x: -x["cost"])
    running, results, errors, n_total, seq = {}, [], [], len(tasks), 0
    log(f"{n_total} tasks: {len(gq)} GPU fits on cards {gpus} x {gpu_slots} slots (CatBoost thread_count {threads}), "
        f"{len(cq)} CPU fits x {cpu_slots} workers (sklearn RF n_jobs {rf_jobs})")

    def launch(t, dev):
        """Start one task in a fresh process pinned to card `dev` (None = CPU)."""
        nonlocal seq
        seq += 1
        t.setdefault("attempt", 0)
        t["attempt"] += 1
        rp = os.path.join(tdir, f"{seq:04d}_{t['fold']}_{t['model']}.pkl")
        if os.path.exists(rp):
            os.remove(rp)
        pr = ctx.Process(target=_proc_entry, args=(t, dev, cache, infos, threads, rf_jobs, gtot.get(dev, 24564), rp), daemon=False)
        pr.start()
        running[pr] = (t, dev, rp, time.time())

    free_cache = {"t": 0.0, "v": {}}

    def free_mib():
        if time.time() - free_cache["t"] > 1.0:
            try:
                q = subprocess.run(["nvidia-smi", "--query-gpu=index,memory.free", "--format=csv,noheader,nounits"],
                                   capture_output=True, text=True, timeout=10).stdout
                free_cache["v"] = {int(x.split(",")[0]): int(x.split(",")[1]) for x in q.strip().splitlines()}
            except Exception:
                pass
            free_cache["t"] = time.time()
        return free_cache["v"]

    def pick(dev):
        """First (largest) queued GPU task whose memory budget fits the card now; fits launched in the last 20 s
        may not have allocated yet, so their budgets are subtracted."""
        fm = free_mib()
        if dev not in fm:
            return gq.pop(0)
        pend = sum(v[0]["need_mib"] for v in running.values() if v[1] == dev and time.time() - v[3] < 20)
        for i, t in enumerate(gq):
            if fm[dev] - pend >= t["need_mib"] + 1024:
                return gq.pop(i)
        return None

    last_log, waited = time.time(), 0.0
    while gq or cq or running:
        for pr in [p for p in running if p.exitcode is not None]:
            t, dev, rp, ts = running.pop(pr)
            pr.join()
            where = "cpu" if dev is None else f"gpu{dev}"
            out = pickle.load(open(rp, "rb")) if os.path.exists(rp) else {"ok": False, "error": f"process exit code {pr.exitcode}"}
            if out["ok"]:
                r = out["res"]
                r.update(pool=where, attempts=t["attempt"])
                results.append(r)
                last = sampler.samples[-1] if sampler.samples else {}
                log(f"done {t['fold']}/{t['model']} on {where} in {r['wall_s']}s best_iter={r.get('best_iteration')} "
                    f"calibrated={r['calibrated']} | {len(results)}/{n_total} | "
                    + " ".join(f"{k}={v}%" for k, v in last.items() if k.startswith("gpu")))
            else:
                log(f"FAILED {t['fold']}/{t['model']} on {where} (attempt {t['attempt']}): {out['error'][-600:]}")
                if t["attempt"] < max_attempts:
                    if t["gpu"]:
                        t["need_mib"] = int(t["need_mib"] * 1.5)  # likely out of memory: ask for more next time
                    (gq if t["gpu"] else cq).insert(0, t)
                else:
                    errors.append(f"{t['fold']}/{t['model']}: {out['error'][-600:]}")
        for dev in sorted(gpus, key=lambda g: sum(1 for v in running.values() if v[1] == g)):
            while gq and sum(1 for v in running.values() if v[1] == dev) < gpu_slots:
                t = pick(dev)
                if t is None:
                    if time.time() - waited > 30:
                        waited = time.time()
                        log(f"gpu{dev}: waiting for free memory ({free_mib().get(dev)} MiB free; other processes use the card)")
                    break
                launch(t, dev)
        while cq and sum(1 for v in running.values() if v[1] is None) < cpu_slots:
            launch(cq.pop(0), None)
        if time.time() - last_log > 30:
            last_log = time.time()
            last = sampler.samples[-1] if sampler.samples else {}
            log(f"running {[(v[0]['fold'] + '/' + v[0]['model'], v[1]) for v in running.values()]} | {len(results)}/{n_total} done | "
                + " ".join(f"{k}={v}%" for k, v in last.items() if k.startswith("gpu"))
                + f" cpu={sampler.cpu[-1] if sampler.cpu else '?'}%")
        time.sleep(0.3)
    shutil.rmtree(tdir, ignore_errors=True)
    if errors:
        raise SystemExit("task failures:\n" + "\n".join(errors))
    return results


# ============================================================== main
def plan_folds(infos, min_train):
    order = sorted(infos, key=lambda w: infos[w]["start"])
    folds, skipped = [], []
    for i, w in enumerate(order):
        if i < min_train:
            continue
        s = infos[w]["start"]
        train = [v for v in order if v != w and infos[v]["start"] < s and infos[v]["data_end"] <= s - H_S]
        excluded = [v for v in order[:i] if v not in train]
        if len(train) < min_train:
            skipped.append({"test": w, "qualifying_train": train, "excluded_by_purge": excluded})
            log(f"SKIP fold {w}: only {len(train)} training windows end >= H before its start (need {min_train}); purged {excluded}")
            continue
        folds.append({"name": f"fold_{w}", "test": w, "train": train, "excluded_by_purge": excluded})
    folds.append({"name": "final", "test": None, "train": order, "excluded_by_purge": []})
    return order, folds, skipped


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("windows", nargs="+")
    ap.add_argument("--min-train", type=int, default=3)
    ap.add_argument("--out", default="models/v5")
    ap.add_argument("--reports", default="reports")
    ap.add_argument("--quick", action="store_true", help="200 CatBoost iterations / 50 trees (tests)")
    ap.add_argument("--gpus", default="0,1")
    ap.add_argument("--gpu-slots", type=int, default=3, help="concurrent fits per GPU")
    ap.add_argument("--cpu-workers", type=int, default=2, help="concurrent CPU tasks (sklearn RF, logistic)")
    ap.add_argument("--cb-threads", type=int, default=0, help="CatBoost thread_count per GPU fit (0 = auto)")
    ap.add_argument("--rf-cpu-jobs", type=int, default=0,
                    help="sklearn RF n_jobs (0 = all cores not reserved for feeding the GPU fits; -1 = every core)")
    ap.add_argument("--models", default=",".join(MODELS))
    a = ap.parse_args()
    T0 = time.time()
    sampler = Sampler()
    sampler.start()
    models = [m for m in a.models.split(",") if m]
    for m in models:
        assert m in MODELS, m
    out = os.path.abspath(a.out)
    cache = os.path.join(out, "_cache")
    os.makedirs(cache, exist_ok=True)
    ctx = mp.get_context("spawn")

    # ---- load
    t = time.time()
    with ProcessPoolExecutor(min(len(a.windows), 16), mp_context=ctx) as ex:
        infos = {i["label"]: i for i in ex.map(load_window, a.windows, [cache] * len(a.windows))}
    assert len(infos) == len(a.windows), "duplicate window labels"
    feats = next(iter(infos.values()))["features"]
    for i in infos.values():
        assert i["features"] == feats, f"{i['label']}: feature columns differ"
    load_s = time.time() - t
    log(f"loaded {len(infos)} windows, {sum(i['rows'] for i in infos.values())} rows, {len(feats)} features in {load_s:.1f}s")
    order, folds, skipped = plan_folds(infos, a.min_train)
    for f in folds:
        log(f"{f['name']}: train {f['train']} excluded_by_purge {f['excluded_by_purge']}")

    # ---- tasks
    tasks = []
    for f in folds:
        fdir = os.path.join(out, f["name"])
        if os.path.isdir(os.path.join(fdir, "_parts")):
            shutil.rmtree(os.path.join(fdir, "_parts"))
        os.makedirs(os.path.join(fdir, "_parts"), exist_ok=True)
        for m in models:
            algo, kind, target, rows, _, _ = MODELS[m]
            n = sum(infos[w]["signal_rows" if rows == "signal" else "hazard_rows" if rows == "hazard" else "rows"] for w in f["train"])
            cost = n * COST[algo] * (3.0 if kind == "multiclass" else 1.0)
            tasks.append({"model": m, "fold": f["name"], "train": f["train"], "test": f["test"], "quick": a.quick,
                          "prefix": os.path.join(fdir, m), "cost": cost, "gpu": algo in ("cb", "xgbrf"),
                          "need_mib": need_mib(algo, n, len(feats), N_BIN if kind == "multiclass" else 2)})
    gpus = [int(g) for g in a.gpus.split(",") if g != ""]
    ncpu = os.cpu_count() or 8
    threads = a.cb_threads or max(2, (ncpu // 3) // max(1, len(gpus) * a.gpu_slots))
    rf_jobs = a.rf_cpu_jobs or max(4, ncpu - len(gpus) * a.gpu_slots * threads - 2)
    try:
        q = subprocess.run(["nvidia-smi", "--query-gpu=index,memory.total", "--format=csv,noheader,nounits"],
                           capture_output=True, text=True, timeout=10).stdout
        gtot = {int(x.split(",")[0]): int(x.split(",")[1]) for x in q.strip().splitlines()}
    except Exception:
        gtot = {}
    results = run_scheduler(ctx, tasks, gpus, a.gpu_slots, a.cpu_workers, cache, infos, threads, rf_jobs, gtot,
                            os.path.join(out, "_tasks"), sampler)
    train_s = time.time() - T0 - load_s

    # ---- scores + metrics
    def cols_of(lab):
        return dict(np.load(os.path.join(cache, lab, "cols.npz")))

    per_fold, pooled_parts = {}, {}
    for f in folds:
        if f["test"] is None:
            continue
        fdir = os.path.join(out, f["name"])
        c = cols_of(f["test"])
        parts = {m: dict(np.load(os.path.join(fdir, "_parts", m + ".npz"))) for m in models}
        import pandas as pd
        sc = pd.DataFrame({"order": c["order"].astype(np.uint64)})
        if "entry_bin" in parts:
            for k in range(N_BIN):
                sc[f"p_bin{k}"] = parts["entry_bin"]["cal"][:, k]
        for col, m in [("p_tp1", "entry_tp1"), ("p_rug_cb", "rug_cb"), ("p_rug_rfgpu", "rug_rfgpu"),
                       ("p_rug_rfcpu", "rug_rfcpu"), ("p_hold", "hold"), ("p_dead", "hazard")]:
            if m in parts:
                sc[col] = parts[m]["cal"]
        # p_rug = mean of calibrated CatBoost and random forest (GPU RF; the CPU RF when xgboost is unavailable)
        rf_name = "rug_rfgpu" if "rug_rfgpu" in parts else "rug_rfcpu"
        rf_col = "p_" + rf_name
        if "p_rug_cb" in sc and rf_col in sc:
            sc["p_rug"] = 0.5 * (sc["p_rug_cb"] + sc[rf_col])
        order_cols = ["order"] + [f"p_bin{k}" for k in range(N_BIN)] + ["p_tp1", "p_rug_cb", "p_rug_rfgpu", "p_rug_rfcpu",
                                                                        "p_rug", "p_hold", "p_dead"]
        sc = sc[[x for x in order_cols if x in sc]]
        sp = os.path.join(fdir, f"scores_{f['test']}.csv.zst")
        sc.to_csv(sp, index=False, compression={"method": "zstd", "level": 3}, float_format="%.9g")
        shutil.rmtree(os.path.join(fdir, "_parts"))

        # model -> (pred used for metrics, raw pred, rows mask, target)
        sig, haz, allr = entry_rows(c), c["hazard"].astype(bool), np.ones(len(c["order"]), bool)
        ev = {}
        for m in models:
            algo, kind, target, rows, _, _ = MODELS[m]
            msk = {"signal": sig, "hazard": haz, "all": allr}[rows] & ~np.isnan(c[target])
            ev[m] = (parts[m]["cal"][msk], parts[m]["raw"][msk], c[target][msk].astype(int), c["y_logmax"][msk])
        if "p_rug" in sc:
            msk = ~np.isnan(c["y_rugdead"])
            ev["rug"] = (sc["p_rug"].to_numpy()[msk], 0.5 * (parts["rug_cb"]["raw"] + parts[rf_name]["raw"])[msk],
                         c["y_rugdead"][msk].astype(int), c["y_logmax"][msk])
        per_fold[f["test"]] = ev
        for k, v in ev.items():
            pooled_parts.setdefault(k, []).append(v)

    def model_metrics(m, cal, raw, y, logmax):
        if m == "entry_bin":
            p5 = cal[:, 5] + cal[:, 6]
            r = {"rows": int(len(y)), "class_counts": np.bincount(y, minlength=N_BIN).tolist(),
                 "mlogloss": multiclass_logloss(cal, y), "mlogloss_raw": multiclass_logloss(raw, y),
                 "bin_ge5": binary_metrics(p5, y >= 5), "bin_ge3": binary_metrics(cal[:, 3:].sum(1), y >= 3),
                 "top1pct_by_p_bin_ge5_reached_10x": top_precision(p5, np.nan_to_num(logmax, nan=-1) >= TEN_X)}
            return r
        r = binary_metrics(cal, y)
        rr = binary_metrics(raw, y)
        r["brier_raw"], r["log_loss_raw"] = rr.get("brier"), rr.get("log_loss")
        if m in ("entry_tp1", "baseline_lr"):
            r["top1pct_reached_10x"] = top_precision(cal, np.nan_to_num(logmax, nan=-1) >= TEN_X)
        return r

    metrics = {"per_fold": {}, "pooled": {}}
    for w, ev in per_fold.items():
        metrics["per_fold"][w] = {m: model_metrics(m, *v) for m, v in ev.items()}
    for m, vs in pooled_parts.items():
        metrics["pooled"][m] = model_metrics(m, *[np.concatenate([v[i] for v in vs]) for i in range(4)])

    def pr(scope, m, sub=None):
        x = scope.get(m, {})
        x = x.get(sub, {}) if sub else x
        return x.get("pr_auc")

    comp = {"entry_tp1_vs_logistic_pr_auc": {}, "rug_models": {}}
    for w, mm in list(metrics["per_fold"].items()) + [("pooled", metrics["pooled"])]:
        comp["entry_tp1_vs_logistic_pr_auc"][w] = {"catboost": pr(mm, "entry_tp1"), "logistic": pr(mm, "baseline_lr")}
        comp["rug_models"][w] = {k: {"pr_auc": pr(mm, k), "roc_auc": mm.get(k, {}).get("roc_auc"),
                                     "brier": mm.get(k, {}).get("brier")} for k in ("rug_cb", "rug_rfgpu", "rug_rfcpu", "rug")}

    importance = {}
    for m in models:
        if MODELS[m][0] != "cb":
            continue
        imps = [r["importance"] for r in results if r["model"] == m and r["importance"] is not None]
        if imps:
            mean = np.mean(imps, axis=0)
            importance[m] = [(feats[i], round(float(mean[i]), 3)) for i in np.argsort(-mean)[:30]]

    sampler.stop_ev.set()
    sampler.join(timeout=3)
    wall = time.time() - T0
    gsum = sampler.summary()
    step = max(1, len(sampler.samples) // 1500)
    report = {
        "generated": time.strftime("%Y-%m-%d %H:%M:%S"), "exploratory": True, "quick": a.quick,
        "config": {"min_train": a.min_train, "H_s": H_S, "gpus": gpus, "gpu_slots": a.gpu_slots,
                   "cpu_workers": a.cpu_workers, "catboost_thread_count": threads, "rf_cpu_n_jobs": rf_jobs, "models": models, "out": out},
        "windows": [{k: infos[w][k] for k in ("label", "path", "start", "end", "data_end", "rows", "signal_rows", "hazard_rows")}
                    for w in order],
        "features": feats, "folds": folds, "skipped_folds": skipped,
        "tasks": sorted([{k: v for k, v in r.items() if k != "importance"} for r in results], key=lambda r: (r["fold"], r["model"])),
        "metrics": metrics, "comparisons": comp, "importance_top30_pooled": importance,
        "timing_s": {"wall": round(wall, 1), "load": round(load_s, 1), "train_parallel": round(train_s, 1),
                     "sum_task_wall": round(sum(r["wall_s"] for r in results), 1)},
        "utilization": gsum, "utilization_samples": sampler.samples[::step], "cpu_samples": sampler.cpu[::step],
    }
    os.makedirs(a.reports, exist_ok=True)
    write_json(os.path.join(a.reports, "V5_MODELS.json"), report)
    with open(os.path.join(a.reports, "V5_MODELS.md"), "w") as fh:
        fh.write(render_md(report))
    log(f"wall {wall:.1f}s (load {load_s:.1f}s, parallel training {train_s:.1f}s, sum of task times "
        f"{report['timing_s']['sum_task_wall']}s); utilization {json.dumps(gsum)}")
    log(f"reports: {os.path.join(a.reports, 'V5_MODELS.md')}")


def _f(x, nd=4):
    return "-" if x is None else (f"{x:.{nd}f}" if isinstance(x, float) else str(x))


def render_md(r):
    L = ["# V5 models (Phase 4) — EXPLORATORY", "",
         f"Generated {r['generated']}. quick={r['quick']}. Walk-forward with purge H={r['config']['H_s']} s, "
         f"min_train={r['config']['min_train']}.", "", "## Windows", "",
         "| window | start | data_end | rows | signal rows | hazard rows |", "|---|---|---|---|---|---|"]
    for w in r["windows"]:
        L.append(f"| {w['label']} | {w['start']} | {w['data_end']} | {w['rows']} | {w['signal_rows']} | {w['hazard_rows']} |")
    L += ["", "## Folds", ""]
    for f in r["folds"]:
        L.append(f"- **{f['name']}**: train {f['train']}; excluded by purge {f['excluded_by_purge']}")
    for s in r["skipped_folds"]:
        L.append(f"- skipped test {s['test']}: qualifying train {s['qualifying_train']}, purged {s['excluded_by_purge']}")
    L += ["", "## Metrics (calibrated, test windows)", "",
          "| scope | model | rows | pos | PR-AUC | ROC-AUC | Brier | Brier raw | log loss |", "|---|---|---|---|---|---|---|---|---|"]
    scopes = list(r["metrics"]["per_fold"].items()) + [("POOLED", r["metrics"]["pooled"])]
    for w, mm in scopes:
        for m, x in mm.items():
            if m == "entry_bin":
                b = x["bin_ge5"]
                L.append(f"| {w} | entry_bin (P bin>=5) | {b['rows']} | {b['positives']} | {_f(b.get('pr_auc'))} | "
                         f"{_f(b.get('roc_auc'))} | {_f(b.get('brier'))} | - | mlogloss {_f(x['mlogloss'])} (raw {_f(x['mlogloss_raw'])}) |")
            else:
                L.append(f"| {w} | {m} | {x['rows']} | {x['positives']} | {_f(x.get('pr_auc'))} | {_f(x.get('roc_auc'))} | "
                         f"{_f(x.get('brier'))} | {_f(x.get('brier_raw'))} | {_f(x.get('log_loss'))} |")
    L += ["", "## Entry: top 1% by P(bin>=5) = p_bin5+p_bin6, reached >= 10x (signal rows)", "",
          "| scope | n top | precision | base rate | lift |", "|---|---|---|---|---|"]
    for w, mm in scopes:
        if "entry_bin" in mm:
            t = mm["entry_bin"]["top1pct_by_p_bin_ge5_reached_10x"]
            L.append(f"| {w} | {t.get('n')} | {_f(t.get('precision'))} | {_f(t.get('base_rate'))} | {_f(t.get('lift'), 2)} |")
    L += ["", "## CatBoost vs logistic baseline (PR-AUC on y_tp1)", "", "| scope | CatBoost | logistic |", "|---|---|---|"]
    for w, v in r["comparisons"]["entry_tp1_vs_logistic_pr_auc"].items():
        L.append(f"| {w} | {_f(v['catboost'])} | {_f(v['logistic'])} |")
    L += ["", "## Rug: CatBoost vs GPU RF vs CPU RF vs average (p_rug = mean of calibrated CatBoost and GPU RF)", "",
          "| scope | model | PR-AUC | ROC-AUC | Brier |", "|---|---|---|---|---|"]
    for w, v in r["comparisons"]["rug_models"].items():
        for k, x in v.items():
            L.append(f"| {w} | {k} | {_f(x['pr_auc'])} | {_f(x['roc_auc'])} | {_f(x['brier'])} |")
    L += ["", "## Reliability (pooled, calibrated, 10 equal-width bins)", ""]
    for m, x in r["metrics"]["pooled"].items():
        rel = x["bin_ge5"]["reliability"] if m == "entry_bin" else x.get("reliability")
        if not rel:
            continue
        L.append(f"**{m}{' (P bin>=5)' if m == 'entry_bin' else ''}**: " + "; ".join(
            f"{b['bin']}: {_f(b['mean_pred'], 3)}/{_f(b['obs_rate'], 3)}/{b['count']}" for b in rel if b["count"]))
        L.append("")
    L += ["## Top-30 feature importance (CatBoost, mean over all fold models)", ""]
    for m, imp in r["importance_top30_pooled"].items():
        L.append(f"**{m}**: " + ", ".join(f"{n} {v}" for n, v in imp))
        L.append("")
    tm, u = r["timing_s"], r["utilization"]
    L += ["## Runtime and utilization", "",
          f"Wall {tm['wall']} s (load {tm['load']} s, parallel training {tm['train_parallel']} s, sum of task wall times "
          f"{tm['sum_task_wall']} s). GPU slots {r['config']['gpu_slots']} per card on {r['config']['gpus']}.", "",
          "| device | util mean % | p50 | max | share >=50% | share >=90% | mem max MiB |", "|---|---|---|---|---|---|---|"]
    for k, v in u.items():
        if k.startswith("gpu"):
            L.append(f"| {k} | {v['util_mean']} | {v['util_p50']} | {v['util_max']} | {v['share_ge50']} | {v['share_ge90']} | {v['mem_max_mib']} |")
    if "cpu" in u:
        L.append(f"| cpu | {u['cpu']['util_mean']} | - | {u['cpu']['util_max']} | - | - | - |")
    L += ["", "Per task:", "", "| fold | model | pool | wall s | inner s | best iter | train rows | calibrated | inner split |",
          "|---|---|---|---|---|---|---|---|---|"]
    for t in r["tasks"]:
        L.append(f"| {t['fold']} | {t['model']} | {t['pool']} | {t['wall_s']} | {t['inner_s']} | {_f(t.get('best_iteration'))} | "
                 f"{t['train_rows']} | {t['calibrated']} | {t['inner_note']} |")
    return "\n".join(L) + "\n"


if __name__ == "__main__":
    main()
