"""Observed honeypot / rug outcomes per coin vs on-chain mint safety flags (does the safety gate separate them?).

usage: python3 python/research/safety_labels.py data/v5_A [data/v5_B ...] [--out reports/v6/SAFETY_LABELS.json]

Per coin, from <dir>/trades.csv (every venue) and data/mint_safety.csv:
  honeypot_observed: >= 10 distinct non-creator buyers, 0 non-creator sells, buys spread over >= 600 s
                     (holders could not or did not sell while new money kept coming in);
  lp_rug_observed:   on an AMM pool, the pool's SOL reserve falls >= 80 % within 120 s from >= 5 SOL
                     (liquidity pulled) or the price falls >= 90 % within 120 s from >= 5 SOL of reserve;
  dev_dump_observed: the creator (known on pump venues) sells and the price is <= 0.3x of the pre-sell price
                     within 300 s.
EXPLORATORY: descriptive association only (current mint state, see mint_safety.rs caveat).
"""
import argparse
import json
import os

import numpy as np
import pandas as pd

FLAGS = ["mint_auth", "freeze_auth", "transfer_fee_bps", "transfer_hook", "permanent_delegate", "default_frozen", "non_transferable", "pausable"]


def coin_outcomes(df):
    df = df.sort_values("order")
    df["px"] = df["sol_lamports"] / df["token_raw"].clip(lower=1)
    out = []
    for mint, g in df.groupby("mint", sort=False):
        creators = set(g["creator"]) - {"unknown_creator"}
        nc = g[~g["trader"].isin(creators)]
        buys, sells = nc[nc["side"] == "buy"], nc[nc["side"] == "sell"]
        span = buys["block_time"].max() - buys["block_time"].min() if len(buys) else 0
        hp = (buys["trader"].nunique() >= 10) and (sells["trader"].nunique() == 0) and span >= 600
        lp = False
        amm = g[g["venue"] != "pumpfun_curve"]
        for _, p in amm.groupby("venue"):
            t = p["block_time"].to_numpy()
            q = p["quote_reserve_after"].to_numpy(np.float64) / 1e9
            px = p["px"].to_numpy(np.float64)
            j = 0
            for i in range(len(p)):
                while t[i] - t[j] > 120:
                    j += 1
                w = slice(j, i + 1)
                qmax, pmax = q[w].max(), px[w].max()
                if qmax >= 5 and (q[i] <= 0.2 * qmax or px[i] <= 0.1 * pmax):
                    lp = True
                    break
            if lp:
                break
        dd = False
        if creators:
            cs = g[(g["trader"].isin(creators)) & (g["side"] == "sell")]
            t, px = g["block_time"].to_numpy(), g["px"].to_numpy(np.float64)
            for _, r in cs.iterrows():
                before = px[(t >= r["block_time"] - 60) & (t < r["block_time"])]
                after = px[(t > r["block_time"]) & (t <= r["block_time"] + 300)]
                if len(before) and len(after) and after.min() <= 0.3 * np.median(before):
                    dd = True
                    break
        out.append({"mint": mint, "n_buyers": int(buys["trader"].nunique()), "n_sellers": int(sells["trader"].nunique()),
                    "honeypot_observed": bool(hp), "lp_rug_observed": bool(lp), "dev_dump_observed": bool(dd),
                    "venues": ",".join(sorted(g["venue"].unique()))})
    return pd.DataFrame(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--safety", default="data/mint_safety.csv")
    ap.add_argument("--out", default="reports/v6/SAFETY_LABELS.json")
    a = ap.parse_args()
    ms = pd.read_csv(a.safety)
    ms["honeypot_risk"] = (ms["freeze_auth"] | ms["transfer_hook"] | ms["permanent_delegate"] | ms["default_frozen"] | ms["non_transferable"]
                           | ms["pausable"] | (ms["transfer_fee_bps"] >= 1000)).astype(bool)
    rows = []
    for d in a.dirs:
        df = pd.read_csv(os.path.join(d, "trades.csv"), usecols=["block_time", "order", "mint", "venue", "side", "sol_lamports", "token_raw", "trader",
                                                                   "creator", "quote_reserve_after"])
        o = coin_outcomes(df)
        o["window"] = os.path.basename(d.rstrip("/"))
        rows.append(o)
    o = pd.concat(rows).merge(ms, on="mint", how="left")
    act = o[o["n_buyers"] >= 10]
    rep = {"label": "EXPLORATORY observed outcomes vs mint safety flags (coins with >= 10 non-creator buyers)", "coins": int(len(o)),
           "coins_active": int(len(act)), "by_flag": {}}
    for f in FLAGS + ["honeypot_risk"]:
        on = act[act[f].fillna(0).astype(float) > 0]
        off = act[act[f].fillna(0).astype(float) == 0]
        rep["by_flag"][f] = {k: {"n": int(len(x)), "honeypot_rate": float(x["honeypot_observed"].mean()) if len(x) else None,
                                  "lp_rug_rate": float(x["lp_rug_observed"].mean()) if len(x) else None,
                                  "dev_dump_rate": float(x["dev_dump_observed"].mean()) if len(x) else None,
                                  "honeypots": int(x["honeypot_observed"].sum())} for k, x in (("flag_on", on), ("flag_off", off))}
    rep["totals"] = {k: int(act[k].sum()) for k in ("honeypot_observed", "lp_rug_observed", "dev_dump_observed")}
    rep["honeypots_by_venue"] = act[act["honeypot_observed"]]["venues"].value_counts().head(20).to_dict()
    os.makedirs(os.path.dirname(a.out), exist_ok=True)
    json.dump(rep, open(a.out, "w"), indent=1)
    o.to_csv(a.out.replace(".json", "_coins.csv.gz"), index=False)
    print(json.dumps(rep, indent=1))


if __name__ == "__main__":
    main()
