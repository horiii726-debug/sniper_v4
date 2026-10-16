# SNIPER V4 — Forensic Audit + Fix & Training Plan for Claude Code

> **Satu file, tiga bagian:**
> **Bagian A** — hasil forensik yang sudah diverifikasi (kode + data nyata).
> **Bagian B** — PLAN kerja untuk Claude Code: apa yang diperbaiki, urutannya, apa yang di-training, dan kriteria lulus.
> **Lampiran** — dokumen spesifikasi asli dari user (`SOLANA_SNIPER_V4_FULL_AUDIT_AND_FIX_PLAN_US_EN.md`), utuh, sebagai referensi detail.
>
> Jika Bagian A/B bertentangan dengan Lampiran, **Bagian A/B menang** karena sudah dicek terhadap kode dan data (contoh: klaim D11 "decimals 0 dibuang" ternyata salah baca — lihat A.3).

**Repo:** `https://github.com/horiii726-debug/sniper_v4` @ `e75a8c53745050e9ce447226dcba722307314209` (2026-10-10, 13 commit)
**Audit:** 2026-10-16, read-only. Tidak ada kode repo yang diubah.
**Data diuji:** `datasets/h1d` (slot 454682050–454690299, 37 menit) dan `datasets/bf_1h` (slot 455069845–455078844, 33 menit): 1.012.270 swap, 137.588 snapshot ML.
**Tidak bisa dijalankan saat audit:** `cargo build/test/clippy` (Rust toolchain tidak ada di mesin audit) → status **BLOCKED**, wajib dijalankan di Fase 0.

---

## Instruksi wajib untuk Claude Code (baca dulu)

1. Kerjakan di checkout repo `sniper_v4`. Ikuti `CLAUDE.md` repo (stage-gate, balas user dalam Bahasa Indonesia, kode/komentar/gate report dalam Bahasa Inggris).
2. **Urutan tidak boleh dilompati:** Fase 0 → 1 → 2 → 3 → 4 → 5 → 6. Jangan training model sebelum Fase 1–4 lulus. Data harga yang salah membuat semua metrik ML tidak berarti (lihat A.1).
3. Setiap perbaikan = **tes regresi dulu (gagal), lalu fix (lulus)**. Perubahan minimal, jangan refactor yang tidak diminta (skill `karpathy-guidelines`).
4. Jangan longgarkan threshold untuk menambah jumlah entry. Jangan klaim "profitable/akurat/live-ready" tanpa artefak yang bisa direproduksi.
5. **Tidak ada transaksi mainnet.** Jangan baca/print/ubah private key. Kredensial hanya dari `.env`.
6. Setiap fase menghasilkan laporan di `reports/` atau `gates/` berisi: commit SHA, window data, perintah persis, angka sebelum/sesudah, PASS/FAIL/BLOCKED.
7. Di akhir tiap fase: tunjukkan hasil ke user dan minta OK sebelum lanjut fase berikutnya.
8. **Hemat token:** baca file ini **sekali** di awal. Jangan baca ulang seluruhnya; buka hanya bagian fase yang sedang dikerjakan. Lampiran dibaca hanya bila butuh detail satu ID temuan.
9. **Jangan keluar jalur:** kerjakan hanya tugas di Bagian B, berurutan. Tidak boleh menambah fitur, model, refactor, atau indikator di luar plan. Bila menemukan masalah baru, catat di `PROGRESS.md` bagian "Temuan baru" dan tanya user dulu — jangan langsung dikerjakan.
10. **Memori lintas sesi:** buat dan perbarui `PROGRESS.md` di root repo (fase aktif, tugas selesai + commit SHA, tugas berikutnya, blocker). Di awal sesi baru, baca `PROGRESS.md` dulu, bukan file ini dari awal.
11. **Fase 1.1 belum pasti penyebabnya:** buktikan dulu dengan data blok mentah (`pump-check --dump`) sebelum mengubah decoder. Jika hipotesis salah, laporkan ke user, jangan menebak.

### Skill yang dipakai (sudah terpasang di sesi audit; pasang juga di repo `.claude/skills/` bila perlu)
| Kapan | Skill |
|---|---|
| Menyusun/menjalankan plan per fase | `writing-plans`, `executing-plans`, `skeptic` (dari maccydee/claude-planning-skills) |
| Setiap edit kode | `karpathy-guidelines` (perubahan minimal + verifikasi) |
| Peta dampak perubahan (decoder → features → labels → sim) | `graphify` (`graphify update .` lalu `graphify query/path`) |
| Label, leakage, split, validasi | ml4t: `lookahead-bias`, `data-leakage`, `point-in-time`, `triple-barrier`, `horizon-design`, `model-validation`, `backtest-overfitting`, `transaction-costs`, `regime-awareness`, `feature-selection` |
| Cek rumus pool math / Kelly / Hawkes / HMM | math agents: `math-numeric-verifier`, `math-symbol-runner`, `math-counterexample-hunter` |

---

# BAGIAN A — Hasil Forensik

## A.0 Ringkasan 10 detik

| # | Masalah | Bukti | Dampak |
|---|---|---|---|
| 1 | **Harga pump.fun curve salah untuk event `fee_bps=0`** (16–29% event curve) | reserve virtual tidak konsisten dengan trade; error harga median −65%, p99 +740…+942% | **82–99.7% label 10x** dan **57–89% label 5x** berasal dari bug ini |
| 2 | **Label 30 menit tersensor** | hanya 5.4% / 16.6% snapshot punya horizon 30 m penuh | target utama (`y_peak2`) bias negatif; "rug" = harga terakhir yang terekam |
| 3 | **Gerbang buy/sell ratio terbalik** | rasio>4 ditolak, sell=0 lolos; yang ditolak justru lebih sering naik | membuang kandidat lebih baik |
| 4 | **Buy pressure non-linear** | peluang 2x berbentuk U-terbalik terhadap flow imbalance | aturan "net flow makin besar makin bagus" salah bentuk |
| 5 | **Sticky-HOT tidak pernah expire** | sampai 1.106 snapshot/coin; 18–27% snapshot >20 m setelah HOT | dataset didominasi coin ramai, sampel efektif kecil |
| 6 | **Validasi melihat masa depan** | leave-one-window-out + tuning di window setelah test | angka OOS di `reports/` tidak valid sebagai bukti live |
| 7 | **N terlalu kecil untuk 5x–100x** | setelah bersih: 10x ≈ 0–4 coin/window | target 5x/10x/100x = **INSUFFICIENT N** |
| 8 | **Belum ada hold yang untung setelah biaya; model belum terbukti membantu** | semua hold 30 s–30 m & TP/SL negatif; model naikkan win rate tapi rata-rata tidak lebih baik (A.6) | target harus return bersih, bukan "akan naik" |
| 9 | **Hanya pump.fun/PumpSwap & tidak tahu umur token** | 11 venue di config, 2 yang ter-decode; window 35 m | token lama yang "bangun" & token di DEX lain tidak terdeteksi (Fase 1B, 3B) |

