"""Feature-group ablation (prediction spec Bagian 18.4, user rule: test every component before keeping it).

usage: python3 python/ml/feature_ablation.py DIR_A DIR_B [--horizon 600] [--model random_forest|catboost]

For each feature group: train WITHOUT the group (drop-one) on one window, test on the other (both directions), and
compare with the full feature set on PR-AUC and top-1% precision for each target. A group is marked `helps` only if
dropping it lowers PR-AUC on BOTH folds for the 2x and 5x targets combined (mean change < 0 on each fold); `hurts`
if dropping it raises PR-AUC on both folds; otherwise `unclear`. Decision rule fixed before running. Short horizon,
2 windows: direction only (the final decision is strategy expectancy on walk-forward test windows).
"""
import argparse
import json
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from model_families import FEATURES, families, load, metrics  # noqa: E402

GROUPS = {
    "base_flow_counts": ["n_trades_15s", "n_trades_60s", "n_trades_300s", "buyers_15s", "buyers_60s", "sellers_60s", "sellers_15s",
                         "buy_sol_15s", "buy_sol_60s", "sell_sol_15s", "sell_sol_60s", "net_flow_15s", "net_flow_60s", "flow_imb_60s",
                         "buy_sell_count_ratio_60s"],
    "buy_sell_size": ["median_buy_60s", "max_buy_60s", "hhi_buy_60s", "retail_share_60s", "whale_buys_60s", "median_sell_60s",
                      "p90_buy_60s", "p90_sell_60s", "max_sell_15s", "sellers_5s", "sell_sol_5s", "buy_sol_5s"],
    "acceleration_ewma": ["ewma_fast", "ewma_slow", "accel", "secs_since_real_buy"],
    "price_returns": ["ret_5s", "ret_15s", "ret_60s", "ret_300s", "price_vs_high300", "price_vs_low300", "mult_since_first", "price_vs_vwap60"],
    "trader_mix": ["dust_share_60s", "repeat_buyer_share_60s", "new_wallet_share_60s", "top_buyer_share_60s"],
    "dev_creator": ["creator_sold", "creator_sell_sol", "creator_buy_sol", "dev_hold_share", "dev_net_sol", "dev_ever_sold"],
    "curve_pool": ["age_s", "newborn", "on_amm", "quote_res_sol", "curve_progress", "fee_bps", "f1_sol_in_curve", "f1_sol_per_trade",
                   "f1_trades_since_first", "log_quote_res", "curve_speed", "eta_migration_s", "created_known"],
    "bundles_concentration": ["f2_bundle_buy_share", "f2_bundle_txs_60s", "f2_early20_supply_share", "f2_top10_holder_share", "top10_supply_share"],
    "wash_multisell": ["f4_wash_share_60s", "f4_multi_sell_txs_60s"],
    "holders": ["f6_holders", "f6_holders_vs_max", "holder_vel_60s"],
    "slot_bursts": ["f7_max_slot_buyers_60s", "f7_slot_buy_std_60s"],
    "arrival_hawkes": ["f8_dt_median20", "f8_dt_p90_20", "f8_dt_cv20", "f8_last_dt", "f8_cluster_share20", "f8_hawkes_fast", "f8_hawkes_slow",
                       "f8_hawkes_ratio", "f8_hawkes_buy_sell_ratio", "hawkes_branching"],
    "smart_wallets": ["smart_buy_q_60s", "smart_share_60s", "n_smart_buyers_300s", "flipper_share_60s", "sniper_share_held", "cluster_buy_share_60s"],
    "crowd_growth": ["buyer_growth"],
    "order_flow_liq": ["buy_q_60s", "sell_q_60s", "ofi_60s", "cvd_300s", "kyle_lambda_60s", "vol_60s"],
    "overhang": ["early_retention", "profit_overhang"],
    "regime": ["attention_share_5m", "new_coins_per_min", "market_vol_10m", "sol_ret_1h"],
    "trader_speed": ["fast_wallet_share_60s", "wallet_rate_med_60s", "early20_delay_med_s"],
    "radar_state": ["rd_state", "rd_kind_new", "rd_birth_src", "rd_secs_in_state", "rd_decisions_in_state"],
    "radar_cusum": ["rd_cusum_uc", "rd_cusum_bsol", "rd_cusum_nfl", "rd_cusum_alarm_age_s", "rd_cusum_alarms", "rd_cusum_pop"],
    "radar_clusters_nfl": ["rd_clusters_60s", "rd_cluster_growth", "rd_cluster_accel", "rd_nfl_60s", "rd_liq_real_sol", "rd_clusters_known"],
    "radar_dust_split": ["rd_dust_n_60s", "rd_dust_sol_60s", "rd_dust_sol_share_60s", "rd_dust_top_cluster_share_60s",
                         "rd_split_max_sol_liq_300s", "rd_split_max_n_300s"],
    "microstructure_misc": ["secs_since_hot"],
}
TARGETS = ["y_s_2x", "y_s_5x"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("a")
    ap.add_argument("b")
    ap.add_argument("--horizon", type=int, default=600)
    ap.add_argument("--model", default="random_forest")
    ap.add_argument("--out", default="reports/v6/FEATURE_ABLATION.json")
    args = ap.parse_args()
    grouped = {f for g in GROUPS.values() for f in g}
    missing = [f for f in FEATURES if f not in grouped]
    unknown = sorted(grouped - set(FEATURES))
    assert not unknown, f"unknown feature names: {unknown}"
    if missing:
        GROUPS["other"] = missing
    data = {d: load(d, args.horizon) for d in (args.a, args.b)}
    make = families()[args.model]
    folds = ((args.a, args.b), (args.b, args.a))

    def run(cols):
        idx = [FEATURES.index(c) for c in cols]
        out = []
        for tr, te in folds:
            dtr, Xtr, wtr = data[tr]
            dte, Xte, _ = data[te]
            r = {}
            for tgt in TARGETS:
                m = make()
                if args.model == "catboost":
                    m.fit(Xtr[:, idx], dtr[tgt].to_numpy(int), sample_weight=wtr)
                else:
                    m.fit(Xtr[:, idx], dtr[tgt].to_numpy(int), **{f"{m.steps[-1][0]}__sample_weight": wtr})
                p = m.predict_proba(Xte[:, idx])[:, 1]
                r[tgt] = metrics(dte[tgt].to_numpy(int), p, dte["y_s_logmax"].to_numpy(np.float64))
            out.append(r)
        return out

    full = run(FEATURES)
    print("full:", [{t: round(f[t]["pr_auc"], 4) for t in TARGETS} for f in full], flush=True)
    rows = []
    for g, cols in GROUPS.items():
        keep = [f for f in FEATURES if f not in cols]
        res = run(keep)
        # change in PR-AUC when the group is DROPPED (negative = the group helps), mean over targets, per fold
        d = [float(np.mean([res[k][t]["pr_auc"] - full[k][t]["pr_auc"] for t in TARGETS])) for k in range(2)]
        dtop = [float(np.mean([res[k][t]["precision_top1pct"] - full[k][t]["precision_top1pct"] for t in TARGETS])) for k in range(2)]
        verdict = "helps" if all(x < 0 for x in d) else "hurts" if all(x > 0 for x in d) else "unclear"
        rows.append({"group": g, "n_features": len(cols), "delta_pr_auc_when_dropped": d, "delta_top1pct_when_dropped": dtop, "verdict": verdict})
        print(f"{g:24} n={len(cols):2} dPR(drop) {d[0]:+.4f} {d[1]:+.4f}  dTop1% {dtop[0]:+.3f} {dtop[1]:+.3f}  -> {verdict}", flush=True)
    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with open(args.out, "w") as f:
        json.dump({"label": "EXPLORATORY feature-group drop-one ablation (short horizon, 2 windows, direction only)", "model": args.model,
                   "horizon_s": args.horizon, "full": full, "groups": rows}, f, indent=1)
    print("written", args.out)


if __name__ == "__main__":
    main()
