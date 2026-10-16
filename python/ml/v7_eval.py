"""V7 model evaluation: every model family alone, then ONE combined system (EXPLORATORY until gates pass).

usage: LABEL_H=3600 python3 python/ml/v7_eval.py --train data/v5_A data/v5_B ... --test data/v5_X ... [--out reports/v7/MODELS.json]

Split: strictly chronological. --train windows must all be older than every --test window (checked on decision_start);
no leave-one-window-out, no tuning on test. Train rows: every labelled snapshot row of the train windows, weighted
1 / rows-per-coin, subsampled to --max-train. Test rows: decision rows (radar signal inside the decision window).
Labels: y_bin (peak before the -65 % barrier within 1 h) and y_rugdead, present only where the 1 h horizon is covered.

Families, each scored alone with identical splits:
  prior            train base rate (reference)
  qfv_radar        user QFV formula P_collapse = sin^2(pi / (1 + exp(sigma2 / v_ratio))), no training; bands < 0.5 / <= 0.75 / > 0.75
  markov_state     empirical P(target | discrete state), state = radar state x flow sign x 60 s return tercile x on_amm
  hmm_filter       4-state Gaussian HMM fitted on train sequences; FORWARD-only filtered posterior per coin + logistic
  kalman_trend     local-linear-trend Kalman filter on log price per coin (level, slope, slope z, innovation z) + logistic
  logistic         standardized logistic regression on all features
  pca20_logistic   PCA(20) fitted on train + logistic (linear algebra)
  quantum_kernel   simulated ZZ-feature-map fidelity kernel (6 PCA dims, 64-amplitude statevector) + kernel logistic
  random_forest, lightgbm, catboost   on all features
  combined         ONE system: CatBoost on all features + markov rate + HMM posterior + Kalman state + PCA components
Metrics per target: AUC, PR-AUC, Brier, precision@top 1 % / 5 %, and coin level: share of >= 10x coins whose first
decision row is in the top 5 % of first-row scores. Bootstrap 90 % CI by coin on PR-AUC.
"""
import argparse
import json
import os
import sys
import time

import numpy as np
import pandas as pd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from model_families import FEATURES  # noqa: E402
from sklearn.decomposition import PCA  # noqa: E402
from sklearn.impute import SimpleImputer  # noqa: E402
from sklearn.linear_model import LogisticRegression  # noqa: E402
from sklearn.metrics import average_precision_score, brier_score_loss, roc_auc_score  # noqa: E402
from sklearn.pipeline import make_pipeline  # noqa: E402
from sklearn.preprocessing import StandardScaler  # noqa: E402

TARGETS = {"y_2x": lambda d: d["y_bin"] >= 3, "y_5x": lambda d: d["y_bin"] >= 4, "y_10x": lambda d: d["y_bin"] >= 5,
           "y_rugdead": lambda d: d["y_rugdead"] == 1}
QF = ["qfv_sigma2", "qfv_vratio", "qfv_x", "qfv_p"]
HMM_OBS = ["ret_15s", "flow_imb_60s", "buyers_60s", "accel", "vol_60s"]
SEED = 0


def load(d):
    df = pd.read_csv(os.path.join(d, "snapshots_v5.csv.zst"), compression="zstd")
    df = df.sort_values(["coin", "order"], kind="stable").reset_index(drop=True)
    df = df.copy()
    df["labelled"] = df["y_bin"].notna() & df["y_rugdead"].notna()
    dw = json.load(open(os.path.join(d, "decision_window.json")))
    return df, dw


# ---------- causal per-coin sequence components ----------

def coin_bounds(coin):
    starts = np.r_[0, np.flatnonzero(coin[1:] != coin[:-1]) + 1]
    return starts, np.r_[starts[1:], len(coin)]


