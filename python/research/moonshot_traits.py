"""What do coins look like in the 5 minutes BEFORE they take off, by final outcome and venue? (descriptive research)

usage: python3 python/research/moonshot_traits.py data/v5_X [...] [--sol-usd 118.59] [--out reports/v6/MOONSHOT_TRAITS.json]

Per coin (trades.csv, every venue, price from the coin's active pool = same rule as the Rust engine: the same pool
always updates, another pool takes over from the curve or when at least as deep):
  takeoff t0  = first time inside the decision window, after the coin traded --min-age s with >= --min-pre-trades
                trades in the previous 5 min, that the price is >= 1.10 x its 5-minute low (the base);
  outcome     = max price within 1 h after t0 / base: moonshot >= 10x, runner 2-10x, fizzle < 1.5x (rose 10 % then died);
  traits      = computed only from trades in [t0 - 300 s, t0) of that coin (causal: known before the takeoff).
Report: median trait per outcome class, univariate ROC-AUC moonshot vs fizzle (0.5 = no signal, > 0.5 = higher in
moonshots), counts per venue. Univariate AUC is a screening statistic, not a model: traits overlap, and ML combines them.
"""
import argparse
import json
import os

import numpy as np
import pandas as pd
from sklearn.metrics import roc_auc_score

NO_POOL = 4294967295
DUST = 0.01e9


def active_paths(df):
    """Yield (mint, times, prices, liq_sol, venues) with the engine's active-pool rule."""
    for mint, g in df.groupby("mint", sort=False):
        t = g["block_time"].to_numpy()
        q = g["q"].to_numpy()
        tk = g["tk"].to_numpy()
        pool = g["pool"].to_numpy()
        venue = g["venue"].to_numpy()
        known, a_pool, a_venue, a_q = False, None, None, 0.0
        keep = np.zeros(len(g), bool)
        for i in range(len(g)):
            amm = venue[i] != "pumpfun_curve"
            same = (pool[i] == a_pool) if (pool[i] != NO_POOL and a_pool not in (None, NO_POOL)) else (venue[i] == a_venue)
            cur_amm = known and a_venue != "pumpfun_curve"
            if (not known) or same or (not cur_amm and amm) or (amm and cur_amm and q[i] >= a_q):
                known, a_pool, a_venue, a_q = True, pool[i], venue[i], q[i]
                keep[i] = tk[i] > 0
        yield mint, g, keep, t[keep], (q[keep] / tk[keep]), q[keep] / 1e9


