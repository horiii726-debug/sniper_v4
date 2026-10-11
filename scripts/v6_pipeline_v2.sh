#!/usr/bin/env bash
# Usage: scripts/v6_pipeline_v2.sh (after scripts/v6_redecode.sh of the 4 windows, logs/redecode4_<w>.out).
# After the shared-authority / jump-filter re-decode: label sanity + fill check, training (4 windows, walk-forward),
# coin-level eval, component sweeps (fee 1.25 % and 3 %), then online learning (C17).
cd /root/sniper_v4
for w in s02d_14h s04d_04h s05d_11h s06d_18h; do
  until grep -q "$w: re-decoded" logs/redecode4_$w.out 2>/dev/null; do
    grep -q "FAILED" logs/redecode4_$w.out 2>/dev/null && { echo "$w decode FAILED"; exit 1; }
    sleep 20
  done
done
echo "$(date -u +%T) decoded"
source /venv/main/bin/activate
export LABEL_H=3600
WINS="data/v5_s02d_14h data/v5_s04d_04h data/v5_s05d_11h data/v5_s06d_18h"
REC="--rec data/v5_s02d_14h --rec data/v5_s04d_04h --rec data/v5_s05d_11h --rec data/v5_s06d_18h"
python3 -c "
import json
for w in ['s02d_14h','s04d_04h','s05d_11h','s06d_18h']:
    d=json.load(open(f'data/v5_{w}/venue_decode_stats.json')); print(w, {k:v for k,v in d['pool_legs'].items() if 'exec' in k}, 'venue skips', {k:v for k,v in d.items() if k!='pool_legs'} if len(str(d))<2000 else '')" > logs/pv2_decode_stats.out 2>&1
(python3 python/research/fill_check.py $WINS --out reports/v6/FILL_CHECK_v2_4w.json > logs/pv2_fill.out 2>&1 &)
rm -rf models/v6_run4w_fix
python3 python/ml/v5_train.py $WINS --min-train 1 --out models/v6_run4w_fix --reports reports/v6/run4w_fix --gpus 0 \
  --models entry_bin,entry_tp1,rug_cb,rug_rfcpu,hold,hazard,baseline_lr > logs/run4w_fix_train.out 2>&1 || { echo TRAIN_FAILED; exit 1; }
echo "$(date -u +%T) trained"
python3 python/ml/coin_level_eval.py models/v6_run4w_fix $WINS --out reports/v6/run4w_fix/COIN_LEVEL.json > logs/run4w_fix_coin.out 2>&1
./target/release/sniper --start-sol 0.8433 --sol-usd 118.59 --label-horizon-s 3600 v5-backtest $REC --models models/v6_run4w_fix \
  --strategy reports/v6/run4w_fix/base.json --variants reports/v6/run4w_fix/variants.json --out reports/v6/run4w_fix/SWEEP.json > logs/run4w_fix_sweep.out 2>&1 &
./target/release/sniper --start-sol 0.8433 --sol-usd 118.59 --label-horizon-s 3600 v5-backtest $REC --models models/v6_run4w_fix \
  --strategy reports/v6/run4w_fix/base.json --variants reports/v6/run4w_fix/variants.json --default-fee-bps 300 --out reports/v6/run4w_fix/SWEEP_fee300.json > logs/run4w_fix_sweep_fee300.out 2>&1 &
wait
echo "$(date -u +%T) SWEEP_DONE"
python3 python/ml/online_learn.py $WINS --every 10 --target y_2x --out reports/v6/run4w_fix/ONLINE_LEARN.json > logs/run4w_fix_online.out 2>&1
echo "$(date -u +%T) ONLINE_DONE"
