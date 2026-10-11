"""Price check of program-agnostic pool legs (other_venue, stable_quoted) against validated venue trades of the
same coin (pump curve / PumpSwap / single-venue decodes) within +-10 s. A correct decode gives ratios near 1.

usage: python3 python/research/generic_leg_check.py data/v5_X [--out reports/v6/GENERIC_LEG_CHECK_<X>.json]
"""
import argparse
import json
import os

import numpy as np
import pandas as pd


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--out")
    a = ap.parse_args()
    df = pd.read_csv(os.path.join(a.dir, "trades.csv"), usecols=["block_time", "mint", "venue", "sol_lamports", "token_raw"])
    df = df[(df["sol_lamports"] > 1_000_000) & (df["token_raw"] > 0)]
    df["px"] = df["sol_lamports"] / df["token_raw"]
    gen = df[df["venue"].isin(["other_venue", "stable_quoted"])]
    ref = df[~df["venue"].isin(["other_venue", "stable_quoted"])].sort_values("block_time")
    rep = {"dir": a.dir, "events_by_venue": df["venue"].value_counts().to_dict()}
    for v in ("other_venue", "stable_quoted"):
        g = gen[gen["venue"] == v].sort_values("block_time")
        m = pd.merge_asof(g, ref[["block_time", "mint", "px"]].rename(columns={"px": "px_ref"}), on="block_time", by="mint",
                          direction="nearest", tolerance=10).dropna(subset=["px_ref"])
        r = (m["px"] / m["px_ref"]).to_numpy()
        rep[v] = {"legs": int(len(g)), "coins": int(g["mint"].nunique()), "matched": int(len(r)),
                  "coins_without_reference_venue": int(len(set(g["mint"]) - set(ref["mint"])))}
        if len(r):
            rep[v].update({"ratio_median": float(np.median(r)), "ratio_p10": float(np.quantile(r, 0.1)), "ratio_p90": float(np.quantile(r, 0.9)),
                           "within_10pct": float(np.mean(np.abs(r - 1) <= 0.10))})
    out = a.out or f"reports/v6/GENERIC_LEG_CHECK_{os.path.basename(a.dir.rstrip('/'))}.json"
    json.dump(rep, open(out, "w"), indent=1)
    print(json.dumps(rep, indent=1))


if __name__ == "__main__":
    main()
