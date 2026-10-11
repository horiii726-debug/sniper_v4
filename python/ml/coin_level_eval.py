"""Coin-level accuracy of the entry and rug models (one vote per coin, not per decision row).

usage: python3 python/ml/coin_level_eval.py models/v6_X data/v5_A [data/v5_B ...] [--out reports/v6/X/COIN_LEVEL.json]

Row-level "top 1 %" precision is dominated by a few coins with hundreds of rows each. Here every coin counts once:
for a score threshold, the coin is "bought" at its FIRST decision row (is_signal & in_window) whose score passes; the
outcome is that row's label (max within the 1 h horizon from that moment: >= 2x, >= 10x, rug/dead). Thresholds are
the score quantiles of the decision rows (top 20 / 10 / 5 / 2 / 1 / 0.5 %), so they are comparable across windows.
Scores come from the fold files (<models>/fold_<W>/scores_<W>.csv.zst: models trained only on earlier windows).
Score = P(bin >= 5) (entry_bin) for the entry model; p_rug for the rug model (higher = more rug). Base rate = share
of all coins with decision rows whose label at their first decision row is positive.
"""
import argparse
import json
import os

import numpy as np
import pandas as pd

QS = [0.80, 0.90, 0.95, 0.98, 0.99, 0.995]


def win_name(d):
    b = os.path.basename(os.path.normpath(d))
    return b[3:] if b.startswith("v5_") else b


def first_pass(df, score, thr):
    x = df[df[score] >= thr]
    return x.sort_values(["time", "order"], kind="stable").groupby("coin", sort=False).head(1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("models")
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--out")
    a = ap.parse_args()
    rep = {"label": "EXPLORATORY coin-level accuracy (first passing row per coin)", "windows": {}}
    for d in a.dirs:
        w = win_name(d)
        sp = os.path.join(a.models, f"fold_{w}", f"scores_{w}.csv.zst")
        if not os.path.exists(sp):
            continue
        s = pd.read_csv(sp, usecols=["order", "p_bin5", "p_bin6", "p_rug"])
        cols = ["order", "coin", "time", "is_signal", "in_window", "y_bin", "y_rugdead"]
        df = pd.read_csv(os.path.join(d, "snapshots_v5.csv.zst"), usecols=lambda c: c in cols)
        df = df[(df["is_signal"] == 1) & (df["in_window"] == 1)].merge(s, on="order")
        df["p10"] = df["p_bin5"] + df["p_bin6"]
        df["y2"], df["y10"] = df["y_bin"] >= 3, df["y_bin"] >= 5
        first = df.sort_values(["time", "order"], kind="stable").groupby("coin").head(1)
        r = {"coins": int(df["coin"].nunique()), "decision_rows": int(len(df)),
             "base_first_row": {"ge2x": float(first["y2"].mean()), "ge10x": float(first["y10"].mean()), "rugdead": float(first["y_rugdead"].mean())},
             "coins_ever_ge2x": int(df.groupby("coin")["y2"].any().sum()), "coins_ever_ge10x": int(df.groupby("coin")["y10"].any().sum()),
             "entry": [], "rug": []}
        for q in QS:
            thr = float(df["p10"].quantile(q))
            b = first_pass(df, "p10", thr)
            r["entry"].append({"top_rows_pct": round(100 * (1 - q), 1), "threshold": thr, "coins_bought": int(len(b)),
                               "prec_ge2x": float(b["y2"].mean()) if len(b) else None, "prec_ge10x": float(b["y10"].mean()) if len(b) else None,
                               "rugdead": float(b["y_rugdead"].mean()) if len(b) else None,
                               "share_of_10x_coins_caught": float(b["y10"].sum() / max(r["coins_ever_ge10x"], 1))})
            thr = float(df["p_rug"].quantile(q))
            v = first_pass(df, "p_rug", thr)
            r["rug"].append({"top_rows_pct": round(100 * (1 - q), 1), "threshold": thr, "coins_flagged": int(len(v)),
                             "prec_rugdead": float(v["y_rugdead"].mean()) if len(v) else None, "ge2x_among_flagged": float(v["y2"].mean()) if len(v) else None})
        rep["windows"][w] = r
        print(f"== {w}: coins {r['coins']}, base at first row >=2x {r['base_first_row']['ge2x']:.3f} >=10x {r['base_first_row']['ge10x']:.3f} "
              f"rug {r['base_first_row']['rugdead']:.3f}; coins ever >=10x {r['coins_ever_ge10x']}")
        for e in r["entry"]:
            print(f"  entry top {e['top_rows_pct']:5}% rows: {e['coins_bought']:5} coins, >=2x {e['prec_ge2x']:.3f} >=10x {e['prec_ge10x']:.3f} "
                  f"rug {e['rugdead']:.3f}, caught {e['share_of_10x_coins_caught']:.2f} of 10x coins")
        for e in r["rug"]:
            print(f"  rug   top {e['top_rows_pct']:5}% rows: {e['coins_flagged']:5} coins, rug/dead {e['prec_rugdead']:.3f}, >=2x among them {e['ge2x_among_flagged']:.3f}")
    out = a.out or os.path.join("reports/v6", os.path.basename(os.path.normpath(a.models)).replace("v6_", ""), "COIN_LEVEL.json")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    json.dump(rep, open(out, "w"), indent=1)


if __name__ == "__main__":
    main()
