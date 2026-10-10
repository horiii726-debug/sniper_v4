"""Characteristics of coins by outcome tier (offline research, Stage 1 radar input).

Features: the coin's first EARLY seconds of activity in the window (from census coins.csv).
Outcome: peak price strictly AFTER the early window / price at its end ("after_early_peak_mult").
So feature and outcome never overlap in time.
Prints: (1) tier sizes, (2) median early features per exclusive tier, (3) lift tables:
P(outcome >= 2x / >= 5x) by feature quintile.
"""
import sys
import numpy as np
import pandas as pd

df = pd.read_csv(sys.argv[1])
df = df[np.isfinite(df["after_early_peak_mult"])].copy()
m = df["after_early_peak_mult"]
edges = [0, 1.5, 2, 3, 5, 10, 20, 50, 100, np.inf]
labels = ["<1.5x", "1.5-2x", "2-3x", "3-5x", "5-10x", "10-20x", "20-50x", "50-100x", ">=100x"]
df["tier"] = pd.cut(m, edges, labels=labels, right=False)
df["early_buy_sell_ratio"] = df["early_buy_sol"] / (df["early_sell_sol"] + 1e-9)
df["early_buyer_seller_ratio"] = df["early_buyers"] / (df["early_sellers"] + 1)
df["early_net_flow_sol"] = df["early_buy_sol"] - df["early_sell_sol"]
feats = ["early_trades", "early_buyers", "early_sellers", "early_buyer_seller_ratio", "early_buy_sol", "early_sell_sol",
         "early_net_flow_sol", "early_buy_sell_ratio", "early_median_buy_sol", "early_max_buy_sol", "early_dust_buy_share",
         "early_repeat_buyer_share", "early_new_wallet_share", "early_price_mult", "newborn_in_window"]

print(f"coins with outcome: {len(df)}  (newborn in window: {int(df.newborn_in_window.sum())})")
print("\n== cumulative tiers (peak after early window) ==")
for x in [2, 3, 5, 10, 20, 50, 100]:
    print(f"  >= {x:>3}x : {int((m >= x).sum()):5d}  ({(m >= x).mean()*100:5.2f}%)")

pd.set_option("display.width", 250, "display.max_columns", 30)
print("\n== median early features per exclusive tier (n = coins) ==")
g = df.groupby("tier", observed=False)
tab = g[feats].median().T
tab.columns = [f"{c} (n={int(g.size()[c])})" for c in tab.columns]
print(tab.round(3).to_string())

print("\n== lift: P(peak after early >= 2x) and >= 5x by feature quintile (Q1 low .. Q5 high) ==")
base2, base5 = (m >= 2).mean(), (m >= 5).mean()
print(f"base rate >=2x {base2*100:.1f}%  >=5x {base5*100:.1f}%")
for f in feats:
    if f == "newborn_in_window":
        s = df.groupby(f)["after_early_peak_mult"]
        r = [f"{k}: >=2x {(v >= 2).mean()*100:4.1f}% >=5x {(v >= 5).mean()*100:4.1f}% n={len(v)}" for k, v in s]
        print(f"  {f:28s} " + " | ".join(r))
        continue
    x = df[f].replace([np.inf, -np.inf], np.nan).dropna()
    if x.nunique() < 5:
        continue
    q = pd.qcut(x.rank(method="first"), 5, labels=["Q1", "Q2", "Q3", "Q4", "Q5"])
    mm = m.loc[x.index]
    row = []
    for lab in ["Q1", "Q2", "Q3", "Q4", "Q5"]:
        sel = mm[q == lab]
        row.append(f"{lab} {(sel >= 2).mean()*100:4.1f}/{(sel >= 5).mean()*100:4.1f}")
    print(f"  {f:28s} " + "  ".join(row) + f"   (Q5 starts at {x[q == 'Q5'].min():.3g})")