## A.1 Data harga & decoding

### A1-1 · BUG KRITIS · Reserve curve salah untuk event `fee_bps=0`
- **Kode:** [crates/decoder/src/pump.rs:93-115](crates/decoder/src/pump.rs) hanya membaca `virtual_sol_reserves`, `fee`, `creator_fee`. `TradeEvent` di `config/idl/pump.json` punya field yang **diabaikan**: `ix_name`, `mayhem_mode`, `cashback_fee_basis_points`, `cashback`, `buyback_fee_basis_points`, `buyback_fee`, `shareholders`, `quote_mint`, `quote_amount`, `virtual_quote_reserves`, `real_quote_reserves`, `holder_rewards_bps`, `holder_rewards`, `creator_fee_unclaimed`. IDL juga punya instruksi `set_mayhem_virtual_params`, `set_virtual_quote_reserves`, `toggle_mayhem_mode`, `buy_v2/v3`, `sell_v2/v3`, `buy_exact_quote_in_v2/v3`.
- **Bukti data** (`scripts_audit/dig_curve.py`, `impact_fee0.py`):
  - Event normal: `virtual_sol − real_sol = 30.000 SOL` dan `Δvirtual_quote / sol = 0.9877` (fee 1.25%) → konsisten.
  - Event `fee_bps=0`: `Δreal_quote / sol = 1.000` (SOL trade benar), tetapi `Δvirtual_quote / Δreal_quote` median **25.6×** (p90 213×); `virtual − real` median 9.6 SOL; k = q·t berubah median **6.8×** dalam satu mint (normal 1.0×). Contoh `659KLfpj6sG8zP3MNUEVsTqxzXL2RxizoG3JKfhppump`: jual 0.005 SOL → virtual SOL turun 30.06 → 27.51.
  - Porsi: 29.2% event curve (bf_1h, 368 mint), 16.2% (h1d, 318 mint); 407 mint mencampur event fee>0 dan fee=0.
  - Harga eksekusi vs harga marginal pre-trade untuk buy curve fee=0: p1 = 0.73 (mustahil pada constant-product → model reserve salah).
- **Dampak:** price path, label, `ret_*`, `price_vs_*`, fill & mark-to-market simulasi salah.
  | label (bf_1h / h1d) | mint fee=0 | mint lain |
  |---|---|---|
  | y_peak10 | 3.0% / 2.7% | **0.0% / 0.06%** |
  | y_peak5 | 8.5% / 7.4% | 0.24% / 0.53% |
  | y_rug | 51.8% / 50.7% | 3.9% / 3.0% |
  | y_dump30s | 73% / 72% | 5.3% / 6.5% |
- **Hipotesis penyebab (wajib dikonfirmasi di Fase 1):** coin mode mayhem / varian kurva dengan virtual quote dinamis, atau event versi lain yang terdecode sebagai prefix (`_truncated`) sehingga fee=0. Harga yang benar kemungkinan dari `virtual_quote_reserves`/`real_quote_reserves` atau parameter mayhem, bukan `virtual_sol_reserves`.
- Selain harga: fee `cashback`/`buyback_fee`/`holder_rewards` tidak dihitung → SOL yang dibayar buyer kurang dari sebenarnya.

### A1-2 · BUG · Pool disimpan per mint, bukan per pool
- [backtest.rs:439-451](crates/app/src/backtest.rs), [ml_sim.rs:201-213](crates/app/src/ml_sim.rs): `Ev` tidak membawa alamat pool, event AMM dari pool mana pun menimpa state.
- Data: kontinuitas token-reserve PumpSwap 99.5–99.6% exact; 11 mint/window punya lompatan >5% (multi-pool atau event hilang).

### A1-3 · BUG · Default 0 / false / "" (D05, D07, D08, D09)
- [pump.rs:55-61](crates/decoder/src/pump.rs) `u()`→0, `s()`→""; [pump.rs:96](crates/decoder/src/pump.rs) `is_buy` hilang → **SELL**; `_truncated` dari [idl.rs:96-111](crates/decoder/src/idl.rs) tidak dicek.
- [txview.rs:26-38](crates/decoder/src/txview.rs) `filter_map` bisa menggeser indeks akun; `stackHeight` hilang → semua inner = stack 2 → parent salah.
- [normalize.rs:118-128](crates/events/src/normalize.rs) slot/parent/root/sub_id default 0.
- [backtest.rs:71](crates/app/src/backtest.rs), [radar_eval.rs:44](crates/app/src/radar_eval.rs) `blockTime`→0 ⇒ `window.0=0` ⇒ `newborn_after=60` ⇒ **semua coin newborn → auto-HOT**. Di 2 dataset ini tidak terpicu (0 kasus) tapi tanpa guard.

### A1-4 · BUG · Cache & provenance
- [backtest.rs:51-54](crates/app/src/backtest.rs) cache `pump_events_v2.bin` tanpa hash IDL/decoder/sumber → **fix decoder tidak berefek sampai cache dihapus**.
- Header `datasets/*/trades.csv` (`trader_id,creator_id`, tanpa `order`) ≠ writer di [main.rs:482](crates/app/src/main.rs) → dataset dibuat versi kode lain.
- `datasets/h1d/verify_backfill.json` 0 byte → G0.H1 untuk h1d tidak tercatat.
- `reports/portfolio_v4_*` berisi 4 window, repo hanya punya 2 → 2 window tidak bisa direproduksi.

### A1-5 · GAP · Validasi pool math cuma 2 dari 4 jalur
- [pump_check.rs:64-71](crates/app/src/pump_check.rs): hanya curve-buy dan AMM-sell. Curve-sell dan AMM-buy (exact-in vs exact-out) tidak diuji. Perbandingan side/amount hanya tx 1-event dengan `user == fee_payer`. Angka README "99.96% / 98.8%" tidak menangkap A1-1 karena tidak ada cek invariant `virtual − real`.

### A1-6 · BUG · Jual di kurva `complete` (migrasi)
- `value_of` / `pool_value` ([backtest.rs:318-327](crates/app/src/backtest.rs), [ml_sim.rs:335-345](crates/app/src/ml_sim.rs)) tidak menolak `complete && !amm` → exit Inactivity bisa terisi di harga yang tidak bisa dieksekusi.

### A1-7 · DESAIN · Definisi harga bercampur (D14)
- Radar `lift` memakai harga eksekusi (`sol/tokens`, termasuk fee+slippage); fitur & label memakai harga marginal pool. Data: buy eksekusi +1.0–1.5%, sell −1.0–1.5% dari marginal → spread ~2.5% masuk ke rule lift.
- `price_vs_vwap60` **bukan VWAP** (rata-rata harga per event, [ml_sim.rs:455-459](crates/app/src/ml_sim.rs)).
- Harga dalam lamport/raw token; desimal token tidak disimpan → salah untuk pool PumpSwap token non-6-desimal.

