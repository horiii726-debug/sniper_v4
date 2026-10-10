"""Forensics of runners from causal snapshots (features at the radar's FIRST snapshot of each coin).
Outcome per coin = y_fwd_max at that first snapshot (max multiple reachable within 15 min before -50%)."""
import sys
import numpy as np, pandas as pd
from sklearn.cluster import KMeans
from sklearn.preprocessing import StandardScaler

wins = sys.argv[1].split(",")
frames = []
for w in wins:
    d = pd.read_csv(f"{w}/snapshots.csv")
    d = d.sort_values("order").groupby("coin", as_index=False).first()
    d["window"] = w.split("/")[-1]
    frames.append(d)
df = pd.concat(frames, ignore_index=True)
df["tier"] = pd.cut(df.y_fwd_max, [0, 2, 3, 5, 10, 1e9], labels=["<2x", "2-3x", "3-5x", "5-10x", ">=10x"], right=False)
KEY = ["age_s", "newborn", "on_amm", "quote_res_sol", "curve_progress", "mult_since_first", "ret_60s", "price_vs_high300",
       "buyers_15s", "buyers_60s", "sellers_60s", "buy_sol_60s", "sell_sol_60s", "net_flow_60s", "median_buy_60s", "max_buy_60s",
       "dust_share_60s", "repeat_buyer_share_60s", "new_wallet_share_60s", "top_buyer_share_60s", "accel",
       "creator_buy_sol", "creator_sold", "f2_bundle_buy_share", "f2_top10_holder_share", "f6_holders", "f7_max_slot_buyers_60s",
       "f8_dt_median20", "f8_hawkes_ratio"]
pd.set_option("display.width", 250, "display.max_columns", 20)
print(f"coins: {len(df)}  per tier:", df.tier.value_counts().sort_index().to_dict())
print("\n== median features at first radar snapshot, by outcome tier (all 4 windows pooled) ==")
print(df.groupby("tier", observed=False)[KEY].median().T.round(3).to_string())

print("\n== consistency: does '>=5x' differ from '<2x' in the SAME direction in every window? ==")
rows = []
for f in KEY:
    signs = []
    for w, g in df.groupby("window"):
        a, b = g[g.y_fwd_max >= 5][f].median(), g[g.y_fwd_max < 2][f].median()
        signs.append(np.sign(a - b) if pd.notna(a) and pd.notna(b) else 0)
    rows.append((f, "".join("+" if s > 0 else "-" if s < 0 else "0" for s in signs), len(set(signs)) == 1 and signs[0] != 0))
print("feature                      per-window direction (>=5x vs <2x)   consistent")
for f, s, c in rows:
    print(f"  {f:28s} {s:10s} {'YES' if c else 'no'}")

print("\n== runner archetypes (coins >=5x, KMeans k=4 on standardized key features) ==")
R = df[df.y_fwd_max >= 5].copy()
X = StandardScaler().fit_transform(R[KEY].replace([np.inf, -np.inf], np.nan).fillna(0).values)
R["arch"] = KMeans(n_clusters=4, n_init=20, random_state=7).fit_predict(X)
show = ["newborn", "on_amm", "age_s", "quote_res_sol", "mult_since_first", "buyers_60s", "buy_sol_60s", "median_buy_60s",
        "top_buyer_share_60s", "new_wallet_share_60s", "creator_buy_sol", "f2_bundle_buy_share", "y_fwd_max"]
summ = R.groupby("arch")[show].median().round(3)
summ["n"] = R.groupby("arch").size()
summ["windows_present"] = R.groupby("arch")["window"].nunique()
print(summ.T.to_string())
print("\narchetype share per window:")
print(pd.crosstab(R.arch, R.window).to_string())

print("\n== regime: base rates per window (do coins behave the same over time?) ==")
print(df.groupby("window").agg(coins=("coin", "size"), p_ge2x=("y_fwd_max", lambda s: (s >= 2).mean()),
      p_ge5x=("y_fwd_max", lambda s: (s >= 5).mean()), p_ge10x=("y_fwd_max", lambda s: (s >= 10).mean()),
      newborn_share=("newborn", "mean"), median_quote_res=("quote_res_sol", "median")).round(4).to_string())
