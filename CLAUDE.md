# Solana DEX Sniper — project rules for Claude Code

The single source of truth is `SOLANA_DEX_SNIPER_V4_FINAL.md` (place a copy in this repo root). Read the relevant sections before working on any stage.

The user writes in Indonesian. Reply in Indonesian; keep code, comments and gate reports in English.

## STAGE-GATE DEVELOPMENT — STRICT, NO SKIPPING

Stage order (§31):
0 Foundations & feed → 1 RADAR → 2 Safety/DEV → 3 Fast features & trader quality → 4 Dataset & labels → 5 Models (ML) → 6 Rust inference → 7 Execution → 8 Shadow → 9 Micro-live → 10 Scale-up

Rules:
1. Work on ONE stage at a time. Never write code for a later stage "in advance".
2. A stage is finished ONLY when every MUST criterion of its gate (§31) is measured on real data and PASSES. "Code compiles", "looks good" or "mostly works" is NOT a pass.
3. Every gate produces `gates/STAGE_<N>_<NAME>.md`: commit hash, data window, hardware, each criterion with its measured value and PASS/FAIL.
4. One FAIL = the stage FAILS. Fix the cause and re-run the WHOLE gate. Do not move on.
5. The RADAR (Stage 1) is the foundation. Do NOT start ML, models, CatBoost, sizing or execution until every Stage 1 sub-gate (1A–1H) PASSES. If the radar is not excellent, keep improving the radar.
6. Do not loosen a gate target to make it pass. A target can be loosened only with written proof in the gate report that no causal method can reach it, and only after the user agrees.
7. If a gate cannot be measured (missing data, plan limits, missing hardware), STOP and tell the user exactly what is missing. Never fake, estimate or skip a measurement.
8. Before starting a stage: list its deliverables and gate criteria and get the user's OK. At the end: show the gate report and ask the user before starting the next stage.
9. Previously passed gates are regression tests. A change that breaks an earlier gate must be fixed before continuing.

## HISTORICAL-FIRST MODE (active)
- Do NOT run 24h/72h live recordings now. Build and validate Stages 0–6 on HISTORICAL data (backfilled past slots for the §4 programs), replayed through the same decoder/replay code.
- Gate 0 uses the historical criteria (G0.H1–H3, G0.3, G0.4). Live-only criteria (G0.1, G0.2, G0.5, G1F live, G1G) move to the Live-Readiness Gate, which must pass before Stage 7 mainnet tx and Stage 8.
- Start small: backfill a few hours, measure Helius credit cost per hour (G0.H3), report it, then ask the user how much history to fetch. Stage 1H needs ≥ 4 weeks of history in the end.
- Stage-gate rules above still apply in full: the radar must pass 1A–1E and 1H on historical data before any later stage.

## Other rules
- Hot path = Rust only (§23.4, §32). Offline research, labels, training and reference implementations = Python under `python/`.
- Secrets come from `.env` only (§30). Never print, log, commit or hardcode keys. Never ask for or store a wallet private key; use `WALLET_KEYPAIR_PATH`.
- No live trading with real capital before Stage 9.
- Program IDs in §4 must be verified against official sources before use; record the source in config.
- No future leakage in any feature (§27).
- Historical backfill (Helius archival / getBlock over past slots) may be used to build data faster than live recording, but it must go through the SAME decoder and replay code as live.
- The first strategy backtest (rule-based entry + MEE exit) happens only after Stages 0–3 PASS. The ML backtest happens only in Stage 5.
