# V6 design — status (2026-10-11)

Branch `v6-components` = `v5-upgrade` + spec files + design documents. **No Rust/Python code has been changed yet.**

| Item | State |
|---|---|
| `01_FORENSIK_SISTEM_SNIPER.md`, `02_PREDIKSI_MOONSHOT_SNIPER.md`, `SOLANA_DEX_SNIPER_V4_FINAL.md` | copied to the repo root (the spec was missing, file 01 §14) |
| `design/D1-bugfixes.md` | done: Phase 1 bugs checked against the code (all 11 confirmed + 4 extra), fix design + tests |
| `design/D2-radar.md` | done: radar redesign (true newborn from creation, new/old paths, dust + clusters, CUSUM, WATCH, repeated decision rows) + 7 extra findings |
| D3 features, D4 safety + data, D5 clusters / wallet DB, D6 labels + ML, D7 strategy + eval | not done (design stopped by the user's request) |
| Phase 0 review of unreviewed code (file 01 §14) | not done |
| Merged contract `V6_IMPLEMENTATION_PLAN.md`, implementation, compile, tests | not done |

Nothing here has been compiled or tested. Build and tests are to be run on the Linux VPS (`cargo build --release`, `cargo test`, `scripts/ci.sh`).

Resume order: designs D3–D7 → one merged contract with file ownership → implement in waves → `cargo test` on the VPS.
