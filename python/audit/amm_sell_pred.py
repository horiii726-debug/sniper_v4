"""Phase 1.5: replicate pump-check's PumpSwap sell prediction (previous event's derived post-state)."""
import json, sys, collections
rows = [json.loads(l)['raw'] for l in open(sys.argv[1])]
a = [v for v in rows if 'pool_base_token_reserves' in v]
eff = lambda v: v['pool_quote_token_reserves'] + int(v.get('virtual_quote_reserves', 0) or 0)
def post(v):
    if 'base_amount_out' in v: return eff(v) + v['quote_amount_in_with_lp_fee'], v['pool_base_token_reserves'] - v['base_amount_out']
    return eff(v) - (v['quote_amount_out'] - v['lp_fee']), v['pool_base_token_reserves'] + v['base_amount_in']
last = {}; c = collections.Counter(); bad = collections.Counter()
for v in a:
    p = v['pool']
    if p in last and 'base_amount_in' in v:
        q, t = post(last[p]); tok = v['base_amount_in']
        fb = v['lp_fee_basis_points'] + v['protocol_fee_basis_points'] + v['coin_creator_fee_basis_points']
        g = q * tok // (t + tok); pred = g - (g * fb + 9999) // 10000; act = v['user_quote_amount_out']
        e = abs(pred - act) / max(act, 1); c['n'] += 1; c['ok'] += e <= 1e-3
        if e > 1e-3:
            bad[('state_continuous' if (q, t) == (eff(v), v['pool_base_token_reserves']) else 'state_gap', 'buyback' if v.get('buyback_fee', 0) else 'plain', round(e, 2))] += 1
    last[p] = v
print(c, f"{c['ok']/c['n']*100:.2f}%"); print(bad.most_common(10))