def kalman_features(df, q_level, q_slope, r):
    """Local linear trend on y = log(mult_since_first); state (level, slope) per coin, filtered causally."""
    y = np.log(np.clip(df["mult_since_first"].to_numpy(np.float64), 1e-9, None))
    t = df["time"].to_numpy(np.float64)
    out = np.full((len(df), 4), np.nan)
    for s, e in zip(*coin_bounds(df["coin"].to_numpy())):
        x = np.array([y[s], 0.0])
        P = np.diag([r, 1e-4])
        for i in range(s, e):
            dt = max(t[i] - t[i - 1], 1.0) if i > s else 1.0
            F = np.array([[1.0, dt], [0.0, 1.0]])
            x = F @ x
            P = F @ P @ F.T + np.diag([q_level * dt, q_slope * dt])
            if not np.isfinite(y[i]):
                out[i] = [x[0], x[1], x[1] / np.sqrt(P[1, 1]), np.nan]
                continue
            S = P[0, 0] + r
            innov = y[i] - x[0]
            K = P[:, 0] / S
            x = x + K * innov
            P = P - np.outer(K, P[0, :])
            out[i] = [x[0], x[1], x[1] / np.sqrt(max(P[1, 1], 1e-12)), innov / np.sqrt(S)]
    return out


def hmm_obs(df, mu=None, sd=None):
    o = np.array(df[HMM_OBS].to_numpy(np.float64), copy=True)
    o[:, 2] = np.log1p(np.clip(o[:, 2], 0, None))
    o[~np.isfinite(o)] = np.nan
    if mu is None:
        mu, sd = np.nanmedian(o, 0), np.nanstd(o, 0) + 1e-9
    o = np.clip(np.nan_to_num((o - mu) / sd, nan=0.0), -6, 6)
    return o, mu, sd


def hmm_filter(df, model, mu, sd):
    """Forward-only filtered state posterior P(s_t | o_1..t) per coin (no smoothing = no look-ahead)."""
    o, _, _ = hmm_obs(df, mu, sd)
    logb = model._compute_log_likelihood(o)
    A, pi = model.transmat_, model.startprob_
    out = np.zeros((len(df), A.shape[0]))
    for s, e in zip(*coin_bounds(df["coin"].to_numpy())):
        a = pi * np.exp(logb[s] - logb[s].max())
        a /= a.sum()
        out[s] = a
        for i in range(s + 1, e):
            a = (a @ A) * np.exp(logb[i] - logb[i].max())
            a /= a.sum() + 1e-300
            out[i] = a
    return out


def markov_state(df, cuts):
    st = df["rd_state"].fillna(-1).to_numpy(int)
    flow = np.sign(df["net_flow_60s"].fillna(0).to_numpy())
    r = np.digitize(df["ret_60s"].fillna(0).to_numpy(), cuts)
    amm = df["on_amm"].fillna(0).to_numpy(int)
    return ((st + 1) * 1000 + (flow + 1) * 100 + r * 10 + amm).astype(np.int64)


# ---------- QFV radar (user spec quantum_qfv_formula.pdf) ----------

def qfv_features(d, df):
    """QFV per snapshot row, causal (trades at or before the row's own trade, block-time resolution 1 s):
    sigma2 = variance of inter-trade intervals (s) over the last 5 min; v_ratio = SOL volume 5 min / 1 h;
    theta = pi / (1 + exp(sigma2 / v_ratio)); P_collapse = sin(theta)^2. Note: P is monotone decreasing in
    x = sigma2 / v_ratio (P > 0.75 <=> x < ln 2), so it is a one-statistic rule; it is not a calibrated probability."""
    tr = pd.read_csv(os.path.join(d, "trades.csv"), usecols=["block_time", "order", "mint", "sol_lamports"])
    tr = tr.sort_values("order", kind="stable").reset_index(drop=True)
    row_of = pd.Series(np.arange(len(tr)), index=tr["order"].to_numpy())
    ti = row_of.reindex(df["order"].to_numpy()).to_numpy()
    out = np.full((len(df), 4), np.nan)
    ok = np.isfinite(ti)
    tr["k"] = tr.groupby("mint", sort=False).cumcount()
    mint_rows = {m: g.index.to_numpy() for m, g in tr.groupby("mint", sort=False)}
    bt = tr["block_time"].to_numpy(np.float64)
    sol = tr["sol_lamports"].to_numpy(np.float64) / 1e9
    snap_rows = pd.Series(np.flatnonzero(ok)).groupby(tr["mint"].to_numpy()[ti[ok].astype(int)]).apply(lambda s: s.to_numpy())
    for m, rows in snap_rows.items():
        ix = mint_rows[m]
        t, v = bt[ix], sol[ix]
        cv = np.r_[0, np.cumsum(v)]
        dlt = np.r_[0.0, np.diff(t)]
        c1, c2 = np.r_[0, np.cumsum(dlt)], np.r_[0, np.cumsum(dlt * dlt)]
        k = tr["k"].to_numpy()[ti[rows].astype(int)]  # position of the row's trade in this mint's sequence
        now = t[k]
        a5 = np.searchsorted(t, now - 300, side="right")
        a60 = np.searchsorted(t, now - 3600, side="right")
        e = k + 1
        n_int = e - (a5 + 1)  # intervals between consecutive trades inside the 5 min window
        s1, s2 = c1[e] - c1[a5 + 1], c2[e] - c2[a5 + 1]
        with np.errstate(invalid="ignore", divide="ignore"):
            var = np.where(n_int >= 2, s2 / np.maximum(n_int, 1) - (s1 / np.maximum(n_int, 1)) ** 2, np.nan)
            vr = (cv[e] - cv[a5]) / (cv[e] - cv[a60])
            x = var / vr
            theta = np.pi / (1 + np.exp(np.clip(x, 0, 700)))
        out[rows] = np.c_[var, vr, x, np.sin(theta) ** 2]
    return out


