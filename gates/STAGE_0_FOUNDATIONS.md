# Gate 0 — Foundations & Feed

**Status: NOT PASSED (BLOCKED).** G0.1, G0.2 and G0.5 cannot be measured on mainnet LaserStream on the
current Helius Free plan. G0.3 and G0.4 PASS.

| Field | Value |
|---|---|
| Commit | `dfb7914` (code measured); report written afterwards |
| Hardware | VPS, AMD EPYC 7713 (15 vCPU), 49 GB RAM, 73 GB disk; Gatineau QC, CA; RTT to `laserstream-devnet-ewr` 9.7 ms |
| Helius plan | Free. Mainnet LaserStream gRPC: **rejected**. Mainnet WS: `slotSubscribe` + `logsSubscribe` only (`transactionSubscribe` "not available on the free plan", `blockSubscribe` "Method not found") |
| Client | Official `helius-laserstream` 0.10.0 (Rust), default features off (`cuckoo` pulls incompatible Solana crates) |
| Data windows | `data/g02_devnet` devnet gRPC 2026-10-09 23:41–00:26 UTC (45 min, 50 forced kills); `data/ws_mainnet_trial` mainnet WS 2026-10-09 23:44–23:49 UTC (5 min, 3 forced kills); `data/smoke1` devnet 90 s |

## Criteria

| ID | Criterion | Measured | Result |
|---|---|---|---|
| G0.1 | 24 h continuous **mainnet** run, 0 unrecovered slot gaps | Mainnet gRPC unavailable on the Free plan. A 24 h devnet gRPC run (`data/g01_devnet_24h`) is in progress as a code/stability check only; it does not count for G0.1. | **BLOCKED** |
| G0.2 | Forced disconnect ×50: 100% auto-recovery, 0 duplicates after dedup | Devnet gRPC: 50/50 kills recovered (`ss -K` on the live TCP socket); recovery 4.99–5.06 s (the SDK's fixed 5 s reconnect delay); 41,947 replayed duplicates removed; duplicate audit of derived output = 0; gap tracker 11,111 finalized slots checked, 0 gaps. Mainnet WS fallback: 3/3 recovered (max 0.83 s), 0 duplicates. Mainnet gRPC: not measurable. | **PASS on devnet; mainnet gRPC BLOCKED** |
| G0.3 | Same recording replayed twice → byte-identical derived output | 45 min recording (347,929 messages incl. 50 reconnects): replay #1 = replay #2 = live derived output, SHA-256 `634bded5…a4ed`. Also `smoke1`: `1dee6f70…0e8c` ×3. | **PASS** |
| G0.4 | CI log-grep finds 0 secrets; `.env` not tracked | `scripts/ci.sh`: CI PASS. Negative test (key planted in `logs/`) → CI FAIL as expected. `git ls-files` tracks only `.env.example`. | **PASS** |
| G0.5 | Slot-time → receive histogram per feed | Recorded for devnet gRPC (below). Mainnet gRPC: not available. Mainnet WS: recorded but no slot-time reference without the gRPC footer stream. Preconfirmation / Preprocessed feeds: not available on the plan. | **BLOCKED for mainnet** |

### G0.5 definitions and devnet values (`data/smoke1`, no forced kills)

- Slot start = `block_producer_time_nanos` of the parent block (BlockFooter). Diagnostics showed the footer timestamp is per block (parent→child 200–400 ms), and 29% of slots have a first transaction received *before* their own producer time. So the parent's timestamp is used as the start of the slot.
- gRPC tx receive − slot start: p50 336 ms, p75 403, p90 471, p95 495, p99 540, 0 negative (n = 10,899). This includes the in-slot execution position of each transaction.
- gRPC tx receive − server `created_at` (transit Helius→VPS): p50 8.3 ms, p90 10.1, p99 21.0.
- In the 45 min chaos run, p90+ rise to seconds because replayed messages after each reconnect arrive late. Clean values must come from runs without kills.

## Stage 0 deliverables

| Deliverable | Where |
|---|---|
| Cargo workspace | `crates/{config,latency,events,recorder,feed,replay,app}` |
| Env loader + redaction | `crates/config` (`${VAR}` expansion; `Settings` Debug hides keys); `crates/app/src/logging.rs` (every log line redacted; panic hook redacted) |
| ns clocks + latency histograms | `crates/latency` |
| LaserStream client + WS fallback | `crates/feed/src/grpc.rs` (processed; tx + all slot statuses + block meta + block footer), `ws.rs` (slot + logs per program, reconnect + `getSignaturesForAddress` backfill) |
| Program-filtered subscriptions | `config/venues.toml`, 11 venues; all IDs checked against official sources and as executable on mainnet (2026-10-09) |
| Reconnect with replay, gap detection + backfill | SDK replay from tracked slot; `crates/events/src/gap.rs` walks finalized-slot parent links and backfills missing blocks via `getBlock` |
| Raw recorder + replay | `crates/recorder` (independent zstd frames, crash-tolerant); `crates/replay` + `sniper replay` |
| Verification tools | `sniper verify-chain` (independent `getBlock` comparison, sample or every slot in each outage window), `sniper report`, `sniper verify-programs` |

## Independent completeness checks (`getBlock` re-scan)

| Recording | Mode | Slots | RPC txs | Missing | Extra |
|---|---|---|---|---|---|
| devnet smoke1 (all non-vote) | sample | 40 | 1,301 | 0 | 0 |
| mainnet WS trial (11 venues) | sample | 30 | 6,130 | 0 | 0 |
| mainnet WS trial (11 venues) | every slot in 3 outage windows | 44 | 8,213 | 0 | 0 |

## Known issues

1. **Plan limit (blocker).** Mainnet LaserStream (and Preprocessed / Preconfirmations) requires a paid Helius plan.
2. **SDK reconnect delay** is fixed at 5 s. Data is replayed (no loss), but that is 5 s of blindness per disconnect. Before live stages, consider a custom reconnect on `laserstream-core-client` with immediate retry. This must be measured, not assumed.
3. **Alpenglow cleared banks** (`EntryUpdateParent`) are only delivered with an entry subscription. That is not subscribed in Stage 0; reconciliation belongs to later stages.
4. **Devnet already carries transaction version 1.** `getBlock` needs `maxSupportedTransactionVersion: 1` (fixed).
5. **Mainnet WS volume** is 2.8 MB/s raw for the 11 venues (~21 GB/day on disk). The Free-plan credit cost per day is unknown.
6. **Program IDs.** The Moonshot SDK repo is archived and renamed Moonit, so the ID must be rechecked against the successor before Stage 1.

## Sign-off

Pending. Gate 0 cannot PASS until G0.1, G0.2 and G0.5 are measured on mainnet LaserStream.
