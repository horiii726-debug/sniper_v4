"""Gate C18: fill accuracy of the simulator's pool math per venue (decides which venues may be traded).

usage: python3 python/research/fill_check.py data/v5_A [data/v5_B ...] [--out reports/v6/FILL_CHECK.json]

For every real trade <= 0.2 SOL (our bet size range) whose pool traded <= 5 s before, predict its outcome from the
pool state after the previous trade of the SAME pool (trades.csv `pool` column), with the simulator's constant
product math and fee (pool fee_bps if known, else the simulator default 125 bps):
  buy  Δq SOL  -> tokens_pred = T·Δq(1-f) / (Q + Δq(1-f))      compared with the tokens actually received
  sell tok     -> sol_pred    = Q·tok(1-f) / (T + tok(1-f))     compared with the SOL actually received
ratio = pred / actual (> 1 = the simulator is optimistic). PASS per venue (criteria fixed before measuring):
abs(median - 1) <= 0.03, >= 80 % within +-10 %, optimistic by > 5 % in <= 20 % of trades, >= 300 trades.
Curve trades are capped by real reserves in the simulator; the check uses the virtual reserves like the simulator.
"""
import argparse
import json
import os

import numpy as np
import pandas as pd

DEFAULT_FEE_BPS = 125
NO_POOL = 4294967295


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--max-sol", type=float, default=0.2)
    ap.add_argument("--max-gap", type=int, default=5)
    ap.add_argument("--out", default="reports/v6/FILL_CHECK.json")
    a = ap.parse_args()
    parts = []
    for d in a.dirs:
        df = pd.read_csv(os.path.join(d, "trades.csv"), usecols=["block_time", "order", "mint", "venue", "side", "sol_lamports", "token_raw",
                                                                   "quote_reserve_after", "token_reserve_after", "fee_bps", "pool"])
        df = df[df["pool"] != NO_POOL].sort_values("order", kind="stable")
        g = df.groupby("pool", sort=False)
        df["q_prev"] = g["quote_reserve_after"].shift(1).astype(np.float64)
        df["t_prev"] = g["token_reserve_after"].shift(1).astype(np.float64)
        df["time_prev"] = g["block_time"].shift(1)
        df["window"] = os.path.basename(d.rstrip("/"))
        parts.append(df)
    df = pd.concat(parts)
    df = df[df["q_prev"].notna() & (df["block_time"] - df["time_prev"] <= a.max_gap) & (df["sol_lamports"] <= a.max_sol * 1e9)
            & (df["sol_lamports"] > 0) & (df["token_raw"] > 0) & (df["q_prev"] > 0) & (df["t_prev"] > 0)]
    f = np.where(df["fee_bps"] > 0, df["fee_bps"], DEFAULT_FEE_BPS).astype(np.float64) / 1e4
    Q, T = df["q_prev"].to_numpy(), df["t_prev"].to_numpy()
    sol, tok = df["sol_lamports"].to_numpy(np.float64), df["token_raw"].to_numpy(np.float64)
    buy = (df["side"] == "buy").to_numpy()
    tok_pred = T * sol * (1 - f) / (Q + sol * (1 - f))
    sol_pred = Q * tok * (1 - f) / (T + tok * (1 - f))
    ratio = np.where(buy, tok_pred / tok, sol_pred / sol)
    df = df.assign(ratio=ratio)
    rep = {"label": "gate C18 fill accuracy per venue", "max_sol": a.max_sol, "max_gap_s": a.max_gap, "venues": {}}
    for v, x in df.groupby("venue"):
        r = x["ratio"].to_numpy()
        r = r[np.isfinite(r)]
        if len(r) == 0:
            continue
        med = float(np.median(r))
        w10 = float(np.mean(np.abs(r - 1) <= 0.10))
        opt = float(np.mean(r > 1.05))
        ok = abs(med - 1) <= 0.03 and w10 >= 0.80 and opt <= 0.20 and len(r) >= 300
        rep["venues"][v] = {"trades": int(len(r)), "median_ratio": med, "within_10pct": w10, "optimistic_gt5pct": opt,
                            "p10": float(np.quantile(r, 0.1)), "p90": float(np.quantile(r, 0.9)), "pass": bool(ok),
                            "buy_median": float(np.median(r[x["side"].to_numpy()[np.isfinite(x["ratio"].to_numpy())] == "buy"])) if (x["side"] == "buy").any() else None}
    os.makedirs(os.path.dirname(a.out), exist_ok=True)
    json.dump(rep, open(a.out, "w"), indent=1)
    for v, s in sorted(rep["venues"].items(), key=lambda kv: -kv[1]["trades"]):
        print(f"{v:18} n={s['trades']:7} median {s['median_ratio']:.3f} within10 {s['within_10pct']:.2f} optimistic {s['optimistic_gt5pct']:.2f} "
              f"p10 {s['p10']:.3f} p90 {s['p90']:.3f} -> {'PASS' if s['pass'] else 'FAIL'}")


if __name__ == "__main__":
    main()