# ---------- simulated quantum kernel ----------

def zz_states(Z):
    """ZZ feature map (2 repetitions) on n = Z.shape[1] qubits, exact statevector simulation: H^n, then
    diag phase exp(i(sum_j x_j Z_j + sum_{j<k} (pi - x_j)(pi - x_k) Z_j Z_k)) per repetition."""
    n = Z.shape[1]
    dim = 2 ** n
    bits = ((np.arange(dim)[:, None] >> np.arange(n)) & 1).astype(np.float64)
    zsign = 1.0 - 2.0 * bits  # Z eigenvalues per basis state
    h = np.full(dim, 1 / np.sqrt(dim), dtype=np.complex128)
    Hn = None
    out = np.empty((len(Z), dim), dtype=np.complex128)
    for i, x in enumerate(Z):
        phase = zsign @ x
        for j in range(n):
            for k in range(j + 1, n):
                phase = phase + (np.pi - x[j]) * (np.pi - x[k]) * zsign[:, j] * zsign[:, k]
        psi = h * np.exp(1j * phase)
        if Hn is None:
            Hn = hadamard(n)
        psi = Hn @ psi
        psi = psi * np.exp(1j * phase)
        out[i] = psi
    return out


def hadamard(n):
    H = np.array([[1.0]])
    for _ in range(n):
        H = np.block([[H, H], [H, -H]]) / np.sqrt(2)
    return H


# ---------- metrics ----------

