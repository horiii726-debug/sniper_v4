"""Minute-by-minute profile ("coin card") of example coins: what the radar / ML sees for one coin.

usage: python3 python/research/coin_card.py data/v5_X [--mints M1,M2] [--auto 3] [--sol-usd 118.59] [--minutes 20]

Per minute (all venues of the coin): REAL buys / sells (>= 0.01 SOL, count), buy / sell volume in SOL and USD, median buy size (USD),
distinct buyers / sellers, NEW buyers (first trade of the wallet in this window), buyer acceleration (distinct buyers
this minute - previous minute), dust buys (< 0.01 SOL), price (last trade of the coin's deepest pool, as a multiple of
the first minute's price) and liquidity (SOL in that pool). `--auto N` picks N coins of each outcome
class from reports/v6/MOONSHOT_TRAITS_<dir>_coins.csv.gz (moonshot >= 10x, runner 2-10x, fizzle < 1.5x).
"""
import argparse
import os

import numpy as np
import pandas as pd

NO_POOL = 4294967295


def card(g, sol_usd, minutes, t_from):
    # price / liquidity from the coin's deepest pool (largest median SOL reserve); counts from every venue
    real_all = g[g["sol_lamports"] >= 10_000_000]
    main = (real_all if len(real_all) else g)["pool"].value_counts().idxmax()   # pool with the most real trades
    g = g[(g["block_time"] >= t_from) & (g["block_time"] < t_from + 60 * minutes)].copy()
    g["m"] = ((g["block_time"] - t_from) // 60).astype(int)
    rows, prev_buyers, seen = [], 0, set()
    p_first = None
    for m in range(minutes):
        x = g[g["m"] == m]
        b, s = x[x["side"] == "buy"], x[x["side"] == "sell"]
        dust_b = b[b["sol_lamports"] < 10_000_000]
        b = b[b["sol_lamports"] >= 10_000_000]          # real buys (>= 0.01 SOL); dust shown separately
        s = s[s["sol_lamports"] >= 10_000_000]
        buyers = set(b["trader"]) - {"arb_roundtrip"}
        new = len(buyers - seen)
        seen |= buyers
        xm = x[x["pool"] == main]
        px = xm["px"].iloc[-1] if len(xm) else np.nan
        if p_first is None and np.isfinite(px):
            p_first = px
        rows.append({"min": m, "buys": len(b), "sells": len(s), "buy_sol": round(b["sol_lamports"].sum() / 1e9, 3),
                     "sell_sol": round(s["sol_lamports"].sum() / 1e9, 3), "buy_usd": round(b["sol_lamports"].sum() / 1e9 * sol_usd, 1),
                     "sell_usd": round(s["sol_lamports"].sum() / 1e9 * sol_usd, 1),
                     "med_buy_usd": round(float(np.median(b["sol_lamports"])) / 1e9 * sol_usd, 2) if len(b) else 0.0,
                     "buyers": len(buyers), "sellers": s["trader"].nunique(), "new_buyers": new, "accel": len(buyers) - prev_buyers,
                     "dust_buys": len(dust_b), "dust_wallets": dust_b["trader"].nunique(), "price_x": round(px / p_first, 2) if p_first and np.isfinite(px) else np.nan,
                     "liq_sol": round(xm["q"].iloc[-1] / 1e9, 1) if len(xm) else np.nan})
        prev_buyers = len(buyers)
    return pd.DataFrame(rows)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--mints", default="")
    ap.add_argument("--auto", type=int, default=1)
    ap.add_argument("--sol-usd", type=float, default=118.59)
    ap.add_argument("--minutes", type=int, default=15)
    a = ap.parse_args()
    name = os.path.basename(a.dir.rstrip("/"))
    df = pd.read_csv(os.path.join(a.dir, "trades.csv"), usecols=["block_time", "order", "mint", "venue", "side", "sol_lamports", "token_raw", "trader",
                                                                   "quote_reserve_after", "token_reserve_after", "pool"], dtype={"token_reserve_after": str})
    picks = [(m, "given") for m in a.mints.split(",") if m]
    tf = f"reports/v6/MOONSHOT_TRAITS_w1_coins.csv.gz" if name == "v5_s02d_14h" else f"reports/v6/MOONSHOT_TRAITS_{name}_coins.csv.gz"
    T = pd.read_csv(tf) if os.path.exists(tf) else None
    if not picks and T is not None:
        for cls in ("moonshot_10x", "runner_2_10x", "fizzle_lt_1.5x"):
            x = T[T["class"] == cls].sort_values("buy_sol_5m", ascending=False).head(a.auto)
            picks += [(m, cls) for m in x["mint"]]
    df = df[df["mint"].isin([m for m, _ in picks])].sort_values("order", kind="stable")
    df["q"] = df["quote_reserve_after"].astype(np.float64)
    df["px"] = df["q"] / df["token_reserve_after"].astype(np.float64)
    pd.set_option("display.width", 250)
    pd.set_option("display.max_columns", 30)
    for m, cls in picks:
        g = df[df["mint"] == m]
        if g.empty:
            continue
        info = T[T["mint"] == m].iloc[0] if T is not None and (T["mint"] == m).any() else None
        t0 = int(info["age_at_t0_s"] + g["block_time"].min()) if info is not None else int(g["block_time"].min())
        print(f"\n=== {cls}: {m}  venues {sorted(g['venue'].unique())}  max multiple after takeoff "
              f"{info['max_mult']:.1f}x" if info is not None else f"\n=== {cls}: {m}")
        print(f"    minutes from takeoff-5 min (takeoff = minute 5, price >= 1.10 x 5-minute low)")
        print(card(g, a.sol_usd, a.minutes, t0 - 300).to_string(index=False))


if __name__ == "__main__":
    main()
