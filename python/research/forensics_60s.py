"""Forensics at the snapshot ~60 s after a coin's first radar snapshot (coin still alive), outcome from that point."""
import sys
import numpy as np, pandas as pd
wins = sys.argv[1].split(",")
rows = []
for w in wins:
    d = pd.read_csv(f"{w}/snapshots.csv").sort_values("order")
    first = d.groupby("coin").time.transform("min")
    d = d[(d.time - first >= 50) & (d.time - first <= 90)]
    d = d.groupby("coin", as_index=False).first()
    d["window"] = w.split("/")[-1]
    rows.append(d)
df = pd.concat(rows, ignore_index=True)
df["tier"] = pd.cut(df.y_fwd_max, [0, 2, 3, 5, 10, 1e9], labels=["<2x", "2-3x", "3-5x", "5-10x", ">=10x"], right=False)
KEY = ["mult_since_first", "ret_15s", "ret_60s", "price_vs_high300", "quote_res_sol", "curve_progress", "buyers_15s", "buyers_60s",
       "sellers_60s", "buy_sol_60s", "sell_sol_60s", "net_flow_60s", "flow_imb_60s", "median_buy_60s", "max_buy_60s", "dust_share_60s",
       "repeat_buyer_share_60s", "new_wallet_share_60s", "top_buyer_share_60s", "accel", "creator_buy_sol", "creator_sold",
       "creator_sell_sol", "f2_bundle_buy_share", "f2_early20_supply_share", "f2_top10_holder_share", "f6_holders", "f6_holders_vs_max",
       "f7_max_slot_buyers_60s", "f8_dt_median20", "f8_hawkes_ratio", "f8_hawkes_buy_sell_ratio"]
pd.set_option("display.width", 250, "display.max_columns", 20)
print(f"coins alive at ~60 s: {len(df)}  per tier:", df.tier.value_counts().sort_index().to_dict())
print(df.groupby("tier", observed=False)[KEY].median().T.round(3).to_string())
print("\nconsistent direction (>=5x vs <2x) in every window, with lift of P(>=5x) top vs bottom quintile:")
for f in KEY:
    signs = []
    for w, g in df.groupby("window"):
        a, b = g[g.y_fwd_max >= 5][f].median(), g[g.y_fwd_max < 2][f].median()
        signs.append(np.sign(a - b) if pd.notna(a) and pd.notna(b) else 0)
    if len(set(signs)) == 1 and signs[0] != 0:
        x = df[f].rank(method="first"); q = pd.qcut(x, 5, labels=False)
        p = df.groupby(q).y_fwd_max.apply(lambda s: (s >= 5).mean())
        print(f"  {f:26s} {'higher' if signs[0] > 0 else 'lower '} for runners in 4/4 windows | P(>=5x) Q1 {p.iloc[0]*100:.1f}% .. Q5 {p.iloc[-1]*100:.1f}%")
