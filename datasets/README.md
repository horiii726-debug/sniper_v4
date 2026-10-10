# Versioned datasets (decoded Helius mainnet data)

Only compressed files are versioned (`.gitignore` excludes `*.csv` and `*.bin` here). Raw `getBlock` JSON is not
kept (≈ 3.5 GB per 1 h window); every pump.fun curve / PumpSwap trade in it is in the decoded files.

| Dir | Format | Content |
|---|---|---|
| `bf_1h`, `h1d` | V4 | 9,000-slot windows (~33 min): `pump_events.bin.zst` (V4 cache, no wallet table), `trades.csv.zst`, `ml_snapshots.csv.zst` (V4 features/labels) |
| `v5_<window>` | V5 | 16,500-slot windows (~1 h) from `config/sample_windows.json`: `pump_events_v2.bin.zst` (window events + wallet table), `follow.bin.zst` (10 h follow-up of every HOT coin), `trades.csv.zst` (window + follow-up, one row per trade, wallet addresses), JSON summaries (backfill, G0.H1 verify, follow, snapshots) |

Files over 95 MB are split as `NAME.part00`, `NAME.part01`, … Restore with `cat NAME.part* > NAME`.

Use a V5 dir with the CLI: create `data/<dir>/`, restore `pump_events_v2.bin` (`zstd -d`) and `follow.bin.zst`
(keep compressed) there, then e.g. `sniper snapshots --rec data/<dir>`.
