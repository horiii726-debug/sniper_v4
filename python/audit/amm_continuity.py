"""Phase 1.5: PumpSwap reserve continuity. Event reserves are pre-trade; derive post-trade and compare
with the next event's pre-trade reserves in the same pool, per candidate formula."""
import json, sys, collections
rows = [json.loads(l)['raw'] for l in open(sys.argv[1])]
amm = [v for v in rows if 'pool_base_token_reserves' in v]
by = collections.defaultdict(list)
for v in amm: by[v['pool']].append(v)
kind = lambda v: ('buy:' + v.get('ix_name', '?')) if 'base_amount_out' in v else 'sell'
cands = {
  'buy q+=quote_amount_in_with_lp_fee': lambda v: v['quote_amount_in_with_lp_fee'],
  'buy q+=quote_amount_in': lambda v: v['quote_amount_in'],
  'buy q+=user_quote_amount_in': lambda v: v['user_quote_amount_in'],
  'buy q+=qin_lp+lp_fee': lambda v: v['quote_amount_in_with_lp_fee'] + v['lp_fee'],
  'buy q+=user_q - protocol - creator': lambda v: v['user_quote_amount_in'] - v['protocol_fee'] - v['coin_creator_fee'],
}
stat = collections.defaultdict(lambda: collections.Counter())
for p, ev in by.items():
    for a, b in zip(ev, ev[1:]):
        k = kind(a)
        if 'base_amount_out' in a:
            stat[k]['n'] += 1
            stat[k]['base_ok'] += (a['pool_base_token_reserves'] - a['base_amount_out'] == b['pool_base_token_reserves'])
            for name, f in cands.items():
                stat[k][name] += (a['pool_quote_token_reserves'] + f(a) == b['pool_quote_token_reserves'])
        else:
            stat[k]['n'] += 1
            stat[k]['base_ok'] += (a['pool_base_token_reserves'] + a['base_amount_in'] == b['pool_base_token_reserves'])
            stat[k]['sell q-=quote_out-lp_fee'] += (a['pool_quote_token_reserves'] - (a['quote_amount_out'] - a['lp_fee']) == b['pool_quote_token_reserves'])
            stat[k]['sell q-=quote_out'] += (a['pool_quote_token_reserves'] - a['quote_amount_out'] == b['pool_quote_token_reserves'])
for k, c in stat.items():
    n = c.pop('n'); print(k, 'n=', n, {x: f'{y/n*100:.1f}%' for x, y in c.items()})
print('virtual_quote_reserves nonzero:', sum(1 for v in amm if str(v.get('virtual_quote_reserves', '0')) not in ('0', '')), 'of', len(amm))
# Effective quote reserve = real + virtual (what the decoder prices with).
print('--- effective (real+virtual) continuity')
eff = lambda v: v['pool_quote_token_reserves'] + int(v.get('virtual_quote_reserves', 0) or 0)
st = collections.defaultdict(lambda: [0, 0, 0])
for p, ev in by.items():
    for a, b in zip(ev, ev[1:]):
        k = kind(a)
        d = a['quote_amount_in_with_lp_fee'] if 'base_amount_out' in a else -(a['quote_amount_out'] - a['lp_fee'])
        s = st[k]; s[0] += 1; s[1] += (eff(a) + d == eff(b))
        same_tx = a['timestamp'] == b['timestamp']
        s[2] += same_tx
for k, (n, ok, same) in st.items():
    print(f'{k}: n={n} exact={ok/n*100:.2f}%')
