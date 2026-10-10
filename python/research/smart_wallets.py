"""Smart-wallet copy-trade experiment, leave-one-window-out (wallet skill learned only from the other windows).

Early buyer: among the first EARLY_N distinct buyers of a coin (>= 0.01 SOL, not the creator).
Outcome of a buy at time t: our entry = marginal price of the first event >= t + 1 s (latency);
peak / end = max / last marginal price within 30 min; mirror = price 1 s after the wallet's first later sell (else end).
Smart wallet (from training windows): >= MIN_HITS early buys whose peak >= HIT_X, hit rate >= MIN_RATE.
Test: copy the first smart-wallet early buy in each coin. Baseline: copy one random non-smart early buyer per coin.
Net per bet = multiple * 0.975 - 1 - 0.02 (venue fees + tx costs on a 0.05 SOL bet)."""
import sys
import numpy as np, pandas as pd

WINS = sys.argv[1].split(",")
EARLY_N, HIT_X, MIN_HITS, MIN_RATE, H = 20, 5.0, 2, 0.2, 1800


def load(w):
    d = pd.read_csv(f"{w}/trades.csv", usecols=["block_time", "order", "mint", "venue", "side", "sol_lamports", "trader", "creator",
                                                "quote_reserve_after", "token_reserve_after"])
    d = d.sort_values("order").reset_index(drop=True)
    d["px"] = d.quote_reserve_after / d.token_reserve_after.replace(0, np.nan)
    first_amm = d[d.venue == "pumpswap"].groupby("mint").order.min()
    fa = d.mint.map(first_amm)
    d = d[~((d.venue == "pumpfun_curve") & fa.notna() & (d.order > fa))].reset_index(drop=True)
    return d


def early_outcomes(d):
    """One row per early buy with peak / end / mirror multiples (NaN when no later price)."""
    rows = []
    for mint, g in d.groupby("mint", sort=False):
        t = g.block_time.values
        px = g.px.values
        od = g.order.values
        tr = g.trader.values
        sd = g.side.values
        creator = g.creator.values[0]
        seen = set()
        for i in np.where((sd == "buy") & (g.sol_lamports.values >= 10_000_000) & (tr != creator))[0]:
            if tr[i] in seen:
                continue
            seen.add(tr[i])
            if len(seen) > EARLY_N:
                break
            j0 = np.searchsorted(t, t[i] + 1)
            if j0 >= len(t) or not np.isfinite(px[j0]) or px[j0] <= 0:
                rows.append((mint, tr[i], t[i], od[i], np.nan, np.nan, np.nan))
                continue
            j1 = np.searchsorted(t, t[i] + H, side="right")
            seg = px[j0:j1]
            peak, end = np.nanmax(seg) / px[j0], seg[-1] / px[j0]
            later_sells = np.where((tr == tr[i]) & (sd == "sell") & (od > od[i]) & (t <= t[i] + H))[0]
            mirror = end
            if len(later_sells):
                k = np.searchsorted(t, t[later_sells[0]] + 1)
                if k < len(t):
                    mirror = px[k] / px[j0]
            rows.append((mint, tr[i], t[i], od[i], peak, end, mirror))
    return pd.DataFrame(rows, columns=["mint", "trader", "t", "order", "peak", "end", "mirror"])


def summarize(name, r):
    r = r.dropna(subset=["peak"])
    if len(r) == 0:
        return {"set": name, "bets": 0}
    out = {"set": name, "bets": len(r), "p_peak>=2x": round((r.peak >= 2).mean(), 3), "p_peak>=5x": round((r.peak >= 5).mean(), 3)}
    for col in ["end", "mirror"]:
        net = r[col].clip(lower=0) * 0.975 - 1 - 0.02
        out[f"{col}_median"] = round(r[col].median(), 3)
        out[f"{col}_mean_net"] = round(net.mean(), 3)
        out[f"{col}_net_excl_top1"] = round(net.sort_values().iloc[:-1].mean(), 3) if len(net) > 1 else np.nan
    return out


eo = {}
for w in WINS:
    eo[w] = early_outcomes(load(w))
    print(f"{w}: {len(eo[w])} early buys, {eo[w].mint.nunique()} coins, {eo[w].trader.nunique()} wallets", flush=True)
rows = []
for w in WINS:
    tr = pd.concat([eo[x] for x in WINS if x != w]).dropna(subset=["peak"])
    s = tr.assign(hit=tr.peak >= HIT_X).groupby("trader").hit.agg(["sum", "count"])
    smart = set(s[(s["sum"] >= MIN_HITS) & (s["sum"] / s["count"] >= MIN_RATE)].index)
    te = eo[w].sort_values("order")
    sm = te[te.trader.isin(smart)].groupby("mint").head(1)
    base = te[~te.trader.isin(smart)].sample(frac=1, random_state=7).groupby("mint").head(1)
    seen_in_test = te.trader.isin(smart).sum()
    rows.append(summarize(f"{w.split('/')[-1]} SMART copy ({len(smart)} smart wallets, {seen_in_test} early buys in test)", sm))
    rows.append(summarize(f"{w.split('/')[-1]} baseline: random early buyer", base))
pd.set_option("display.width", 260, "display.max_colwidth", 70)
print(pd.DataFrame(rows).to_string(index=False))
