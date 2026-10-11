#!/usr/bin/env bash
# V6 ML run on the TRAIN windows given as arguments (never the final holdout windows):
#   1. model families one at a time (logistic, PCA + logistic, random forest, CatBoost), leave one window out  (C8)
#   2. V5 models walk-forward with purge (entry_bin, entry_tp1, rug CB + RF, hold, hazard, logistic baseline)
#   3. online learning vs walk-forward (update every 10 coins with known outcomes)                          (C17)
#   4. walk-forward backtest, 100 USD capital, strategy grid tuned on these dev windows, per exit mode       (C16)
# Usage: scripts/v6_ml_run.sh OUT_TAG data/v5_A data/v5_B ...   (chronological order not required)
set -u
cd "$(dirname "$0")/.."
TAG=$1; shift
WINS=("$@")
OUT=reports/v6/$TAG
MOD=models/v6_$TAG
mkdir -p $OUT logs
source /venv/main/bin/activate
export LABEL_H=3600
START_SOL=${START_SOL:-0.8433}   # 100 USD at the on-chain SOL/USD of window 1 (118.59)
SOL_USD=${SOL_USD:-118.59}
echo "$(date -u +%FT%TZ) families"
python3 python/ml/model_families_v6.py "${WINS[@]}" --out $OUT/MODEL_FAMILIES_V6.json > logs/${TAG}_families.out 2>&1
echo "$(date -u +%FT%TZ) v5_train"
python3 python/ml/v5_train.py "${WINS[@]}" --min-train 2 --out $MOD --reports $OUT --gpus 0 \
  --models entry_bin,entry_tp1,rug_cb,rug_rfcpu,hold,hazard,baseline_lr > logs/${TAG}_train.out 2>&1
echo "$(date -u +%FT%TZ) online"
python3 python/ml/online_learn.py "${WINS[@]}" --every 10 --target y_2x --out $OUT/ONLINE_LEARN.json > logs/${TAG}_online.out 2>&1
REC=(); for w in "${WINS[@]}"; do REC+=(--rec "$w"); done
for mode in trail adaptive_trail momentum_fade; do
  echo "{\"exit_mode\":\"$mode\",\"time_stop_s\":300,\"time_stop_max_buyers60\":3.0,\"max_positions\":18,\"bet_sol\":$(python3 -c "print($START_SOL/18)"),\"max_total_exposure\":1.0,\"max_entries_per_min\":1000000}" > $OUT/base_$mode.json
  echo "$(date -u +%FT%TZ) backtest $mode"
  ./target/release/sniper --start-sol $START_SOL --sol-usd $SOL_USD --label-horizon-s 3600 v5-backtest "${REC[@]}" --models $MOD \
    --strategy $OUT/base_$mode.json --grid --mc-runs 20 --out $OUT/BACKTEST_ML_$mode.json > logs/${TAG}_bt_$mode.out 2>&1
done
echo "$(date -u +%FT%TZ) done"