def traits(w, liq0, sol_usd, first_seen):
    """Pre-takeoff traits from the trades w (all venues) of one coin in [t0-300, t0)."""
    b, s = w[w["side"] == "buy"], w[w["side"] == "sell"]
    bs, ss = b["sol_lamports"].to_numpy(np.float64), s["sol_lamports"].to_numpy(np.float64)
    t_end = w["block_time"].max() if len(w) else 0
    b60 = b[b["block_time"] >= t_end - 60]
    b60p = b[(b["block_time"] < t_end - 60) & (b["block_time"] >= t_end - 120)]
    buyers = b["trader"].nunique()
    real = bs >= DUST
    top = (b.groupby("trader")["sol_lamports"].sum().max() / max(bs.sum(), 1)) if len(b) else np.nan
    new_w = np.mean([first_seen.get(x, 0) >= t_end - 300 for x in b["trader"].unique()]) if len(b) else np.nan
    return {
        "trades_5m": len(w), "buyers_5m": buyers, "sellers_5m": s["trader"].nunique(),
        "buy_sol_5m": bs.sum() / 1e9, "sell_sol_5m": ss.sum() / 1e9,
        "net_flow_over_liq": (bs.sum() - ss.sum()) / 1e9 / max(liq0, 1e-9),
        "buy_sell_count_ratio": len(b) / max(len(s), 1),
        "median_buy_usd": float(np.median(bs[real]) / 1e9 * sol_usd) if real.any() else 0.0,
        "max_buy_usd": float(bs.max() / 1e9 * sol_usd) if len(bs) else 0.0,
        "dust_share_of_buys": float(np.mean(~real)) if len(bs) else np.nan,
        "top_buyer_share": float(top),
        "new_wallet_share": float(new_w),
        "buyer_accel_60s": (b60["trader"].nunique() - b60p["trader"].nunique()),
        "liquidity_usd": liq0 * sol_usd,
        "arb_share": float((w["trader"] == "arb_roundtrip").mean()) if len(w) else 0.0,
        "venues_5m": w["venue"].nunique(),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--sol-usd", type=float, default=118.59)
    ap.add_argument("--min-age", type=int, default=120, help="takeoff counted only after the coin traded this long (s)")
    ap.add_argument("--min-pre-trades", type=int, default=10, help="trades of the coin in the 5 min before takeoff")
    ap.add_argument("--out", default="reports/v6/MOONSHOT_TRAITS.json")
    a = ap.parse_args()
    rows = []
    for d in a.dirs:
        dw = json.load(open(os.path.join(d, "decision_window.json")))
        w0, w1 = dw["decision_start"], dw["decision_end"]
        df = pd.read_csv(os.path.join(d, "trades.csv"), usecols=["block_time", "order", "mint", "venue", "side", "sol_lamports", "trader",
                                                                   "quote_reserve_after", "token_reserve_after", "pool"], dtype={"token_reserve_after": str})
        df["q"] = df["quote_reserve_after"].astype(np.float64)
        df["tk"] = df["token_reserve_after"].astype(np.float64)
        df = df.sort_values("order", kind="stable")
        first_seen = df.groupby("trader")["block_time"].min().to_dict()
        for mint, g, keep, t, p, liq in active_paths(df):
            if len(t) < 20:
                continue
            # base = lowest price of the previous 5 minutes (a takeoff from a recent base, not from the launch price)
            run_min = pd.Series(p, index=pd.to_datetime(t, unit="s")).rolling("300s").min().to_numpy()
            hit = np.nonzero((p >= 1.10 * run_min) & (t >= w0) & (t <= w1) & (t - t[0] >= a.min_age))[0]
            if len(hit) == 0:
                continue
            i0 = hit[0]
            t0, base = t[i0], run_min[i0]
            after = (t > t0) & (t <= t0 + 3600)
            mx = p[after].max() / base if after.any() else p[i0] / base
            cls = "moonshot_10x" if mx >= 10 else "runner_2_10x" if mx >= 2 else "fizzle_lt_1.5x" if mx < 1.5 else "mid_1.5_2x"
            pre = g[(g["block_time"] >= t0 - 300) & (g["block_time"] < t0)]
            if len(pre) < a.min_pre_trades:
                continue
            r = {"window": os.path.basename(d.rstrip("/")), "mint": mint, "class": cls, "max_mult": float(mx),
                 "venue_at_t0": g["venue"].to_numpy()[keep][i0], "age_at_t0_s": int(t0 - t[0])}
            r.update(traits(pre, liq[i0 - 1] if i0 > 0 else liq[i0], a.sol_usd, first_seen))
            rows.append(r)
    R = pd.DataFrame(rows)
    feats = [c for c in R.columns if c not in ("window", "mint", "class", "max_mult", "venue_at_t0")]
    rep = {"label": "EXPLORATORY pre-takeoff traits (5 min before +10 %), descriptive", "coins": int(len(R)),
           "class_counts": R["class"].value_counts().to_dict(),
           "class_by_venue": R.groupby(["venue_at_t0", "class"]).size().unstack(fill_value=0).to_dict(orient="index"),
           "median_by_class": R.groupby("class")[feats].median().to_dict(orient="index"), "auc_moonshot_vs_fizzle": {}}
    m = R[R["class"].isin(["moonshot_10x", "fizzle_lt_1.5x"])]
    y = (m["class"] == "moonshot_10x").to_numpy(int)
    for f in feats:
        x = m[f].to_numpy(np.float64)
        ok = np.isfinite(x)
        if ok.sum() > 20 and 0 < y[ok].sum() < ok.sum():
            rep["auc_moonshot_vs_fizzle"][f] = float(roc_auc_score(y[ok], x[ok]))
    rep["auc_moonshot_vs_fizzle"] = dict(sorted(rep["auc_moonshot_vs_fizzle"].items(), key=lambda kv: -abs(kv[1] - 0.5)))
    os.makedirs(os.path.dirname(a.out), exist_ok=True)
    json.dump(rep, open(a.out, "w"), indent=1, default=float)
    R.to_csv(a.out.replace(".json", "_coins.csv.gz"), index=False)
    print("classes", rep["class_counts"])
    print("venues x class", json.dumps(rep["class_by_venue"], indent=0))
    print("AUC moonshot vs fizzle (n moonshot =", int(y.sum()), ", fizzle =", int(len(y) - y.sum()), ")")
    med = R.groupby("class")[feats].median()
    for f, v in rep["auc_moonshot_vs_fizzle"].items():
        print(f"  {f:22} AUC {v:.3f}   median moonshot {med.loc['moonshot_10x', f] if 'moonshot_10x' in med.index else float('nan'):10.3f}"
              f"   runner {med.loc['runner_2_10x', f] if 'runner_2_10x' in med.index else float('nan'):10.3f}   fizzle {med.loc['fizzle_lt_1.5x', f]:10.3f}")


if __name__ == "__main__":
    main()
