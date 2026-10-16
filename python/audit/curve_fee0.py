"""Phase 1.1: inspect pump.fun curve TradeEvents with fee_bps == 0 from a pump-check --dump file."""
import json, sys, collections
rows = [json.loads(l) for l in open(sys.argv[1])]
curve = [r for r in rows if 'virtual_sol_reserves' in r['raw']]
z = [r for r in curve if r['raw'].get('fee_basis_points', 0) + r['raw'].get('creator_fee_basis_points', 0) == 0]
print('curve events', len(curve), 'fee0', len(z))
keys = collections.Counter()
for r in z:
    for k, v in r['raw'].items():
        if v not in (0, '', False, None, '11111111111111111111111111111111'):
            keys[k] += 1
print('nonzero fields in fee0:', dict(keys))
print('ix_name:', collections.Counter(r['raw'].get('ix_name') for r in z), 'vs all', collections.Counter(r['raw'].get('ix_name') for r in curve))
print('mayhem:', collections.Counter(r['raw'].get('mayhem_mode') for r in z), 'vs all', collections.Counter(r['raw'].get('mayhem_mode') for r in curve))
print('truncated:', sum('_truncated' in r['raw'] for r in z))
for r in z[:3]:
    print(json.dumps({k: v for k, v in r['raw'].items()}, sort_keys=True)[:1500])
