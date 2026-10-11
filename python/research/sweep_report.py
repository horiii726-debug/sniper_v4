"""Readable summary of a component sweep (`sniper v5-backtest --variants`): equity from 100 USD, trades, win rate, PF,
drawdown, moonshots caught, and the per-venue funnel of >= 10x coins.

usage: python3 python/research/sweep_report.py reports/v6/run4w_fix/SWEEP.json [--variant base] [--md out.md]

Equity: each test window starts from the same capital (100 USD); "chained" multiplies the window returns (the
windows run one after another), "per window" lists them. Trades / wins / PF are pooled over the test windows.
"""
import argparse
import json
from collections import defaultdict


def pooled(v):
    trades = [t for w in v["windows"] for t in w["trades_list"]]
    gp = sum(t["pnl_sol"] for t in trades if t["pnl_sol"] > 0)
    gl = -sum(t["pnl_sol"] for t in trades if t["pnl_sol"] < 0)
    chained = 100.0
    for w in v["windows"]:
        chained *= 1 + w["return_pct"] / 100
    return {
        "chained_usd": chained,
        "per_window_usd": [round(100 * (1 + w["return_pct"] / 100), 1) for w in v["windows"]],
        "trades": len(trades),
        "win_rate": sum(t["pnl_sol"] > 0 for t in trades) / len(trades) if trades else 0.0,
        "pf": gp / gl if gl > 0 else float("inf") if gp > 0 else 0.0,
        "max_dd": max((w["max_drawdown_pct"] for w in v["windows"]), default=0.0),
        "runners_10x": sum(w["runners_10x"] for w in v["windows"]),
        "caught_5x": sum(w["captured_5x"] for w in v["windows"]),
        "decision_min": sum(w["window_s"] for w in v["windows"]) / 60,
    }


def venue_funnel(v):
    out = defaultdict(lambda: defaultdict(int))
    for w in v["windows"]:
        for ven, bars in w["funnel"]["coins_10x_by_venue"].items():
            for b, n in bars.items():
                out[ven][b] += n
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("sweep")
    ap.add_argument("--variant", default="base")
    ap.add_argument("--md")
    a = ap.parse_args()
    d = json.load(open(a.sweep))
    rows = sorted(((v["variant"], pooled(v)) for v in d["variants"]), key=lambda r: -r[1]["chained_usd"])
    lines = [f"Test windows: {', '.join(d['test_windows'])}; decision time {rows[0][1]['decision_min']:.0f} min in total.", "",
             "| variant | 100 USD -> (chained) | per window | trades | win rate | PF | max DD % | 10x coins | caught >= 5x |",
             "|---|---|---|---|---|---|---|---|---|"]
    for n, p in rows:
        lines.append(f"| {n} | {p['chained_usd']:.1f} | {p['per_window_usd']} | {p['trades']} | {p['win_rate']:.0%} | {p['pf']:.2f} | "
                     f"{p['max_dd']:.1f} | {p['runners_10x']} | {p['caught_5x']} |")
    v = next(x for x in d["variants"] if x["variant"] == a.variant)
    lines += ["", f"Per-venue funnel of >= 10x coins, variant `{a.variant}` (last barrier of each coin):", "",
              "| venue | 10x coins | entered | last barriers |", "|---|---|---|---|"]
    for ven, bars in sorted(venue_funnel(v).items(), key=lambda kv: -sum(kv[1].values())):
        tot = sum(bars.values())
        lines.append(f"| {ven} | {tot} | {bars.get('entered', 0)} | " + ", ".join(f"{b} {n}" for b, n in sorted(bars.items(), key=lambda kv: -kv[1])) + " |")
    txt = "\n".join(lines)
    print(txt)
    if a.md:
        open(a.md, "w").write(txt + "\n")


if __name__ == "__main__":
    main()
