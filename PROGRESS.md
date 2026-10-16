# PROGRESS — sniper_v4 fix & training plan

Plan: [SNIPER_V4_FORENSIC_AUDIT_AND_CLAUDE_CODE_PLAN.md](SNIPER_V4_FORENSIC_AUDIT_AND_CLAUDE_CODE_PLAN.md). Read this file first in a new session.

## Active phase
Phase 0 (baseline) + Phase 1.1 (curve price bug root cause).

## Done
- 2026-10-16 Phase 0 partial @ e75a8c5: `cargo build --release` OK (rustc stable, ~3 min); `cargo test --workspace` all pass (few tests);
  `cargo fmt --check` FAIL; `cargo clippy -D warnings` 1 finding. Helius key in `.env` (gitignored), RPC reachable.

- Phase 1 (pump.fun/PumpSwap) 1.1–1.7 PASS on fresh probe data — see reports/DECODER_VALIDATION.md, PRICE_AND_RESERVE_VALIDATION.md.
  A1-1 root cause = Mayhem mode (agent rewrites virtual SOL), not a decoder field bug → mayhem mints excluded.
  New bug fixed: PumpSwap signed virtual quote clamped to 0 (AMM sell math 83.6% → 99.65%).
- Probe backfill cost: 150 slots in 32 s at concurrency 4 (≈1.9× real time), 36 MB disk; 3 h continuous backfill
  `data/live3h` (27,000 slots, concurrency 6, ~5.1 slots/s) started 2026-10-16 21:53 UTC.

## Next
- Phase 1B (other venues) → Phase 2 (data foundation) → Phase 3 → 4 → 5 → 6, in plan order.

## Blockers
- Helius **free plan**: credits/rate limited — amount of history that can be fetched is bounded by the plan.

## New findings (ask user before acting)
- 146 undecodable pump/PumpSwap events per 150 blocks (2%): not classified yet.
- `cargo fmt --check` fails on pre-existing code (never formatted); not reformatted.
