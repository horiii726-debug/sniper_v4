"""Does the OOS signal have positive expected value? One bet per coin: the first snapshot whose OOS score
passes a threshold; outcome = marginal-price multiple at +30 min (y_end30; truncated at window end).
Net per bet = end30 * (1 - 2.5% venue fees) - 1 - tx costs (0.001 SOL round trip on a 0.05 SOL bet = 2%)."""
import sys
import numpy as np, pandas as pd
WINS = sys.argv[1].split(",")
COST = 0.02
rows = []
for w in WINS:
    d = pd.read_csv(f"{w}/snapshots.csv")
    s = pd.read_csv(f"{w}/scores_outer.csv")
    d = d.merge(s, on=["coin", "order"]).sort_values("order")
    for T in [0.2, 0.3, 0.4, 0.5, 0.6, 0.7]:
        for veto in [False, True]:
            q = d[d.p_up2 >= T]
            if veto:
                q = q[(q.p_rug < 0.5) & (q.p_devsell < 0.3)]
            first = q.groupby("coin").first()
            if len(first) == 0:
                continue
            m = first.y_end30.clip(lower=0)
            net = m * 0.975 - 1 - COST
            rows.append({"window": w.split("/")[-1], "thr": T, "veto": veto, "bets": len(first),
                         "mean_end30": round(m.mean(), 3), "median_end30": round(m.median(), 3),
                         "p_ge2x": round((m >= 2).mean(), 3), "p_le0.2": round((m <= 0.2).mean(), 3),
                         "mean_net": round(net.mean(), 3), "mean_net_excl_top1": round(net.sort_values().iloc[:-1].mean(), 3) if len(net) > 1 else np.nan,
                         "max_end30": round(m.max(), 1)})
r = pd.DataFrame(rows)
pd.set_option("display.width", 220, "display.max_rows", 200)
print(r.to_string(index=False))
