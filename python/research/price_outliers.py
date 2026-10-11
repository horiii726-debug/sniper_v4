"""Decoder quality C3d: share of trades whose execution price is far from the same pool's recent prices.

usage: python3 python/research/price_outliers.py data/v5_X [--out reports/v6/PRICE_OUTLIERS_<X>.json]

Per pool (trades.csv `pool`), trades >= 0.01 SOL in order: outlier if the execution price (SOL / token) differs from
the median of the previous 20 trades of that pool by more than 2x either way. Real moves of 2x within 20 trades of
one pool exist (rugs, launches), so the rate is not 0 for a correct decoder; the test compares decoder versions on
the same window and lists the worst pools.
"""
import argparse
import json
import os

import numpy as np
import pandas as pd

NO_POOL = 4294967295


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--out")
    a = ap.parse_args()
    df = pd.read_csv(os.path.join(a.dir, "trades.csv"), usecols=["order", "mint", "venue", "sol_lamports", "token_raw", "trader", "pool"])
    df = df[(df["pool"] != NO_POOL) & (df["sol_lamports"] >= 10_000_000) & (df["token_raw"] > 0)].sort_values("order", kind="stable")
    df["lp"] = np.log(df["sol_lamports"] / df["token_raw"])
    med = df.groupby("pool")["lp"].transform(lambda s: s.shift(1).rolling(20, min_periods=5).median())
    df["dev"] = (df["lp"] - med).abs()
    x = df[med.notna()]
    out = x["dev"] > np.log(2)
    rep = {"dir": a.dir, "trades_checked": int(len(x)), "outlier_share": float(out.mean()),
           "by_venue": {v: {"trades": int(len(g)), "outlier_share": float((g["dev"] > np.log(2)).mean()),
                            "arb_share_of_outliers": float((g[g["dev"] > np.log(2)]["trader"] == "arb_roundtrip").mean()) if (g["dev"] > np.log(2)).any() else 0.0}
                        for v, g in x.groupby("venue")}}
    out_p = a.out or f"reports/v6/PRICE_OUTLIERS_{os.path.basename(a.dir.rstrip('/'))}.json"
    json.dump(rep, open(out_p, "w"), indent=1)
    print(f"overall outlier share {rep['outlier_share']:.4f} of {rep['trades_checked']}")
    for v, s in sorted(rep["by_venue"].items(), key=lambda kv: -kv[1]["trades"]):
        print(f"  {v:18} n={s['trades']:8} outliers {s['outlier_share']:.4f}  (arb share {s['arb_share_of_outliers']:.2f})")


if __name__ == "__main__":
    main()
