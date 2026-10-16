# Decoder validation — Phase 1 (pump.fun curve + PumpSwap)

- Branch `fix/phase1-decoder`, base commit `e75a8c5`.
- Data: fresh Helius backfill `data/probe`, slots 457793073–457793222 (150 confirmed blocks, 171,722 txs, 2026-10-16).
- Commands: `sniper backfill --out data/probe --slots 150 --concurrency 4`; `sniper pump-check --rec data/probe [--dump data/probe/dump.jsonl]`;
  `python3 python/audit/{curve_fee0,curve_mayhem,amm_continuity,amm_sell_pred}.py data/probe/dump.jsonl`.

## 1.1 — A1-1 "curve price wrong for fee_bps=0": root cause found, audit hypothesis REJECTED

| Measurement | Value |
|---|---|
| Curve events / with fee_bps = 0 | 1,315 / 337 (25.6%) |
| fee_bps = 0 events in `mayhem_mode = true` mints | 337 / 337 (100%) |
| Wallets producing the fee_bps = 0 events | 1 (`BwWK17cbHxwWBKZkUYvzxLcNQ1YVyaFezduWbtm2de6s`, the Mayhem-mode agent) |
| `_truncated` events | 0 |
| Next trade predicted from previous event's virtual reserves (mayhem mints) | 100% within 0.1% (n = 442) |
| k = x·y change at agent trades, p10 / p50 / p90 | 0.747 / 1.019 / 1.265 (normal trades: 1.000) |
| virtual − real SOL, non-mayhem curve events | exactly 30 SOL in 847 / 847 (100%) |

Conclusion: the decoder reads the right fields. In pump.fun Mayhem mode an agent wallet trades fee-free and the program rewrites
the virtual SOL reserve at each of its trades (virtual − real ranged 0.006–80 SOL while real SOL was ~0.2–0.5 SOL).
The following trades really execute against the new reserves, so the price series is the true on-chain state, but the
virtual-reserve price is not backed by real SOL: a holder cannot exit at it. The "10x" labels of the audit came from these mints.

Fix (scope rule, documented): `MarketEvent.mayhem`; the loader excludes every mint ever seen in Mayhem mode and logs the count.
Status: **PASS** for non-mayhem curve (reserve invariant 100%, state continuity 100%); mayhem mints = **out of scope / unsupported**.

## 1.2 — Default-valued fields (A1-3 / D07 / D08)
- Curve and PumpSwap required fields are parsed as `Option`; missing/mistyped/empty → event quarantined (`malformed_events` counter),
  never defaulted to 0 / "" / SELL. `_truncated` payloads rejected. Zero token amount / zero reserves rejected.
- Curve fees now include `cashback`, `buyback_fee`, `holder_rewards` (were ignored).
- Tests: `pump::tests::{curve_event_valid, missing_required_field_is_quarantined_not_defaulted, extra_fees_and_mayhem_flag}`.
- On real data: malformed 0 of 7,172 events. **PASS**.

## 1.3 — txview index shifting (D09)
- A non-string account key, out-of-range program/account index, or an inner instruction without `stackHeight` now rejects the
  transaction instead of silently dropping one item (which shifted every later index).
- Tests: `txview::tests::{valid_tx_with_loaded_address, malformed_key_or_index_rejects_tx}`.
- Real data: decoded events unchanged (7,172 → 7,172). **PASS**.

## 1.4 — Pool state per mint (A1-2)
- Loader keeps one AMM pool per coin (first seen, causal); events of a second pool of the same mint are excluded and counted.
- Probe: 375 AMM mints, 0 with a second pool. **PASS** (guard in place; re-measured on every load).

## 1.5 — Pool math, all four paths (A1-5 / D13)
New bug found and fixed: PumpSwap `virtual_quote_reserves` is a signed i128 and is often **negative**; the decoder clamped it to 0,
mispricing those pools by ~5%. Effective quote reserve = real + signed virtual (`effective_quote`, test
`amm_tests::negative_virtual_quote_is_subtracted_not_clamped`).

Next-trade prediction from the previous post-trade state of the same pool (`pump-check`, now 4 paths):

| Path | n | within 0.1% before | within 0.1% after |
|---|---|---|---|
| curve buy | 643 | 99.84% | 99.84% |
| curve sell | 526 | not tested | 100.00% |
| AMM buy | 3,207 | not tested | 99.78% |
| AMM sell | 2,275 | 83.60% | 99.65% |

Remaining misses: integer rounding on dust trades and state gaps after the 146 undecodable events (see New findings).
Effective-reserve continuity per instruction: buy 100%, buy_exact_quote_in 99.64%, buy_exact_quote_in_v2 99.01%, buy_v2 93.20%
(misses coincide with base-reserve gaps → missing events, not formula), sell 99.74%. **PASS** (target ≥ 99% per path).

Side/amount vs independent balance-delta reference (single-event txs, user = fee payer): curve 100% / 100% (n = 903),
PumpSwap 100% / 99.65% (n = 5,406).

## 1.6 — Fee bias in SOL flow (A2-2)
- New `pool_sol`: SOL into/out of the pricing reserve, protocol/creator fees excluded on both sides (curve: `sol_amount`;
  AMM buy: `quote_amount_in_with_lp_fee`; AMM sell: `quote_amount_out − lp_fee`). Flow features and the radar's average trade
  price use it; `sol` keeps the trader-side amount. Test `flow_tests::curve_pool_sol_is_symmetric`. **PASS**.

## Open / new findings
- 146 undecodable pump/PumpSwap events per 150 blocks (2.0%): event types not in the IDL or missing parent instruction. Cause not yet
  classified (logged in PROGRESS.md).
- `cargo fmt --check` fails on pre-existing code (rustfmt was not installed; repo never formatted). Not reformatted to keep the diff minimal.