def metrics(y, p, coin, first, y10_coin=None):
    n_pos = int(y.sum())
    out = {"rows": int(len(y)), "positives": n_pos, "positive_coins": int(len(np.unique(coin[y == 1]))),
           "base_rate": float(y.mean()) if len(y) else float("nan")}
    if n_pos == 0 or n_pos == len(y):
        return out
    out["auc"] = float(roc_auc_score(y, p))
    out["pr_auc"] = float(average_precision_score(y, p))
    out["brier"] = float(brier_score_loss(y, np.clip(p, 0, 1)))
    order = np.argsort(-p, kind="stable")
    for frac in (0.01, 0.05):
        k = max(1, int(round(frac * len(y))))
        out[f"precision_top{int(frac * 100)}pct"] = float(y[order[:k]].mean())
    # Coin level: first decision row of each coin (the earliest actionable moment).
    fy, fp = y10_coin if y10_coin is not None else y, p
    cf = first
    yc, pc = fy[cf], fp[cf]
    if yc.sum() > 0:
        k = max(1, int(round(0.05 * len(pc))))
        top = np.argsort(-pc, kind="stable")[:k]
        out["coin_first_row_recall_top5pct"] = float(yc[top].sum() / yc.sum())
        out["coin_first_row_positives"] = int(yc.sum())
    # Bootstrap CI on PR-AUC, resampling coins.
    rng = np.random.default_rng(SEED)
    uc = np.unique(coin)
    idx_by = pd.Series(np.arange(len(coin))).groupby(coin).apply(lambda s: s.to_numpy())
    vals = []
    for _ in range(200):
        pick = rng.choice(uc, len(uc))
        ix = np.concatenate([idx_by[c] for c in pick])
        if 0 < y[ix].sum() < len(ix):
            vals.append(average_precision_score(y[ix], p[ix]))
    if vals:
        out["pr_auc_ci90"] = [float(np.percentile(vals, 5)), float(np.percentile(vals, 95))]
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--train", nargs="+", required=True)
    ap.add_argument("--test", nargs="+", required=True)
    ap.add_argument("--out", default="reports/v7/MODELS.json")
    ap.add_argument("--max-train", type=int, default=300_000)
    ap.add_argument("--families", default="prior,qfv_radar,markov_state,hmm_filter,kalman_trend,logistic,pca20_logistic,"
                                          "quantum_kernel,random_forest,lightgbm,catboost,combined")
    a = ap.parse_args()
    fams = a.families.split(",")
    t0 = time.time()
    tr = {d: load(d) for d in a.train}
    te = {d: load(d) for d in a.test}
    last_train = max(dw["decision_end"] for _, dw in tr.values())
    first_test = min(dw["decision_start"] for _, dw in te.values())
    assert last_train < first_test, "train windows must be older than every test window"
    print(f"loaded in {time.time() - t0:.0f}s; train rows {sum(len(x) for x, _ in tr.values())}, test rows {sum(len(x) for x, _ in te.values())}", flush=True)

    # Fit sequence components on train only.
    trall = pd.concat([x for x, _ in tr.values()], ignore_index=True)
    dy = np.diff(np.log(np.clip(trall["mult_since_first"].to_numpy(np.float64), 1e-9, None)))
    dy = dy[np.isfinite(dy) & (trall["coin"].to_numpy()[1:] == trall["coin"].to_numpy()[:-1])]
    mad = float(np.median(np.abs(dy - np.median(dy)))) + 1e-6
    q_level, q_slope, r = (0.5 * mad) ** 2, (0.05 * mad) ** 2, (1.0 * mad) ** 2
    cuts = np.nanpercentile(trall["ret_60s"].to_numpy(np.float64), [33, 67])
    from hmmlearn.hmm import GaussianHMM
    o, mu, sd = hmm_obs(trall)
    st, en = coin_bounds(trall["coin"].to_numpy())
    rng = np.random.default_rng(SEED)
    pick = np.sort(rng.choice(len(st), min(len(st), 4000), replace=False))
    seq = np.concatenate([o[st[i]:en[i]] for i in pick])
    lengths = [en[i] - st[i] for i in pick]
    hmm = GaussianHMM(n_components=4, covariance_type="diag", n_iter=30, random_state=SEED).fit(seq, lengths)
    print(f"components fitted {time.time() - t0:.0f}s", flush=True)

    def extend(df, d):
        q = qfv_features(d, df)
        k = kalman_features(df, q_level, q_slope, r)
        h = hmm_filter(df, hmm, mu, sd)
        ext = pd.DataFrame(np.c_[k, h], columns=["kf_level", "kf_slope", "kf_slope_z", "kf_innov_z"] + [f"hmm_p{i}" for i in range(4)])
        ext["mk_state"] = markov_state(df, cuts)
        for j, c in enumerate(QF):
            ext[c] = q[:, j]
        return pd.concat([df, ext], axis=1)

    trx = [extend(x, d) for d, (x, _) in tr.items()]
    tex = [extend(x, d) for d, (x, _) in te.items()]
    print(f"sequence features {time.time() - t0:.0f}s", flush=True)
    trd = pd.concat(trx, ignore_index=True)
    trd = trd[trd["labelled"]]
    trd["w"] = 1.0 / trd.groupby("coin")["coin"].transform("size")
    if len(trd) > a.max_train:
        trd = trd.sample(a.max_train, weights="w", random_state=SEED)
    ted = pd.concat([x.assign(win=i) for i, x in enumerate(tex)], ignore_index=True)
    ted = ted[(ted["is_signal"] == 1) & (ted["in_window"] == 1) & ted["labelled"]].reset_index(drop=True)
    tcoin = (ted["win"].astype(str) + ":" + ted["coin"].astype(str)).to_numpy()
    first = ~pd.Series(tcoin).duplicated().to_numpy()
    KF = ["kf_level", "kf_slope", "kf_slope_z", "kf_innov_z"]
    HM = [f"hmm_p{i}" for i in range(4)]

    def X(df, cols):
        m = np.array(df[cols].to_numpy(np.float64), copy=True)  # pandas 3 may return a read-only view
        m[~np.isfinite(m)] = np.nan
        return m

    Xtr, Xte = X(trd, FEATURES), X(ted, FEATURES)
    imp = SimpleImputer(strategy="median", keep_empty_features=True).fit(Xtr)
    sc = StandardScaler().fit(imp.transform(Xtr))
    pca = PCA(n_components=20, random_state=SEED).fit(sc.transform(imp.transform(Xtr)))
    Ptr, Pte = pca.transform(sc.transform(imp.transform(Xtr))), pca.transform(sc.transform(imp.transform(Xte)))
    res = {"label": "EXPLORATORY v7 per-family + combined evaluation, chronological split", "train": a.train, "test": a.test,
           "train_rows": int(len(trd)), "test_decision_rows": int(len(ted)), "test_coins": int(len(np.unique(tcoin))),
           "features": len(FEATURES), "kalman": {"q_level": q_level, "q_slope": q_slope, "r": r},
           "hmm_means": hmm.means_.tolist(), "results": {}}
    w = trd["w"].to_numpy()
    for tname, f in TARGETS.items():
        ytr, yte = f(trd).to_numpy(int), f(ted).to_numpy(int)
        y10 = TARGETS["y_10x"](ted).to_numpy(int)
        y10_coin = pd.Series(y10).groupby(tcoin).transform("max").to_numpy()
        rr = {}
        # Markov rate (Laplace-smoothed) is also a feature of the combined system.
        mk = pd.DataFrame({"s": trd["mk_state"].to_numpy(), "y": ytr, "w": w})
        g = mk.groupby("s").apply(lambda z: (np.sum(z.w * z.y) + ytr.mean()) / (np.sum(z.w) + 1.0), include_groups=False)
        mk_tr = trd["mk_state"].map(g).fillna(ytr.mean()).to_numpy()
        mk_te = ted["mk_state"].map(g).fillna(ytr.mean()).to_numpy()
        for fam in fams:
            t1 = time.time()
            try:
                if fam == "prior":
                    p = np.full(len(ted), ytr.mean())
                elif fam == "qfv_radar":
                    p = np.nan_to_num(ted["qfv_p"].to_numpy(np.float64), nan=0.0)
                elif fam == "markov_state":
                    p = mk_te
                elif fam in ("hmm_filter", "kalman_trend", "logistic"):
                    cols = HM if fam == "hmm_filter" else KF if fam == "kalman_trend" else FEATURES
                    m = make_pipeline(SimpleImputer(strategy="median", keep_empty_features=True), StandardScaler(), LogisticRegression(C=0.5, max_iter=3000))
                    m.fit(X(trd, cols), ytr, logisticregression__sample_weight=w)
                    p = m.predict_proba(X(ted, cols))[:, 1]
                elif fam == "pca20_logistic":
                    m = LogisticRegression(C=0.5, max_iter=3000).fit(Ptr, ytr, sample_weight=w)
                    p = m.predict_proba(Pte)[:, 1]
                elif fam == "quantum_kernel":
                    # 6 PCA dims scaled to [0, pi]; fidelity kernel against 600 train landmarks (balanced), then logistic.
                    lo, hi = np.percentile(Ptr[:, :6], [1, 99], axis=0)
                    sq = lambda P: np.clip((P[:, :6] - lo) / (hi - lo + 1e-9), 0, 1) * np.pi
                    rs = np.random.default_rng(SEED)
                    pos, neg = np.flatnonzero(ytr == 1), np.flatnonzero(ytr == 0)
                    land = np.r_[rs.choice(pos, min(300, len(pos)), replace=False), rs.choice(neg, 300, replace=False)]
                    fit_ix = rs.choice(len(ytr), min(20000, len(ytr)), replace=False)
                    SL = zz_states(sq(Ptr[land]))
                    Kf = np.abs(zz_states(sq(Ptr[fit_ix])) @ SL.conj().T) ** 2
                    Kt = np.abs(zz_states(sq(Pte)) @ SL.conj().T) ** 2
                    m = LogisticRegression(C=1.0, max_iter=3000).fit(Kf, ytr[fit_ix], sample_weight=w[fit_ix])
                    p = m.predict_proba(Kt)[:, 1]
                elif fam == "random_forest":
                    from sklearn.ensemble import RandomForestClassifier
                    m = RandomForestClassifier(n_estimators=400, min_samples_leaf=20, max_features="sqrt", n_jobs=-1, random_state=SEED)
                    m.fit(imp.transform(Xtr), ytr, sample_weight=w)
                    p = m.predict_proba(imp.transform(Xte))[:, 1]
                elif fam == "lightgbm":
                    import lightgbm as lgb
                    m = lgb.LGBMClassifier(n_estimators=600, learning_rate=0.03, num_leaves=31, min_child_samples=50, subsample=0.8,
                                           subsample_freq=1, colsample_bytree=0.7, verbose=-1, random_state=SEED)
                    m.fit(Xtr, ytr, sample_weight=w)
                    p = m.predict_proba(Xte)[:, 1]
                elif fam in ("catboost", "combined", "combined_no_qfv"):
                    from catboost import CatBoostClassifier
                    if fam == "catboost":
                        A_tr, A_te = Xtr, Xte
                    elif fam == "combined_no_qfv":
                        A_tr = np.c_[Xtr, mk_tr, X(trd, HM), X(trd, KF), Ptr]
                        A_te = np.c_[Xte, mk_te, X(ted, HM), X(ted, KF), Pte]
                    else:
                        A_tr = np.c_[Xtr, mk_tr, X(trd, HM), X(trd, KF), Ptr, X(trd, QF)]
                        A_te = np.c_[Xte, mk_te, X(ted, HM), X(ted, KF), Pte, X(ted, QF)]
                    m = CatBoostClassifier(iterations=800, depth=6, learning_rate=0.05, verbose=False, random_seed=SEED, task_type="GPU", devices="0")
                    m.fit(A_tr, ytr, sample_weight=w)
                    p = m.predict_proba(A_te)[:, 1]
                else:
                    continue
                rr[fam] = metrics(yte, p, tcoin, first, y10_coin if tname == "y_10x" else None)
                rr[fam]["seconds"] = round(time.time() - t1, 1)
                print(tname, fam, {k: (round(v, 3) if isinstance(v, float) else v) for k, v in rr[fam].items() if k in ("auc", "pr_auc", "precision_top1pct", "coin_first_row_recall_top5pct", "positive_coins")}, flush=True)
            except Exception as ex:  # one family failing must not hide the others
                rr[fam] = {"error": repr(ex)}
                print(tname, fam, "ERROR", ex, flush=True)
        pq = ted["qfv_p"].to_numpy(np.float64)
        bands = {"P<0.50": pq < 0.5, "0.50<=P<=0.75": (pq >= 0.5) & (pq <= 0.75), "P>0.75": pq > 0.75, "P=NaN (<3 trades in 5 min)": ~np.isfinite(pq)}
        rr["qfv_bands"] = {b: {"rows": int(m.sum()), "positives": int(yte[m].sum()), "rate": float(yte[m].mean()) if m.sum() else None,
                               "positive_coins": int(len(np.unique(tcoin[m & (yte == 1)])))} for b, m in bands.items()}
        res["results"][tname] = rr
    os.makedirs(os.path.dirname(a.out), exist_ok=True)
    json.dump(res, open(a.out, "w"), indent=1)
    print(f"done {time.time() - t0:.0f}s -> {a.out}")


if __name__ == "__main__":
    main()
