# PROGRESS — sniper_v4 fix & training plan

Plan: [SNIPER_V4_FORENSIC_AUDIT_AND_CLAUDE_CODE_PLAN.md](SNIPER_V4_FORENSIC_AUDIT_AND_CLAUDE_CODE_PLAN.md). Read this file first in a new session.

## Active phase
Phase 0 (baseline) + Phase 1.1 (curve price bug root cause).

## Done
- 2026-10-16 Phase 0 partial @ e75a8c5: `cargo build --release` OK (rustc stable, ~3 min); `cargo test --workspace` all pass (few tests);
  `cargo fmt --check` FAIL; `cargo clippy -D warnings` 1 finding. Helius key in `.env` (gitignored), RPC reachable.

## Next
- Probe backfill (150 slots) → measure time / size / credits per hour (G0.H3) before fetching more.
- Phase 1.1: dump raw fee_bps=0 curve TradeEvents from fresh blocks, determine correct reserve fields.

## Blockers
- Helius **free plan**: credits/rate limited — amount of history that can be fetched is bounded by the plan.

## New findings (ask user before acting)
- (none yet)
