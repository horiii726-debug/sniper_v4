import json, sys
d = json.load(open(sys.argv[1]))
keys = ['window_s', 'coins_with_price_path', 'newborn_coins', 'promoted_coins', 'hot_max_concurrent', 'precision_2x',
        'precision_5x', 'naive_n', 'naive_precision_2x', 'base_rate_run_2x']
print({k: (round(d[k], 3) if isinstance(d[k], float) else d[k]) for k in keys})
print('params', d['params'])
for name in ['tiers_newborn', 'tiers_existing']:
    print(name)
    for t in d[name]:
        up = t['median_upside_after_flag']
        rc = t['recall']
        print(f"  >={t['tier']:>5}x runners {t['runners']:3d} (instant {t['instant_jumps']:2d}) caught {t['caught']:3d} recall "
              f"{'-' if rc is None else f'{rc:.2f}'}  naive {t['naive_caught']:3d}  median upside after flag "
              f"{'-' if up is None else f'{up:.1f}x'}")
