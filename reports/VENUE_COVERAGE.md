# Venue coverage — Phase 1B.1

Data: `data/probe`, slots 457793073–457793222 (150 blocks, 2026-10-16). Command: `sniper pump-check --rec data/probe` (`venue_coverage`).
Successful transactions that invoke each registry program (top-level or CPI), and how many yielded a decoded swap event.

| Venue | Status | Successful txs | Txs with decoded event |
|---|---|---|---|
| pumpfun_curve | implemented + verified (Phase 1; Mayhem-mode mints unsupported) | 1,753 | 1,314 |
| pumpswap_amm | implemented + verified (Phase 1; non-SOL quote pools skipped: 2,987 events) | 8,509 | 5,854 |
| meteora_damm_v2 | unsupported | 1,050 | 0 |
| meteora_dlmm | unsupported | 423 | 0 |
| orca_whirlpool | unsupported | 373 | 0 |
| raydium_clmm | unsupported | 289 | 0 |
| raydium_cpmm | unsupported | 68 | 0 |
| raydium_amm_v4 | unsupported | 67 | 0 |
| raydium_launchlab | unsupported | 27 | 0 |
| meteora_dbc | unsupported | 20 | 0 |
| moonshot | unsupported | 0 | 0 |

Pump.fun + PumpSwap = 10,262 of 12,579 venue-touching successful txs (81.6%). The strategy covers **pump.fun curve + PumpSwap only**;
no claim of "all DEX" coverage. Unsupported venues are not in the trading universe. Share of memecoin (non SOL/USDC/USDT-major)
volume on the unsupported venues is not measured yet (needs their decoders, 1B.2).