## A.2 Buy pressure & data trader

### A2-1 · BUG · Gerbang rasio buy/sell tidak monoton
- [radar/src/lib.rs:208-213](crates/radar/src/lib.rs), [backtest.rs:571-578](crates/app/src/backtest.rs): `bs/ss > 4` ditolak, `ss == 0` lolos.
- Data (6.7% snapshot sell=0; 6.7–6.8% rasio>4): y_peak2 sell=0 = **5.7–6.3%** (lolos) vs rasio>4 = **7.5–11.5%** (ditolak). y_cont: 9–12% vs 13–15%.

### A2-2 · BUG · Bias fee di arus SOL
- Buy `sol` termasuk fee, sell `sol` setelah fee ([pump.rs:103](crates/decoder/src/pump.rs), [pump.rs:157-162](crates/decoder/src/pump.rs)). Net flow bias ±1.0–1.1% volume (≈963 SOL dari 93.620 SOL di bf_1h).

### A2-3 · DESAIN · Buy pressure non-linear
- y_peak2 per desil `flow_imb_60s` (bf_1h): 3.4, 6.3, 13.5, 14.6, 12.4, 8.8, 8.2, 6.6, 8.0, 6.2% → **U-terbalik** (h1d mirip). Desil `buy_sol_60s` datar di bf_1h, naik di h1d → tidak stabil antar window.
- Ablation (A.5): fitur buy-pressure ternormalisasi likuiditas **tidak menambah** PR-AUC di atas fitur yang ada; BP saja memberi sinyal parsial (cont5 PR-AUC 0.21 vs base 0.10). Kesimpulan: buy pressure berguna sebagai bagian model non-linear, **bukan** sebagai gerbang ambang tunggal.

### A2-4 · BUG · Spam dust mencemari hitungan
- 52–54% event < 0.01 SOL. Satu mint (`F4aY8X7qXpMGf64FFYiMJWJMv8HuSLMv6bgNw7fepump`) = 121.353 buy 10-lamport dari 79 wallet (32% AMM buy di h1d). `REAL_MIN_SOL` hanya dipakai sebagian fitur; `n_trades_*`, `f8_*` (Hawkes/arrival), holders, `buy_sell_count_ratio_60s` tetap tercemar.

### A2-5 · BUG · Fitur artefak posisi waktu (T01)
- `new_wallet_share_60s` = 0.72–0.78 di 2 menit pertama window vs 0.16 setelah 15 menit ([ml_sim.rs:437](crates/app/src/ml_sim.rs)). Sama untuk `newborn` (0.09→0.43), `age_s`, `mult_since_first`, `f1_trades_since_first`, `secs_since_hot`.

### A2-6 · DESAIN · Holder dari swap saja (T02)
- `f6_holders`, `f2_top10_holder_share`, `f2_early20_supply_share`, `dev_hold_share` dari saldo swap mulai 0 → proxy, bukan holder sebenarnya.

### A2-7 · BUG · Dev sell (T03)
- `dev_sell_frac = 1.0` bila saldo creator yang terlihat ≤ 0 ([ml_sim.rs:277-281](crates/app/src/ml_sim.rs)) → coin yang dibuat sebelum rekaman: jual kecil apa pun = "dump 100%" → kill-switch.
- `creator_sold` identik `dev_ever_sold` di 100% baris (fitur duplikat).

## A.3 Radar, snapshot, funnel, ML

| ID | Status | Inti | Bukti |
|---|---|---|---|
| R01 sticky-HOT | **BUG + data** | [ml_sim.rs:222](crates/app/src/ml_sim.rs) set `last_event` lalu [ml_sim.rs:325](crates/app/src/ml_sim.rs) cek selisih = 0; `Demote` dibuang [ml_sim.rs:197-198](crates/app/src/ml_sim.rs) | snapshot/coin p99 462–649, max 902–1.106; 428+711 snapshot pada coin tanpa real buy >300 s |
| R02 score hilang | **BUG** | [ml_sim.rs:1029](crates/app/src/ml_sim.rs) `continue` sebelum `dg.snapshots += 1` | funnel tidak bisa menjelaskan sedikitnya entry |
| M06 read_scores | **BUG** | [ml_sim.rs:786-798](crates/app/src/ml_sim.rs) dump/devsell hilang → 0.0 (fail-open), baris <6 kolom dilewati, key duplikat menimpa | — |
| M07 config | **BUG (statis)** | `config/ml_strategy_params.json` hanya 14 field; `MlStrategy` punya 24 field tanpa `#[serde(default)]` → `ml-sim/ml-diagnose --params` dengan file ini akan gagal "missing field" | perlu dikonfirmasi `cargo run` di Fase 0 |
| Liquidity gate | **DESAIN** | `min_quote_res_sol` dibanding **virtual** SOL kurva (mulai 30) → selalu lolos di curve | — |
| `radar.sweep` | **PERF** | [radar/src/lib.rs:133-142](crates/radar/src/lib.rs) iterasi semua coin tiap detik | O(N coin) di hot path |
| R05 newborn | **DESAIN** | newborn = pertama terlihat setelah warm-up & auto-HOT → recall 98–100% sirkular | — |
| M01 horizon | **BUG + data** | [ml_sim.rs:617-673](crates/app/src/ml_sim.rs) | 30 m penuh: 5.4% / 16.6%; y_peak2 sisa<5m 4.0–7.1% vs penuh 15.1–16.2% |
| `y_rugdead` | **BUG** | [nested_lowo.py:16](python/ml/nested_lowo.py) `end30 ≤ 0.2` = harga terakhir yang terekam | bukan "mati & tidak pulih" |
| M02 nama target | **BUG** | `p_up2` dilatih dari `y_peak2`; `p_up5=min(p_up5,p_up2)` | — |
| M03 validasi | **BUG** | `nested_lowo.py` & `BacktestLowo` ([main.rs:453-477](crates/app/src/main.rs)) training/tuning di window setelah test | bukan walk-forward |
| M09 fitur | **BUG** | fitur = semua kolom kecuali META → artefak A2-5 ikut masuk | — |
| M04 N kecil | **data** | positif unik y_peak5 98/73 coin, y_peak10 35/23 coin; 99.7%/81.7% positif 10x dari mint korup | INSUFFICIENT N |
| M08 nesting | OK di data | 0 pelanggaran `peak5>peak2`, `up5>up2` | tetap laporkan raw vs koreksi |

