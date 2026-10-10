"""End-to-end test of python/ml/v5_train.py on 5 synthetic window dirs (GPU, --quick).

  python3 -m pytest -q python/ml/tests/test_v5_train.py      (or: python3 python/ml/tests/test_v5_train.py)
  python3 python/ml/tests/test_v5_train.py --make DIR --windows 6 --rows 400000   (bigger synthetic set, benchmarks)
"""
import json
import os
import subprocess
import sys
import tempfile

import numpy as np
import pandas as pd

HERE = os.path.dirname(os.path.abspath(__file__))
ML = os.path.dirname(HERE)
sys.path.insert(0, ML)
from v5_common import H_S, ID_COLS, apply_iso_binary, rust_str_array  # noqa: E402

FEATURES = rust_str_array("FEATURES")
LABEL_COLS = rust_str_array("LABEL_COLS")
DAY = 86_400
BASE_T = 1_780_000_000
ORDER_BASE = (1 << 60) + 12345  # > 2^53: any float round trip would corrupt the keys


def make_window(d, start, seed, rows=6000, wlen=3600):
    """Fake snapshots_v5.csv.zst with the exact Rust column layout. y_bin never takes class 6."""
    rng = np.random.default_rng(seed)
    n_coins = max(10, rows // 24)
    coin = np.sort(rng.integers(0, n_coins, rows))
    time_ = rng.integers(0, wlen + 3 * 3600, rows)
    o = np.lexsort((coin, time_))  # file order = event (time) order
    coin, time_ = coin[o], time_[o]
    F = rng.normal(size=(rows, len(FEATURES))).astype(np.float32)
    F += rng.normal(size=(n_coins, len(FEATURES))).astype(np.float32)[coin] * 0.5  # per-coin effect
    F[rng.random(F.shape) < 0.05] = np.nan
    f = np.nan_to_num(F[:, :6])
    z = 1.2 * f[:, 0] + 0.8 * f[:, 1] - 0.5 * f[:, 2] + rng.normal(scale=0.8, size=rows)
    y_bin = np.digitize(z, [-1.2, -0.3, 0.6, 1.6, 2.6])  # 0..5, class 6 missing
    logmax = np.array([np.log(0.2), np.log(0.7), np.log(1.5), np.log(3), np.log(7), np.log(20)])[y_bin]
    logmax = logmax + rng.uniform(-0.05, 0.05, rows)
    sig = (rng.random(rows) < 0.3).astype(np.int8)
    lab = {
        "y_tp1": (z + rng.normal(scale=0.5, size=rows) > 0.8).astype(int),
        "y_bin": y_bin,
        "y_peak_barrier": np.exp(logmax).round(4),
        "y_hold": (f[:, 3] + rng.normal(size=rows) > 0.5).astype(int),
        "y_dead_10m": (f[:, 4] + rng.normal(size=rows) > 1.4).astype(int),
        "y_logmax": logmax.round(6),
        "y_end_h": rng.uniform(0, 10, rows).round(3),
        "y_rugdead": (-z + f[:, 5] + rng.normal(size=rows) > 1.5).astype(int),
        "y_t2x": np.where(y_bin >= 3, rng.integers(5, 5000, rows), -1),
    }
    order = np.uint64(ORDER_BASE) + np.uint64(seed) * np.uint64(10_000_000) + np.arange(rows, dtype=np.uint64) * np.uint64(3)
    df = pd.DataFrame({"coin": coin, "order": order, "time": time_, "is_signal": sig, "in_window": (time_ <= wlen).astype(np.int8)})
    df = pd.concat([df, pd.DataFrame(F, columns=FEATURES), pd.DataFrame(lab)[LABEL_COLS]], axis=1)
    os.makedirs(d, exist_ok=True)
    df.to_csv(os.path.join(d, "snapshots_v5.csv.zst"), index=False, na_rep="NaN", compression={"method": "zstd"})
    data_end = start + wlen + H_S + 120
    json.dump({"stats": {"rows": rows}, "window": [start, start + wlen], "data_end": data_end, "horizon_s": H_S},
              open(os.path.join(d, "snapshots_v5_stats.json"), "w"))
    return df


def make_set(root, n=5, rows=6000):
    """Windows 2 days apart; the last one starts 50000 s after the previous start (within H of its data_end)."""
    starts = [BASE_T + i * 2 * DAY for i in range(n - 1)]
    starts.append(starts[-1] + 50_000)
    dirs, dfs = [], {}
    for i, s in enumerate(starts):
        d = os.path.join(root, f"v5_t{i}")
        dfs[os.path.basename(d)] = make_window(d, s, seed=i + 1, rows=rows)
        dirs.append(d)
    return dirs, dfs


def _check(tmp, dirs, dfs):
    from catboost import CatBoostClassifier
    out, rep = os.path.join(tmp, "models"), os.path.join(tmp, "reports")
    r = json.load(open(os.path.join(rep, "V5_MODELS.json")))
    folds = {f["name"]: f for f in r["folds"]}
    # purge rule: t4 starts 50000 s after t3, so t3 (data_end = start + 3600 + H + 120) must be excluded from fold t4
    assert set(folds) == {"fold_t3", "fold_t4", "final"}, folds.keys()
    assert folds["fold_t4"]["train"] == ["t0", "t1", "t2"] and "t3" in folds["fold_t4"]["excluded_by_purge"]
    assert folds["fold_t3"]["train"] == ["t0", "t1", "t2"]
    assert folds["final"]["train"] == ["t0", "t1", "t2", "t3", "t4"]
    st = {w["label"]: w for w in r["windows"]}
    for f in r["folds"]:
        if f["test"]:
            for w in f["train"]:
                assert st[w]["data_end"] <= st[f["test"]]["start"] - H_S
    forbidden = set(LABEL_COLS) | set(ID_COLS)
    for fname in folds:
        fd = os.path.join(out, fname)
        for m in ["entry_bin", "entry_tp1", "rug_cb", "rug_rfgpu", "rug_rfcpu", "hold", "hazard", "baseline_lr"]:
            meta = json.load(open(os.path.join(fd, m + ".meta.json")))
            assert set(meta) == {"name", "kind", "n_classes", "features", "target", "train_windows", "calibrated"}, meta.keys()
            assert meta["features"] == FEATURES and not forbidden & set(meta["features"])  # no label/id column as feature
            assert os.path.exists(os.path.join(fd, m + ".iso.json")) == meta["calibrated"], (fname, m)
            ext = {"rug_rfgpu": ".rf.json", "rug_rfcpu": ".rf.joblib", "baseline_lr": ".lr.joblib"}.get(m, ".cbm.json")
            assert os.path.exists(os.path.join(fd, m + ext)), (fname, m, ext)
        assert json.load(open(os.path.join(fd, "rug_cb.meta.json")))["calibrated"]
        # 7-class entry model even though y_bin == 6 never occurs
        meta = json.load(open(os.path.join(fd, "entry_bin.meta.json")))
        assert meta["kind"] == "multiclass" and meta["n_classes"] == 7
        cb = CatBoostClassifier()
        cb.load_model(os.path.join(fd, "entry_bin.cbm.json"), format="json")
        P = cb.predict_proba(np.zeros((3, len(FEATURES)), np.float32))
        assert P.shape == (3, 7)
        if meta["calibrated"]:
            assert len(json.load(open(os.path.join(fd, "entry_bin.iso.json")))["classes"]) == 7
    for d in dirs:
        assert not (dfs[os.path.basename(d)]["y_bin"] == 6).any()
    # scores: one row per snapshot row, exact u64 keys, calibrated probabilities in [0, 1]
    for w in ["t3", "t4"]:
        sp = os.path.join(out, f"fold_{w}", f"scores_{w}.csv.zst")
        sc = pd.read_csv(sp, compression="zstd", dtype={"order": np.uint64})
        src = dfs[f"v5_{w}"]
        assert len(sc) == len(src)
        assert (sc["order"].to_numpy() == src["order"].to_numpy()).all()
        assert sc["order"].max() > 2 ** 53
        cols = [f"p_bin{k}" for k in range(7)] + ["p_tp1", "p_rug_cb", "p_rug_rfgpu", "p_rug_rfcpu", "p_rug", "p_hold", "p_dead"]
        assert list(sc.columns) == ["order"] + cols
        v = sc[cols].to_numpy()
        assert np.isfinite(v).all() and (v >= 0).all() and (v <= 1).all()
        assert np.allclose(sc[[f"p_bin{k}" for k in range(7)]].sum(1), 1.0, atol=1e-6)
        assert np.allclose(sc["p_rug"], 0.5 * (sc["p_rug_cb"] + sc["p_rug_rfgpu"]), atol=1e-8)
        # files alone reproduce the score: CatBoost JSON model + isotonic table (what Rust will do)
        fd = os.path.join(out, f"fold_{w}")
        cb = CatBoostClassifier()
        cb.load_model(os.path.join(fd, "entry_tp1.cbm.json"), format="json")
        p = cb.predict_proba(src[FEATURES].to_numpy(np.float32))[:, 1]
        if json.load(open(os.path.join(fd, "entry_tp1.meta.json")))["calibrated"]:
            p = apply_iso_binary(p, json.load(open(os.path.join(fd, "entry_tp1.iso.json"))))
        assert np.abs(p - sc["p_tp1"].to_numpy()).max() < 1e-6
    assert "gpu0" in r["utilization"] and "gpu1" in r["utilization"]
    assert {t["pool"] for t in r["tasks"]} >= {"gpu0", "gpu1", "cpu"}
    assert os.path.exists(os.path.join(rep, "V5_MODELS.md"))


def test_v5_train_end_to_end():
    with tempfile.TemporaryDirectory() as tmp:
        dirs, dfs = make_set(os.path.join(tmp, "data"))
        cmd = [sys.executable, os.path.join(ML, "v5_train.py"), *dirs, "--quick", "--min-train", "3", "--gpu-slots", "2",
               "--out", os.path.join(tmp, "models"), "--reports", os.path.join(tmp, "reports")]
        p = subprocess.run(cmd, capture_output=True, text=True)
        print(p.stdout[-6000:], p.stderr[-4000:])
        assert p.returncode == 0, p.stderr[-4000:]
        _check(tmp, dirs, dfs)


def test_purge_rule_unit():
    sys.path.insert(0, ML)
    from v5_train import plan_folds
    infos = {
        "a": {"start": 0, "data_end": 40_000}, "b": {"start": 200_000, "data_end": 240_000},
        "c": {"start": 400_000, "data_end": 440_000}, "d": {"start": 600_000, "data_end": 640_000},
        "e": {"start": 660_000, "data_end": 700_000},  # 660000 - H = 624000 < d.data_end -> d purged
    }
    order, folds, skipped = plan_folds(infos, 3)
    f = {x["name"]: x for x in folds}
    assert f["fold_d"]["train"] == ["a", "b", "c"] and f["fold_e"]["train"] == ["a", "b", "c"]
    assert f["fold_e"]["excluded_by_purge"] == ["d"] and f["final"]["train"] == list("abcde")
    _, folds, skipped = plan_folds(infos, 4)  # e has only 3 qualifying windows -> skipped
    assert [x["name"] for x in folds] == ["final"] and [s["test"] for s in skipped] == ["e"]


if __name__ == "__main__":
    if "--make" in sys.argv:
        i = sys.argv.index("--make")
        root = sys.argv[i + 1]
        n = int(sys.argv[sys.argv.index("--windows") + 1]) if "--windows" in sys.argv else 5
        rows = int(sys.argv[sys.argv.index("--rows") + 1]) if "--rows" in sys.argv else 6000
        dirs, _ = make_set(root, n, rows)
        print(" ".join(dirs))
    else:
        test_purge_rule_unit()
        test_v5_train_end_to_end()
        print("OK")
