"""Effective swap fee per venue and pool age from the buy/sell spread (C18 complement: the fill check cannot see fees).

usage: python3 python/research/fee_spread.py TRADES_CSV [--out reports/v6/FEE_SPREAD_<name>.json]

For execution-priced pools the simulator's reserves come from the previous swap's execution price, which already contains
that swap's fee in its direction, so the fill check (consecutive small swaps) cancels the fee out and cannot measure it.
Here: consecutive swaps of the SAME pool, opposite sides, <= 2 s apart, both 0.01-0.5 SOL. Execution price (SOL per raw
token) of the buy over the sell ~ (1 + f) / (1 - f) x price impact, so f ~ (ratio - 1) / (ratio + 1) is an upper bound of
the fee (impact of small swaps adds a little). Reported per venue and pool age (seconds since the pool's first swap):
median and p75 of f. The simulator charges `default_fee_bps` (125) per side on pools without a decoded fee.
"""
import argparse
import json
import os

import numpy as np
import pandas as pd

NO_POOL = 4294967295
AGES = [0, 60, 300, 1800, 10**9]
AGE_NAMES = ["<1m", "1-5m", "5-30m", ">30m"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("trades")
    ap.add_argument("--out")
    a = ap.parse_args()
    df = pd.read_csv(a.trades, usecols=["block_time", "order", "venue", "side", "sol_lamports", "token_raw", "pool", "fee_bps"])
    df = df[(df["pool"] != NO_POOL) & (df["token_raw"] > 0)].sort_values("order", kind="stable")
    df["first"] = df.groupby("pool")["block_time"].transform("min")
    df["px"] = df["sol_lamports"] / df["token_raw"]
    g = df.groupby("pool", sort=False)
    for c in ["px", "side", "block_time", "sol_lamports"]:
        df["prev_" + c] = g[c].shift(1)
    x = df[(df["side"] != df["prev_side"]) & df["prev_side"].notna() & (df["block_time"] - df["prev_block_time"] <= 2)
           & df["sol_lamports"].between(1e7, 5e8) & df["prev_sol_lamports"].between(1e7, 5e8)].copy()
    buy_px = np.where(x["side"] == "buy", x["px"], x["prev_px"])
    sell_px = np.where(x["side"] == "buy", x["prev_px"], x["px"])
    r = buy_px / sell_px
    x["f"] = (r - 1) / (r + 1)
    x = x[np.isfinite(x["f"]) & (x["f"] > -0.5) & (x["f"] < 0.9)]
    x["age"] = pd.cut(x["block_time"] - x["first"], AGES, labels=AGE_NAMES, right=False)
    rep = {"label": "effective fee per side from the buy/sell spread (upper bound)", "trades": a.trades, "venues": {}}
    for v, gv in x.groupby("venue"):
        rep["venues"][v] = {"pairs": int(len(gv)), "median_f": float(gv["f"].median()), "p75_f": float(gv["f"].quantile(0.75)),
                            "decoded_fee_bps_median": float(gv["fee_bps"].median()),
                            "by_age": {str(k): {"pairs": int(len(ga)), "median_f": float(ga["f"].median()), "p75_f": float(ga["f"].quantile(0.75))}
                                       for k, ga in gv.groupby("age", observed=True) if len(ga) >= 20}}
    out = a.out or f"reports/v6/FEE_SPREAD_{os.path.basename(os.path.dirname(os.path.abspath(a.trades)))}.json"
    os.makedirs(os.path.dirname(out), exist_ok=True)
    json.dump(rep, open(out, "w"), indent=1)
    for v, s in sorted(rep["venues"].items(), key=lambda kv: -kv[1]["pairs"]):
        ages = "  ".join(f"{k}: {b['median_f']*1e4:6.0f}bps(n={b['pairs']})" for k, b in s["by_age"].items())
        print(f"{v:18} pairs {s['pairs']:7}  fee median {s['median_f']*1e4:6.0f} bps  p75 {s['p75_f']*1e4:6.0f}  decoded {s['decoded_fee_bps_median']:4.0f} | {ages}")


if __name__ == "__main__":
    main()