**Verifikasi klaim spesifikasi asli (Lampiran):**
- D01–D10, D12–D15, T01–T04, R01–R02, R05, M01–M09: **terkonfirmasi** (lihat tabel di atas / A.1–A.2).
- D03 (bit-packing `order`): risiko nyata di kode, **tidak terjadi** di data (0 key duplikat; maks event per tx/mint/wallet ≈ 13 < 256).
- D11 poin "`retain` membuang mint decimals 0": **SALAH BACA** — di [balance_delta.rs:118](crates/decoder/src/balance_delta.rs) `retain(|_, (d, _)| *d != 0)` membuang mint dengan **delta 0**, bukan decimals 0. Poin D11 lain (default 0 untuk balance/fee/amount, owner == fee payer) tetap benar.
- D15 (tokens==0 → harga 0): ada di kode, **0 kasus** di data.
- R03, R04, R06, T05, M05, M10: gap/desain, belum ada instrumen → dikerjakan di Fase 3–5.

## A.4 Keamanan
- **OK:** 13 commit dipindai, tidak ada `.env`, keypair, atau API key ter-commit. `.gitignore` menutup `.env*`, `*keypair*`, `id.json`. `Settings` punya `Debug` manual + redactor.
- **Catatan:** redactor hanya mengenali nilai di `secrets()` + `api-key=`; `kill_connections` memakai `ss -K` (root) — jangan jalankan recorder sebagai root di mesin yang memegang wallet. `sol_usd_at` memanggil `curl` (input integer, bukan injeksi). Tidak ada jalur kirim transaksi mainnet.

## A.5 Riset out-of-sample (dikerjakan saat audit, di luar repo)

**Setup:** train = h1d (lebih awal), test = bf_1h (~1,8 hari kemudian). Mint ber-harga korup (A1-1) dikeluarkan (318 train / 411 test). Label dihitung ulang dari `trades.csv` dengan horizon **lengkap saja**: `cont5` (+30% sebelum −25% dalam 5 m), `peak2_15` (≥2x dalam 15 m), `rug5` (≤0.2× dalam 5 m), `dump30` (≤0.8× dalam 30 s). CI = bootstrap per mint (90%). Fitur CLEAN = fitur repo tanpa 12 fitur artefak; BP = 9 fitur buy-pressure baru (flow/likuiditas, FI accel, breadth, absorption, ex-whale); HMM = 4 state Gaussian, filter **forward-only** (kausal).

| Target (test pos / mint) | base | Markov state | CatBoost ALL | CatBoost CLEAN | +BP | +BP+HMM | LightGBM | RF | Logistic | Kernel RBF |
|---|---|---|---|---|---|---|---|---|---|---|
| cont5 (3.771 / 127) | 0.098 | 0.162 | 0.303 | 0.309 | 0.301 | 0.312 | 0.315 | **0.349** | 0.341 | 0.340 |
| peak2_15 (909 / **36**) | 0.039 | 0.068 | 0.123 | 0.119 | 0.118 | 0.118 | 0.109 | 0.121 | 0.122 | 0.129 |
| rug5 (1.765 / **21**) | 0.046 | 0.136 | **0.425** | 0.349 | 0.345 | 0.352 | 0.349 | 0.397 | 0.327 | 0.345 |
| dump30 (2.404 / 174) | 0.053 | 0.133 | 0.381 | 0.388 | 0.375 | 0.377 | 0.386 | **0.402** | 0.325 | 0.317 |

(angka = PR-AUC test. ROC-AUC 0.82–0.92. CI90 PR-AUC lebar, mis. cont5 RF [0.29, 0.41], rug5 RF [0.24, 0.56].)

**Kesimpulan riset:**
1. Ada sinyal nyata OOS untuk horizon pendek (cont5, dump30): lift 3–7× di atas base rate, precision@top-1% 0.42–0.56.
2. **CatBoost tidak lebih unggul** dari RF / LightGBM / logistic / kernel — semua dalam CI yang sama. Pilih model paling sederhana yang lulus kalibrasi.
3. **HMM dan fitur buy-pressure baru tidak menambah** nilai OOS (selisih di dalam noise). Jangan masukkan ke produksi tanpa ablation yang menang di ≥3 window.
4. `rug5` dengan fitur ALL (berisi artefak) > CLEAN (0.425 vs 0.349) → artefak posisi-waktu ikut "membantu" = tanda leakage struktural, bukan sinyal pasar.
5. Fitur teratas semua model: `quote_res_sol` & `fee_bps` (20–28% importance) → model sebagian besar belajar **tipe pool/venue** (curve vs AMM, tier fee). Harus diuji per-venue.
6. Linear algebra: condition number 159, 20 dari 60 komponen menjelaskan 90% varians → banyak fitur redundan; pruning aman.
7. peak2_15 hanya 36 mint positif, rug5 hanya 21 mint → **INSUFFICIENT N** untuk klaim; 5x/10x/100x tidak bisa diuji sama sekali dengan data ini.
8. Brier logistic/kernel buruk (0.13–0.15) karena `class_weight=balanced` → probabilitas wajib dikalibrasi sebelum dipakai untuk sizing/Kelly.

## A.6 Durasi hold & apakah kandidat model membantu (uji saat audit)

**Setup:** test = bf_1h, model = Random Forest `cont5` (train h1d, fitur CLEAN). Satu entry per mint (snapshot pertama yang lolos), masuk di harga ≥ t+1 s, biaya = fee pool tiap sisi + 0.5% (impact + network). Hanya entry dengan horizon penuh yang dihitung. Skrip: `hold_and_candidates.py`, `tp_exit.py`.

**Hold waktu tetap — rata-rata return bersih (CI90):**
| Hold | radar-only (1.174 mint) | model top 5% (78 mint) | model bawah 50% (763 mint) |
|---|---|---|---|
| 30 s | −1.8% [−3.4, +0.1] | −5.4% [−13.4, +1.7], win 38% | −2.8% |
| 1 m | −2.0% | −11.2% | −2.7% |
| 3 m | −4.4% | −22.8% | −3.2% |
| 5 m | −4.7% | −26.6% | −3.3% |
| 10 m | −5.7% | −35.6% | −3.5% |
| 15 m | −5.1% | −36.4% | −3.4% |
| 20 m | −5.6% | −51.1% | −3.8% |
| 30 m | −12.8% (n=194) | −58.1% (n=15) | −7.0% |

**Exit take-profit / stop (TP/SL, maks hold H):**
- radar-only: semua kombinasi negatif (−2.3% s/d −4.1%), win 11–19%.
- model top 5%: terbaik TP 1.3× / SL 0.8× dalam 5 m = −3.3% [−8.3, +3.0], win 45%; TP 1.5× / SL 0.7× dalam 20 m = −3.5% [−15.5, +9.5]. TP 3× tanpa stop = −20% s/d −40%.

