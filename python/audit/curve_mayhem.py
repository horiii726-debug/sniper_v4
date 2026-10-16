"""Phase 1.1: mayhem-mode curve events. Who trades fee-free, and do virtual reserves predict the next trade?"""
import json, sys, collections
rows = [json.loads(l) for l in open(sys.argv[1])]
curve = [r['raw'] for r in rows if 'virtual_sol_reserves' in r['raw']]
fee = lambda v: v['fee_basis_points'] + v['creator_fee_basis_points']
may = [v for v in curve if v['mayhem_mode']]
print('mayhem events', len(may), 'fee0 among mayhem', sum(fee(v) == 0 for v in may))
print('fee0 users', collections.Counter(v['user'] for v in may if fee(v) == 0).most_common(5))
print('fee>0 users in mayhem (top)', collections.Counter(v['user'] for v in may if fee(v) > 0).most_common(3))
off = lambda v: (v['virtual_sol_reserves'] - v['real_sol_reserves']) / 1e9
print('virtual-real SOL offset: normal', sorted(round(off(v), 3) for v in curve if not v['mayhem_mode'])[::200])
print('  mayhem fee>0', sorted(round(off(v), 3) for v in may if fee(v) > 0)[::40])
print('  mayhem fee0 ', sorted(round(off(v), 3) for v in may if fee(v) == 0)[::40])
# Next-trade prediction: from event k post-reserves, constant product predicts tokens of event k+1 (same mint).
by = collections.defaultdict(list)
for v in curve: by[v['mint']].append(v)
res = collections.defaultdict(list)
for m, ev in by.items():
    for a, b in zip(ev, ev[1:]):
        x, y = a['virtual_sol_reserves'], a['virtual_token_reserves']
        s = b['sol_amount']
        pred = y - x * y // (x + s) if b['is_buy'] else x * y // (y - b['token_amount']) - x if False else None
        if b['is_buy']:
            err = abs(pred - b['token_amount']) / max(b['token_amount'], 1)
        else:
            out = s; pred_out = x - x * y // (y + b['token_amount']); err = abs(pred_out - out) / max(out, 1)
        res[(a['mayhem_mode'], fee(a) == 0, fee(b) == 0)].append(err)
for k, e in sorted(res.items()):
    e.sort(); n = len(e)
    print(f'mayhem={k[0]} prev_fee0={k[1]} next_fee0={k[2]} n={n} within0.1%={sum(x < 1e-3 for x in e)/n:.3f} median_err={e[n//2]:.4f}')
# k jump across each event: pre-state = previous event's post-state (same mint).
jumps = collections.defaultdict(list)
for m, ev in by.items():
    for a, b in zip(ev, ev[1:]):
        k0 = a['virtual_sol_reserves'] * a['virtual_token_reserves']; k1 = b['virtual_sol_reserves'] * b['virtual_token_reserves']
        p0 = a['virtual_sol_reserves'] / a['virtual_token_reserves']; p1 = b['virtual_sol_reserves'] / b['virtual_token_reserves']
        # constant-product post price if the trade were plain x*y=k
        x, y = a['virtual_sol_reserves'], a['virtual_token_reserves']
        if b['is_buy']: xp, yp = x + b['sol_amount'], y - b['token_amount']
        else: xp, yp = x - b['sol_amount'], y + b['token_amount']
        jumps[(b['mayhem_mode'], fee(b) == 0)].append((k1 / k0, p1 / (xp / yp), b['virtual_sol_reserves'] - xp, b['virtual_token_reserves'] - yp))
for k, v in sorted(jumps.items()):
    r = sorted(x[1] for x in v); kk = sorted(x[0] for x in v); n = len(v)
    print(f'mayhem={k[0]} fee0={k[1]} n={n} k_ratio p10/50/90={kk[n//10]:.3f}/{kk[n//2]:.3f}/{kk[9*n//10]:.3f}  post_price/cp_price p10/50/90={r[n//10]:.3f}/{r[n//2]:.3f}/{r[9*n//10]:.3f}  dSOL_adj median={sorted(x[2] for x in v)[n//2]/1e9:.3f} dTOK_adj median={sorted(x[3] for x in v)[n//2]}')
