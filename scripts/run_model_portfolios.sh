#!/usr/bin/env bash
# Train each model/feature-set, then run the $100 portfolio backtest for it. Results: reports/portfolio_<tag>.json
cd /root/sniper_v4
for cfg in "catboost all" "randomforest all" "ensemble all"; do
  set -- $cfg
  python3 python/ml/compare_models.py data/bf_1h,data/h1d data/h7d,data/h30d $1 $2 2>&1 | grep -v -i warning
  ./target/release/sniper backtest-ml --dev data/bf_1h --dev data/h1d --test data/h7d --test data/h30d \
     --scores-suffix "_$1_$2" --out "reports/portfolio_$1_$2.json" > /dev/null 2>&1 && echo "portfolio $1_$2 done"
done
echo ALL_DONE