**Kesimpulan:**
1. **Belum ada durasi hold yang untung setelah biaya** di data ini. Hold makin lama makin rugi; yang paling kecil ruginya adalah hold sangat pendek (≤30 s–1 m) atau TP kecil (1.3×) dengan stop ketat dalam ≤5 m.
2. **Model belum terbukti membantu profit.** Model menaikkan peluang menyentuh 2x (17–25% vs 2–3%) dan win rate dengan TP kecil (40–45% vs 14–19%), tapi juga memilih coin yang jatuh jauh lebih dalam → rata-rata return tidak lebih baik dari radar-only (CI melewati 0). Artinya model saat ini mendeteksi **volatilitas**, bukan **edge**. Vonis: `TIDAK TERBUKTI`.
3. Keterbatasan: 1 window test (33 m), harga marginal (belum quote jual executable per ukuran), latency 1 s, mint ber-harga korup dibuang. Hold 1 j–24 j **tidak bisa diuji** (window terlalu pendek) → G9.
4. Konsekuensi untuk plan: target model berikutnya harus memprediksi **return bersih / MFE sebelum MAE** (triple-barrier dengan biaya), bukan hanya "akan naik", dan dievaluasi dengan Fase 6B. Angka di atas jadi baseline yang harus dikalahkan.

Skrip reproduksi (taruh di `python/audit/` saat Fase 0): `forensic_trades.py`, `dig_price.py`, `dig_curve.py`, `impact_fee0.py`, `forensic_snaps.py`, `research_oos.py`, `summarize_oos.py`, `hold_and_candidates.py`, `tp_exit.py`. Output: `forensic_trades.json`, `impact_fee0.json`, `forensic_snaps.json`, `research_oos.json`, `hold_and_candidates.json`, `tp_exit.txt`.

---

# BAGIAN B — PLAN untuk Claude Code

Format tiap tugas: **Masalah → Penyebab → Perbaikan → Tes (tulis dulu) → Lulus bila**.

## Fase 0 — Baseline & reproduksi (tanpa ubah logika)
1. Catat `git rev-parse HEAD`, `rustc -V`, hash `config/idl/*.json`, `config/*.json`.
2. Jalankan `cargo fmt --check`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`. Catat hasil apa adanya.
3. Konfirmasi M07: `sniper ml-sim --params config/ml_strategy_params.json ...` → harus gagal "missing field". Catat.
4. Salin skrip audit ke `python/audit/`, jalankan ulang pada `datasets/`, pastikan angka A.1–A.5 tereproduksi.
5. `graphify update .` → simpan `graphify-out/GRAPH_REPORT.md` untuk peta dampak.
- **Output:** `reports/BASELINE_2026xxxx.md`. **Lulus bila** semua angka kunci A.0 tereproduksi ±1%.

## Fase 1 — Decoder & harga (P0, paling penting)
| # | Masalah | Perbaikan | Tes | Lulus bila |
|---|---|---|---|---|
| 1.1 | A1-1 harga curve salah (fee=0) | Ambil 20 tx contoh event fee=0 dari blok mentah (`pump-check --dump`). Tentukan field yang benar (mayhem / `virtual_quote_reserves` / versi event). Implementasi decoder per versi; masukkan cashback/buyback/holder_rewards ke SOL dibayar | fixture tx nyata mayhem & normal; cek `Δquote` == `curve_quote` dan k konstan ±1 lamport | invariant reserve lulus ≥99.9% event curve, termasuk fee=0; 0 event dengan error harga >0.1% |
| 1.2 | A1-3 default 0/false | `u()/s()/is_buy` → `Result`; tolak `_truncated`, trailing bytes, mint/user kosong; hitung alasan reject | hapus tiap field / ganti tipe → event ditolak, counter naik, **tidak pernah** jadi SELL palsu | 0 event default-valued masuk pipeline |
| 1.3 | txview `filter_map`, stackHeight | validasi penuh array & indeks; tanpa stackHeight → tandai unsupported | fixture key campur, indeks out-of-range, v0, CPI | tidak ada indeks bergeser |
| 1.4 | A1-2 pool per mint | simpan `pool` di `Ev`; state per pool; mint→pool aktif eksplisit | 2 pool PumpSwap satu mint | kontinuitas token-reserve PumpSwap ≥99.9% per pool |
| 1.5 | A1-5 pool math 2/4 jalur | uji curve buy/sell, AMM buy exact-in/exact-out, sell; checked arithmetic | prediksi trade k+1 dari reserve k untuk 4 jalur | ≥99% dalam 0.1% per jalur, laporan per jalur |
| 1.6 | A2-2 bias fee | simpan `gross_sol`, `net_sol`, `fees` terpisah; flow memakai definisi yang sama untuk buy & sell | unit test simetri | bias flow ≈ 0 |
| 1.7 | A1-7 definisi harga | field eksplisit: `exec_price`, `marginal_price_pre/post`, `exit_value_for_size`; simpan `decimals` | uji unit & desimal ≠ 6 | radar/fitur/label memakai definisi yang tertulis di registry |
- **Output:** `reports/DECODER_VALIDATION.md`, `reports/PRICE_AND_RESERVE_VALIDATION.md`.

### Fase 1B — Venue selain pump.fun (setelah Fase 1 lulus untuk pump.fun/PumpSwap)
**Masalah:** `config/venues.toml` mendaftar 11 venue (Raydium AMM v4/CPMM/CLMM/LaunchLab, Meteora DLMM/DAMM v2/DBC, Orca Whirlpool, Moonshot), tapi decoder hanya pump.fun curve + PumpSwap. Token yang hidup/ramai di Raydium/Meteora/Orca **tidak terlihat sama sekali** oleh radar, fitur, dan model.
| # | Perbaikan | Tes | Lulus bila |
|---|---|---|---|
| 1B.1 | Tabel status per venue `implemented / verified / unsupported` di `reports/VENUE_COVERAGE.md`; hitung tx per venue yang **tidak** ter-decode | — | tidak ada klaim "semua DEX" sebelum venue verified |
| 1B.2 | Decoder per venue dari IDL resmi di `config/idl/` (urutan prioritas berdasarkan volume memecoin terukur: Raydium CPMM + AMM v4 + LaunchLab → Meteora DBC/DAMM v2/DLMM → Orca Whirlpool → Moonshot) | fixture tx mainnet per venue: buy, sell, multi-hop, fee payer ≠ trader | side 100%, amount ≥99.9% vs balance-delta reference per venue |
| 1B.3 | Pool math per venue: CPMM (x·y=k + fee), CLMM/Whirlpool (tick & sqrtPrice), DLMM (bin), DBC (kurva) — implementasi + uji independen (agent `math-numeric-verifier`) | prediksi trade k+1 dari state k | ≥99% dalam 0.1% per venue per jalur |
| 1B.4 | Satu identitas token lintas venue: state & flow per **mint** dijumlahkan dari semua pool/venue, harga dari pool paling likuid (aturan tertulis) | mint dengan pool di 2 venue | flow per mint = jumlah flow semua pool |
| 1B.5 | Quote non-SOL (USDC/USDT) dikonversi ke SOL dengan harga SOL/USD pada slot yang sama, atau dicatat terpisah | — | tidak ada flow USDC yang hilang diam-diam |
- Venue yang belum verified **tidak boleh** masuk universe trading.

## Fase 2 — Data foundation
| # | Perbaikan | Lulus bila |
|---|---|---|
| 2.1 | D01 `blockTime` → `Option`, tolak/flag; tanpa `window=(0,…)` | fixture tanpa blockTime ditolak |
| 2.2 | D02 cache dengan manifest (hash sumber, IDL, decoder version, config, slot range, count) + `--force-rebuild-cache` | ubah 1 byte IDL → cache invalid; 2 run identik → hash output sama |
| 2.3 | D03 event id = struct `(slot, tx_index, ix, inner_ix, signature)`; ML pakai `snapshot_id` unik | event_index 255/256/257 tanpa tabrakan |
| 2.4 | D04/D05 input type eksplisit; gRPC/WS-only ditolak loader backtest; normalizer tanpa slot 0 | error jelas |
| 2.5 | A2-4 dust: satu definisi `REAL_MIN_SOL` dipakai di **semua** fitur hitungan; laporkan spam mint | `n_trades_*`, `f8_*`, holders bebas dust |
| 2.6 | D06 manifest kualitas per rekaman; `verify_backfill.json` wajib ada & non-kosong | gate menolak dataset tanpa manifest |
| 2.7 | Rebuild semua cache/dataset dari decoder Fase 1; regenerasi `trades.csv` dengan writer HEAD | header & angka konsisten dengan kode |
- **Output:** `reports/DATA_QUALITY.md`.

## Fase 3 — Radar, funnel, buy pressure
| # | Perbaikan | Lulus bila |
|---|---|---|
| 3.1 | R01: state `COLD→HOT→DEMOTED` eksplisit; snapshot hanya saat HOT/grace; `Demote` reset `hot_time` | tidak ada snapshot setelah demote; re-promote = interval baru |
| 3.2 | R02/M06: funnel lengkap 17 gerbang (lihat Lampiran R03) + `missing_score`, `score_nonfinite`; `read_scores` fail-closed (header, versi, kolom wajib, tanpa duplikat) | hapus 1% key → `missing_score` tepat 1%; skor risiko hilang = veto, bukan 0 |
| 3.3 | A2-1: **hapus** gerbang `bs/ss > ratio`; ganti dengan skor/fitur kontinu | tidak ada diskontinuitas sell=0 vs ratio |
| 3.4 | Liquidity gate memakai **real** SOL (curve) / quote real (AMM) | coin curve dengan real SOL kecil diblok |
| 3.5 | A2-7 dev sell: `creator_direct_sell` vs `unknown_balance`; tidak ada frac=1.0 default; hapus fitur duplikat | uji coin dibuat sebelum rekaman |
| 3.6 | A2-5/A2-6 ganti nama/flag fitur artefak (`wallet_first_seen_in_dataset`, `*_proxy`) dan **keluarkan dari model** sampai ada riwayat on-chain | fitur artefak tidak ada di allowlist |
| 3.7 | R05 radar eval per kohort: launch terverifikasi on-chain / awakening / mulai-tengah / instant jump | laporan per kohort |
| 3.8 | `radar.sweep` pakai antrean HOT saja (bukan semua coin) | benchmark O(HOT) |
- **Output:** `reports/RADAR_EVALUATION.md`, `reports/ENTRY_FUNNEL.md`.

### Fase 3B — Radar untuk token yang sudah lama (bukan hanya yang baru lahir)
**Masalah saat ini:**
- Radar fokus ke newborn (auto-HOT). Aturan "awakening" (`slow_rule`, `lift`) memang ada di `config/radar_params.json` (slow: ≥6 pembeli & ≥2 SOL dalam 300 s; lift ≥1.15×), tapi **di-tuning dan dievaluasi pada 2 window yang sama** (`frozen_from: bf_1h, h1d`) → overfit, belum ada bukti OOS.
- Window rekaman hanya ~35 menit → sistem **tidak tahu umur token**. Token berumur 6 jam yang tiba-tiba ramai dianggap "pertama terlihat", bukan "bangun lagi".
- Token yang sudah pindah ke Raydium/Meteora/Orca tidak terlihat (lihat Fase 1B).
- Rule `lift` memakai harga eksekusi (bias fee/slippage, A1-7).

**Perbaikan:**
| # | Perbaikan | Lulus bila |
|---|---|---|
| 3B.1 | **Umur & riwayat token nyata:** ambil waktu create/mint/pool-create on-chain (atau backfill riwayat ≥24 jam per mint yang aktif) → fitur `token_age_onchain`, `vol_24h_baseline`, `buyers_24h_baseline`, `ath_mult`, `drawdown_from_ath` | setiap kandidat punya umur on-chain atau flag `UNKNOWN` |
| 3B.2 | **Kohort radar terpisah:** (a) newborn < 10 m, (b) muda 10 m–2 j, (c) **mapan 2–24 j**, (d) lama > 24 j. Parameter & metrik per kohort | laporan recall/precision per kohort |
| 3B.3 | **Kriteria "awakening" berbasis lonjakan relatif terhadap baseline token itu sendiri**, bukan angka absolut: rasio arus masuk 5 m / rata-rata 24 j, pertumbuhan pembeli unik, net flow / likuiditas real, kenaikan dari harga dasar terakhir, pembeli baru (belum pernah pegang token ini) vs pembeli lama | lonjakan terukur sebagai z-score/multiple terhadap baseline, bukan threshold tetap |
| 3B.4 | **Buy pressure per token** (sama dengan Fase 5 point 6): FI per horizon 15 s/60 s/5 m/1 j, net inflow / likuiditas real, akselerasi inflow, breadth pembeli, porsi whale, absorption (kenaikan harga per unit inflow), flow lintas semua venue token itu | fitur tersedia untuk semua kohort; dust & spam dikecualikan |
| 3B.5 | **Tidak hard-filter sebelum model:** radar menghasilkan tier (watch / momentum / high-conviction / dangerous) dan mencatat rule yang memicu; model yang menilai | setiap kandidat punya tier + rule trace |
| 3B.6 | **Tuning radar walk-forward:** tuning di window dev, uji di window lain yang lebih baru (≥3 window) | recall & lead-time OOS dilaporkan per kohort, termasuk kohort mapan 2–24 j |
- Target model untuk kohort mapan dilatih terpisah atau dengan fitur kohort, karena perilaku token berumur jam berbeda dari newborn.

## Fase 4 — Data untuk training (lebih penting dari model)
1. **Kumpulkan lebih banyak window**: minimal **20 window** (target ≥4 minggu sesuai CLAUDE.md 1H), tersebar jam/hari berbeda, panjang window ≥ horizon terpanjang + 30 menit. Ukur biaya kredit Helius per jam dulu dan minta persetujuan user untuk jumlahnya.
2. **Label registry** (`config/labels.toml`): nama, horizon, barrier, definisi harga, aturan sensor, versi. Label hanya valid bila horizon penuh; sisanya `censored` (dibuang dari training, bukan negatif).
3. Ganti nama target: `p_peak2_15m`, `p_cont30_5m`, `p_rug_5m`, `p_dump20_30s`, `p_devdump_10m`; `y_rugdead` didefinisikan ulang (likuiditas/aktivitas kolaps dan tidak pulih dalam H, horizon penuh).
4. **Feature allowlist** versi (nama, unit, waktu ketersediaan, lineage). Tes: mutasi kolom label/masa depan → prediksi tidak berubah.
5. Laporan N per target: jumlah baris, **mint unik positif**, per window. Target dengan < 50 mint positif total → `INSUFFICIENT N`, jangan dilatih untuk keputusan.

## Fase 5 — Training (apa yang di-training, apa yang tidak)

### Yang DI-TRAINING (setelah Fase 1–4 lulus)
| Prioritas | Target | Alasan |
|---|---|---|
| 1 | `p_dump20_30s` (exit/risiko) | sinyal OOS terkuat (PR-AUC ~0.38–0.40 vs base 0.05), N cukup (174 mint) |
| 2 | `p_cont30_5m` (entry momentum) | PR-AUC ~0.31–0.35 vs base 0.10, 127 mint |
| 3 | `p_peak2_15m` | sinyal ada (~0.12 vs 0.04) tapi N kecil → training boleh, keputusan hanya setelah N cukup |
| 4 | `p_rug_5m` | N sangat kecil (21 mint) → kumpulkan data dulu |

### Yang TIDAK di-training sekarang
- `p_peak5`, `p_peak10`, "1x–100x": label positif hampir seluruhnya artefak A1-1; setelah dibersihkan 0–4 coin/window. Tunggu ≥ 4 minggu data.
- Model regresi "harga berikutnya" titik tunggal. Gunakan kuantil/distribusi bila diperlukan.

### Kontrak training
1. **Split kronologis walk-forward**: train window lama → validasi window berikutnya → test window lebih baru yang tidak disentuh. **Embargo** ≥ horizon label + latency. Group per mint (tidak ada mint yang sama di train & test pada periode tumpang tindih).
2. **Model:** baseline prior + Markov state table + logistic (ridge) + Random Forest + LightGBM + CatBoost, **split & biaya identik**. Pilih yang paling sederhana dalam CI terbaik (hasil A.5: RF/logistic setara CatBoost).
3. **Kalibrasi** (Platt/isotonic) di window validasi; laporkan Brier, log-loss, ECE, kurva kalibrasi. Threshold 0.6 ≠ probabilitas 60% sampai dikalibrasi.
4. **Metrik per target:** base rate, PR-AUC, ROC-AUC, precision@top-K, recall, Brier, CI bootstrap per mint/window, stabilitas per venue (curve vs AMM) dan per tier fee (karena `quote_res_sol`/`fee_bps` mendominasi importance).
5. **Ablation wajib** sebelum menambah komponen: CLEAN vs +buy-pressure non-linear vs +HMM (filter forward-only) vs +Kalman (trend log-price) vs +PCA. Komponen dipertahankan hanya bila menang OOS di ≥3 window. (A.5: BP & HMM belum menang.)
6. Buy pressure: jangan rasio 1:1. Pakai bentuk non-linear (FI per horizon, net flow / real liquidity, akselerasi FI, breadth pembeli, absorption = return per unit flow ternormalisasi, ex-whale ratio) dan biarkan model tree/kernel menangkap bentuk U-terbalik.
7. Kelas langka: class weight hanya di fold train; jangan oversample test. Cek jumlah kelas per fold sebelum fit.
8. Simpan: versi model, hash fitur, hash label, window train/val/test, seed.
- **Output:** `reports/MODEL_METRICS.md`, `reports/ML_LEAKAGE_AUDIT.md`.

## Fase 6 — Backtest, exit, hold
1. Fill pakai pool math per pool (Fase 1), latency, gagal-fill **buy dan sell** (sekarang sell selalu sukses), tolak trade di kurva complete.
2. Tuning threshold hanya di window dev; test window final sekali saja. Laporkan semua kombinasi yang dicoba (overfitting correction — skill `backtest-overfitting`).
3. Eksperimen exit/hold sebagai grid terpisah (30 s … 24 h), partial TP + runner, trailing, hazard; nilai exit = **executable sell quote untuk ukuran posisi**, bukan harga marginal.
4. Laporan: trade, coin unik, win rate, expectancy setelah biaya, PF, max DD, CVaR, slot utilisation, kandidat diblok per gerbang, runner yang terlewat & alasannya.
- **Output:** `reports/BACKTEST_OOS.md`, `reports/FULL_AUDIT_FINDINGS.md` (status tiap temuan: fixed / not reproduced / blocked).

## Fase 6B — Laporan kandidat: apakah model benar-benar membantu
Setiap run backtest/shadow **wajib** menghasilkan `reports/CANDIDATES_<window>.csv` + `reports/MODEL_HELPFULNESS.md`:
1. **Per kandidat:** waktu, mint, venue, kohort umur (3B.2), tier radar + rule yang memicu, skor model (terkalibrasi), keputusan (trade / tolak + alasan gerbang), harga masuk executable, hasil di tiap horizon hold, peak (MFE), drawdown (MAE), alasan exit, PnL bersih.
2. **Perbandingan wajib (window test yang sama, biaya sama):**
   - acak (kandidat radar dipilih acak, jumlah sama),
   - radar saja (tanpa model),
   - model top-K (5% & 1%),
   - model bawah 50% (harus lebih buruk — kalau tidak, model tidak bermakna).
3. **Metrik "membantu atau tidak":** selisih rata-rata return bersih model vs radar-only **dengan CI90 bootstrap per mint**; precision@K; berapa runner (≥2x/≥5x/≥10x) tertangkap vs terlewat dan di gerbang mana; win rate; PF; expectancy tanpa trade terbaik.
4. **Vonis tertulis per window:** `MEMBANTU` (CI selisih > 0 di ≥3 window berturut-turut), `TIDAK TERBUKTI` (CI melewati 0), `MERUGIKAN` (CI < 0). Hanya `MEMBANTU` yang boleh lanjut ke shadow trading.

## Fase 6C — Durasi hold & exit
- Grid hold tetap: 30 s, 1 m, 3 m, 5 m, 10 m, 15 m, 20 m, 30 m, 1 j, 2 j, 4 j, 8 j, 24 j — **per kohort umur** dan **per tier skor model**. Horizon > panjang rekaman = `BLOCKED` (butuh window lebih panjang), jangan diisi.
- Bandingkan dengan exit adaptif: trailing dari peak, partial TP (mis. 50% di 2x) + runner, exit flow-memburuk (net inflow / likuiditas berbalik negatif), exit model dump (`p_dump20_30s`), time-stop berbasis hazard.
- Nilai exit = quote jual executable untuk ukuran posisi, setelah fee, slippage, priority fee/tip, dan kemungkinan gagal jual.
- Tidak ada "hold terbaik universal": pilih aturan exit per kohort yang menang OOS di ≥3 window.

## Fase 7 — Sebelum live (setelah semua fase di atas lulus)
1. **Safety token (rug check) yang belum ada sama sekali di kode:** mint authority & freeze authority dicabut?, Token-2022 extensions (transfer fee/hook, permanent delegate), LP terkunci/terbakar, konsentrasi holder **nyata** (snapshot token account), wallet bundle/klaster pendana sama saat launch, honeypot (simulasi jual sebelum beli).
2. **Biaya eksekusi nyata:** priority fee & Jito tip per kondisi jaringan, risiko sandwich/MEV, latency nyata dari feed ke landing, tingkat gagal buy **dan** sell.
3. **Regime pasar:** harga & tren SOL, total volume memecoin, jumlah launch per jam — sebagai fitur/kondisi, dan laporan performa per regime.
4. **Kontrol risiko:** ukuran posisi maks, eksposur total, maks posisi bersamaan, batas rugi harian, kill-switch drawdown, blacklist creator.
5. **Monitoring:** drift fitur & kalibrasi, alert data feed putus, perbandingan backtest vs shadow (TCA).
6. **Shadow trading** ≥2 minggu (sinyal nyata, tanpa uang) → micro-live dengan modal kecil hanya atas persetujuan user.
7. **Keamanan wallet:** signer terpisah, keypair tidak pernah di repo/log, izin minimum.

## Analisa yang masih kurang (wajib dikerjakan, belum ada di audit ini)
| # | Yang belum dianalisa | Kenapa penting | Dikerjakan di |
|---|---|---|---|
| G1 | Data blok mentah untuk event `fee_bps=0` | memastikan penyebab bug harga terbesar | Fase 1.1 |
| G2 | `cargo test` / `clippy` / build | belum pernah dijalankan saat audit | Fase 0 |
| G3 | Kode feed live (gRPC, WS, recorder, gap tracker, replay) | baru dibaca sekilas; bisa ada event hilang/duplikat | Fase 2 (audit + tes chaos yang sudah ada) |
| G4 | 2 window lain di `reports/` | data tidak ada di repo → hasil lama tak bisa direproduksi | Fase 0 (minta data atau tandai tidak valid) |
| G5 | Venue selain pump.fun | token di Raydium/Meteora/Orca tak terlihat | Fase 1B |
| G6 | Umur token on-chain & baseline 24 j | radar token lama/awakening tidak bisa dinilai | Fase 3B |
| G7 | Holder & konsentrasi nyata, wallet klaster/bundle | proxy swap-only tidak akurat; inti deteksi rug | Fase 7.1 |
| G8 | Safety token (authority, Token-2022, LP lock, honeypot) | belum ada di kode sama sekali | Fase 7.1 |
| G9 | Hold > 30 menit (1 j – 24 j) | window rekaman cuma ~35 menit | Fase 4 (window lebih panjang) + 6C |
| G10 | Performa per regime pasar & jam/hari | 2 window dalam 2 hari ≠ representatif | Fase 4 + 7.3 |
| G11 | Biaya nyata (priority fee, Jito tip, MEV, gagal jual) | backtest sekarang memakai biaya tetap & jual selalu sukses | Fase 6 + 7.2 |
| G12 | Target 5x/10x/100x | N tidak cukup (0–4 coin/window setelah dibersihkan) | setelah ≥4 minggu data |

## Definisi selesai
- Semua kriteria "Acceptance Criteria" di Lampiran §13 terpenuhi, **plus**:
- 0 event curve dengan error invariant reserve >0.1% (A1-1 tuntas).
- 0 snapshot setelah demote (R01), 100% kandidat terhitung di funnel (R02).
- Tidak ada label dari horizon tidak lengkap; tidak ada fitur artefak di allowlist.
- Klaim performa hanya dari walk-forward ≥3 window test yang tidak disentuh, dengan CI.

---

# LAMPIRAN — Spesifikasi asli dari user

Lampiran (spesifikasi asli `SOLANA_SNIPER_V4_FULL_AUDIT_AND_FIX_PLAN_US_EN.md`, §0–§14: D01–D15, T01–T05, R01–R06, M01–M10, model §7, hold §8, funnel §9, gates §10, backtest §11, urutan §12, acceptance §13) disimpan terpisah di
[docs/SPEC_ORIGINAL_APPENDIX.md](docs/SPEC_ORIGINAL_APPENDIX.md) bila sudah disalin. Ringkasan setiap ID sudah tercakup di Bagian A (A.3 "Verifikasi klaim spesifikasi asli"). Acceptance criteria §13:

- [ ] No missing timestamp, trade direction, mint, amount, decimals, pool, or reserve is silently converted into a plausible zero/default event.
- [ ] Every input transaction is classified as decoded, failed, unsupported, malformed, non-swap, or excluded by a documented scope rule.
- [ ] The cache cannot be reused if data, decoder, schema, IDL, or config has changed.
- [ ] Event IDs never collide; ordering is deterministic and stable across repeated replays.
- [ ] Every listed venue has an explicit support status. The strategy does not claim coverage for unsupported DEXs.
- [ ] Prices and trade amounts reconcile for every supported venue/instruction type, including buys and sells, with per-type reports; reserve/fee math is tested against an independent source.
- [ ] Radar candidate → snapshots → scores → every gate → order → fill is fully accounted for. Score coverage is 100% for candidates actually evaluated, or missing-score failures explicitly block the run.
- [ ] All labels use complete forward horizons or explicit censoring. Target definitions match training, diagnosis, and execution semantics.
- [ ] Features contain no post-snapshot information. Training on future windows is not presented as live walk-forward evidence.
- [ ] Actual holder metrics use actual owner/token-account balances. Swap-only holder/concentration estimates are labeled proxy or unavailable.
- [ ] CatBoost is evaluated per target using rare-event metrics and calibration. Random Forest/HMM/Markov/Kalman/linear additions remain only if out-of-sample ablation demonstrates incremental value.
- [ ] Holding periods/exits are tested over predeclared horizons with fees, price impact, latency, and failed fills included. No universal "best hold" is claimed without evidence.
- [ ] Performance reports use multiple chronological windows, preserve the final holdout, and include uncertainty/insufficient-sample warnings.
- [ ] `cargo fmt`, workspace tests, lint, Python tests, deterministic replay, and all achievable stage gates pass; failures/blockers are listed.
- [ ] This task does not execute real-money mainnet transactions.
