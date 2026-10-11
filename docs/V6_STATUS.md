# Status V6: laporan serah terima

Status per 2026-10-11 malam (branch `v6-impl`). Semua hasil dari **paper / backtest data histori**. Live trading
tetap mati. Dokumen ini ditulis supaya pekerjaan bisa dilanjutkan dari titik ini oleh siapa pun.

## Ringkasan

**Yang berhasil**
- Decoder semua venue (pump.fun, PumpSwap, Raydium, Meteora, Orca, program tak dikenal). Audit trade vs transfer bersih:
  0 dari 260.551 leg yang ternyata transfer.
- Radar menangkap 97% koin yang naik >100% sebelum naik 10% (window 1, 15.544 koin).
- ML (CatBoost / Random Forest) jauh di atas base rate untuk ≥2x dan rug. Prediksi ≥10x lemah.
- **12 bug data / desain** ditemukan dan diperbaiki (bagian 4). Tiga di antaranya menghasilkan label 10x–86x palsu.
- Gate C18 (akurasi fill) lolos untuk 8 venue, termasuk Meteora DAMM v2/DBC, tempat sebagian besar peluang ML berada.
- Simulasi eksekusi wallet realistis (latency, fee, tip, slippage, gagal beli/jual, rent ATA), Kelly ¼ berbunga, filter
  memecoin saja.
- Alat ukur:
  - funnel per koin dan per venue di setiap backtest;
  - sweep komponen nyala/mati;
  - audit fee;
  - evaluasi tingkat koin.

**Yang masih kurang**
- **Data pendek.** Waktu keputusan total hanya ±3 jam: s02d 74 menit, tiga window lain masing-masing ~37 menit.
  Target ≥ 300 trade out-of-sample belum tercapai.
- Hasil setelah perbaikan terakhir (bug 9–12) **belum keluar**: training dan sweep sedang berjalan (bagian 7).
- Prediksi ≥10x lemah. Sampel moonshot asli ±10–100 per window.
- Kalibrasi probabilitas belum lolos, jadi Kelly belum sah untuk sistem final.
- Online learning per 10 koin belum terbukti lebih baik dari model tanpa belajar.
- Holdout akhir (s03d_21h) belum diuji.
- Download lambat karena paket Helius Free.

## 1. Sistem saat ini (alur)

1. **Data**: blok Solana histori dari Helius RPC (`getBlock`, paket Free, ~2–6 slot/detik). Tidak ada DexScreener
   atau Binance. Harga SOL/USD dihitung dari swap SOL–stablecoin on-chain (`sol_usd_1m.csv`). Yang dipindai: **semua
   koin yang diperdagangkan di semua venue**, baru maupun lama.
2. **Decoder** (Rust, `crates/decoder`, `crates/app/src/backtest.rs`):
   - pump.fun curve dan PumpSwap pakai decoder program.
   - Raydium AMM v4/CPMM/CLMM/LaunchLab, Meteora DLMM/DAMM v2/DBC, dan Orca Whirlpool pakai venue decoder (delta vault).
   - Program lain masuk lewat "pool legs" generik.
   - Arbitrase (beli + jual mint yang sama dalam 1 tx) dipindah ke wallet `arb_roundtrip`.
   - Pool berharga-eksekusi (konsentrasi / virtual) diberi harga dari swap yang sah saja (bug 4, 7).
3. **Radar** (Rust): status HOT / WATCH dari arus beli/jual dibanding likuiditas, jumlah trader dan cluster nyata
   (≥ 0,01 SOL; debu dihitung per cluster), akselerasi pembeli, ukuran buy, CUSUM.
4. **Fitur / snapshot**: 130 fitur per momen keputusan (tiap 2–5 detik per koin). Label = hasil 1 jam ke depan.
   Label kelas = kenaikan puncak sebelum jatuh −65%. Label rug = berakhir ≤ 20% tanpa pernah 2x.
5. **ML** (Python, `python/ml/v5_train.py`): `entry_bin` (kelas <0,35x … ≥50x), `entry_tp1` (≥2x), `rug` (CatBoost + RF),
   `hold`, `hazard`. Walk-forward dengan purge: model untuk window X hanya dari window sebelumnya.
6. **Strategi** (Rust, `crates/app/src/strategy_v5.rs`):
   - EV = Σ P(kelas) × hasil nyata per kelas di window training. Hasil kelas naik dibuat monoton.
   - Gate:
     - venue lolos C18;
     - pool dikenal;
     - likuiditas;
     - mint safety;
     - **memecoin saja** (`meme_only`);
     - EV ≥ `ev_min`;
     - rug veto;
     - price guard;
     - konsentrasi top-10 dan bundle.
   - Exit:
     - trailing dari ATH, adaptive trailing, atau momentum fade (jual 100%);
     - time stop berbasis flow;
     - **RugModel exit dengan ambang sendiri** (`rug_exit`);
     - dev sell dan LP drop.
7. **Eksekusi simulasi wallet** (`CostsV5`):
   - latency 2 slot;
   - per tx: fee 5.000 + priority 195.000 + tip 300.000 lamport;
   - fee pool: hasil decode, atau 1,25% per sisi untuk venue tanpa fee terdecode;
   - 15% buy gagal; slippage limit buy 20% dan sell 30%;
   - sell gagal 5% dengan 3 kali retry;
   - rent ATA dikunci lalu dikembalikan.
8. **Sizing**: taruhan tetap (modal awal / 18), compounding, atau **Kelly ¼** (`kelly_frac`). Satu-satunya batas:
   jumlah posisi (default 18).

## 2. Data histori

| Window | Waktu keputusan | Status |
|---|---|---|
| s02d_14h (window 1) | 74 menit | lengkap, decode terbaru |
| s04d_04h | 37 menit | lengkap, decode terbaru |
| s05d_11h | 37 menit | lengkap, decode terbaru |
| s06d_18h | 37 menit | lengkap, decode terbaru |
| s08d_08h, s09d_15h | – | download selesai / berjalan; **perlu `scripts/v6_redecode.sh`** dengan decoder terbaru |
| s03d_21h | – | **holdout akhir**, jangan dipakai sebelum setelan dibekukan |

Setiap window = waktu keputusan + perpanjangan ±1 jam (khusus label 1 jam).

GitHub Release `data-v6-2026-10-11`:
- **Raw blok** untuk 4 window lengkap (23,5 GB). Ini sumber kebenaran: decode ulang dari raw dengan kode terbaru.
  File di atas 1,9 GB dipecah; gabungkan dengan `cat <nama>.part* > <nama>`.
- Bundel olahan:
  - `*.processed.tar.zst`: dari decoder **sebelum** bug 7 dan 9. Usang; jangan dipakai untuk training.
  - `*.processed.v2.tar.zst`: dari decoder terbaru (diunggah 2026-10-11 malam).
- `models_v6_run2w.tar.zst`: model lama (label tercemar); hanya arsip.

## 3. Hasil terukur

**Radar** (window 1, 15.544 koin; tidak terpengaruh bug label):

| Naik | >100% | >200% | >400% | >900% | >1.900% | >4.900% | >9.900% |
|---|---|---|---|---|---|---|---|
| Koin | 565 | 369 | 239 | 125 | 73 | 25 | 13 |

- 97% yang naik >100% sudah WATCH/HOT sebelum naik 10%.
- Presisi radar ≈ base rate: radar menangkap hampir semua, tapi memilih adalah tugas ML.

**ML** (label bersih setelah bug 4, latih 1 window, uji window lain; `reports/v6/run2w/MODEL_FAMILIES_V6.json`):

| Target | AUC | Top 1% benar | Base rate |
|---|---|---|---|
| ≥2x | 0,81 | 48% | 12% |
| ≥10x | 0,77 | 13–16% | 1,7% |
| Rug/mati | 0,89 | 60% | 10,5% |

Angka ini sebelum bug 7 dan 9; pengukuran ulang dengan data terbaru sedang berjalan.

**Ciri moonshot asli** (5 menit sebelum naik; `reports/v6/MOONSHOT_TRAITS_w1_fixprice.json`):

| Ciri | Moonshot | Gagal | AUC |
|---|---|---|---|
| Buy terbesar | $819 | $261 | 0,79 |
| Jumlah transaksi | 668 | 97 | 0,77 |
| Arus bersih ÷ likuiditas | 1,0 | 0,1 | 0,75 |
| Akselerasi pembeli | +8 | −1 | 0,74 |

**Backtest 100 USD**, window s02d_14h (74 menit), semua sebelum bug 7–12 diperbaiki:

| Setelan | 100 USD jadi | Trade | Win rate | PF | Max DD |
|---|---|---|---|---|---|
| Venue pump saja (grid) | $108,9–111,7 | 2–4 | – | – | 0,7–2,8% |
| Venue C18, EV ≥ 0, taruhan tetap | $127,6 | 36 | 42% | 4,8 | 4,0% |
| Venue C18, EV ≥ 0, Kelly ¼ | $120,0 | 12 | 50% | 14,8 | 6,9% |

- Di baris 36 trade, 9 trade adalah token besar (ORCA 5×, PUMP 1×, WBTC 3×) yang dibeli karena label palsu (bug 7) dan
  bukan memecoin. Semuanya rugi biaya ±4,5%.
- 16 posisi masih terbuka saat data habis dan dinilai pada harga terakhir.
- Moonshot ≥10x yang tertangkap ≥5x: 0 dari 83.

## 4. Bug / masalah yang ditemukan dan sudah diperbaiki

| # | Masalah | Dampak | Perbaikan (kode / test) |
|---|---|---|---|
| 1 | Ukuran trade venue dari saldo trader | 17,9 SOL "untuk" 0,33 ORCA | ukuran dari delta vault (`venue_pool.rs`, `trade_size_from_vault_not_trader_balance`) |
| 2 | Pool DAMM v2 kosong jadi sumber harga | 58x palsu | pool < 1 SOL / program tak dikenal tidak jadi sumber harga baru (`ActivePool::accept`) |
| 3 | Inventori multi-quote (limit order) dibaca sebagai pool | harga salah | di-skip (`pool_legs.rs`) |
| 4 | Swap debu di pool berharga-eksekusi menentukan harga | label 10x/50x palsu; moonshot 30 → **10 asli** | `repair_exec_prices` (≥ 0,01 SOL dan ≥ 10.000 token mentah) |
| 5 | Nama window Rust vs Python beda | backtest ML gagal | `dir_name` membuang prefiks `v5_` |
| 6 | Online learning skor ulang semua baris tiap update | waktu kuadratik | skor sekali per baris; refit penuh tiap 100 update |
| 7 | Lompatan harga palsu (tambah likuiditas + swap kecil dalam 1 tx) | ORCA 2.000x, label ≥50x palsu; ORCA/PUMP ikut dibeli | lompatan > 3x harus dikonfirmasi swap berikutnya (`EXEC_PRICE_JUMP`) |
| 8 | EV meremehkan kelas ≥50x (penyusutan ke rata-rata negatif) | moonshot dinilai jelek | hasil kelas naik monoton (`monotone_upside`, test `upside_bins_are_monotone`) |
| 9 | Vault authority bersama (Raydium v4/CPMM/LaunchLab, DAMM v2, DBC): multi-hop memasangkan vault pool A dengan vault SOL pool B | token stabil bonkSOL berlabel 86x | swap ambigu di-skip (`Skip::AmbiguousVaults`, test `shared_authority_multi_hop_is_ambiguous`); harga stable dan SOL satu vault tidak dicampur |
| 10 | Exit RugModel memakai ambang yang sama dengan veto entry (0,5) | moonshot yang sudah terbeli dijual cepat | ambang `rug_exit` terpisah; nilai dipilih dari sweep |
| 11 | Rug veto 0,5 memblokir 19 dari 36 moonshot yang EV-nya positif (model rug menilai awal moonshot 0,5–0,7) | moonshot tidak dibeli | veto 0,5 / 0,7 / mati diuji di sweep; EV sudah memuat risiko crash |
| 12 | Aset non-memecoin ikut diperdagangkan (stablecoin, LST, BTC/ETH terbungkus, ORCA/PUMP/JUP, saham tokenisasi) | slot dan biaya terbuang | `meme_only`: daftar `config/non_meme_mints.txt`, flag mint berisiko, deteksi harga terpatok |

Audit yang **bukan masalah**:
- **Gate honeypot:** yang diblokir memang saham tokenisasi dan stablecoin.
- **Fitur `top10_supply_share` dan fitur lain:** kausal, tanpa kebocoran masa depan.
- **Fee pump.fun dan PumpSwap:** spread terukur 116–118 bps, cocok dengan fee terdecode 120–125 bps.

## 5. Kenapa sedikit trade dan moonshot tidak masuk (diagnosis)

1. **Data pendek.** 36 trade terjadi dalam 74 menit, bukan dari ribuan koin selama berhari-hari.
2. **Venue diblokir.** Koin EV tertinggi ada di Meteora DAMM v2/DBC, yang sebelumnya diblokir. Sudah dibuka setelah
   lolos C18.
3. **Rug veto dan RugModel exit** (bug 10, 11).
4. **Price guard.** Harga > 10% di atas titik terendah 5 menit.
5. **Slot penuh.** 18 slot terisi posisi yang lama dipegang: 16 masih terbuka di akhir; 323 sinyal ditolak karena kas habis.
6. **Pool launch Meteora DBC** dengan SOL nyata < 1 SOL dianggap pool kecil. Perbaikan yang benar butuh decode
   cadangan virtual DBC (belum dikerjakan).
7. **Label palsu** (bug 4, 7, 9) membuat model memilih token besar yang tidak bergerak.

## 6. Yang belum maksimal (jujur)

1. Data pendek (lihat di atas). Paket Helius Free kira-kira 1,7 jam download per 37 menit window.
2. Kalibrasi probabilitas belum lolos (C9). Kelly sudah ada, tapi belum sah untuk sistem final.
3. Online learning (C17) belum dibuktikan.
4. Tuning hanya di 1–3 window. Hyperparameter model belum di-search.
5. **Fee Meteora DBC/DAMM v2.** Spread di vault hanya 6–25 bps, tapi fee partner/protokol di luar vault belum terukur.
   Sweep juga dijalankan dengan 3% per sisi. Butuh decode event swap Meteora untuk angka pasti.
6. Decode cadangan virtual Meteora DBC belum ada (lihat bagian 5, poin 6).
7. Raydium CPMM **masih gagal** C18 dengan decoder terbaru (75% dalam ±10%; `reports/v6/FILL_CHECK_v2_4w.json`),
   jadi tetap tidak di-trade. Raydium LaunchLab (bonk.fun) sekarang lolos (880 trade) dan dibuka.
8. Holdout s03d_21h belum diuji.
9. Live trading tetap mati.

## 7. Proses yang sedang berjalan saat dokumen ini ditulis

Pipeline `pipeline_v2` (skrip di scratchpad sesi; langkahnya bisa diulang dengan perintah di bagian 9). Setelah
decode ulang 4 window dengan bug 7 dan 9 diperbaiki:

1. training walk-forward 4 window → `models/v6_run4w_fix`, laporan `reports/v6/run4w_fix/V5_MODELS.md`;
2. evaluasi tingkat koin → `reports/v6/run4w_fix/COIN_LEVEL.json`;
3. sweep 26 varian → `reports/v6/run4w_fix/SWEEP.json` (fee 1,25%) dan `SWEEP_fee300.json` (fee 3%).
   - Base: venue C18, EV ≥ 0, adaptive trailing, taruhan tetap 1/18, memecoin saja.
   - Varian:
     - rug veto 0,5 / 0,7 / mati; rug exit terpisah;
     - price guard nyala / mati;
     - entry di WATCH;
     - ambang EV;
     - mode exit; time stop klasik; hold maksimum;
     - Kelly ¼;
     - 10 / 18 / 30 posisi;
     - venue pump saja;
     - semua aset.
   - Ringkas dengan:
     `python3 python/research/sweep_report.py reports/v6/run4w_fix/SWEEP.json --variant base`.
     Hasilnya: modal 100 USD jadi berapa, trade, win rate, PF, DD, moonshot tertangkap, dan funnel koin ≥10x per venue.
4. online learning C17 → `reports/v6/run4w_fix/ONLINE_LEARN.json`.

Window uji walk-forward (`--min-train 1`): s05d, s04d, s02d. Window s06d hanya untuk training.

## 8. Langkah berikutnya (urut prioritas)

1. Baca hasil sweep. Tetapkan default dari data:
   - `rug_veto` dan `rug_exit`;
   - price guard;
   - mode exit;
   - sizing.

   Lalu bekukan setelan di `reports/v6/run4w_fix/base.json`.
2. Decode ulang s08d_08h dan s09d_15h (`scripts/v6_redecode.sh`), lalu ulangi training dan sweep dengan lebih banyak
   window uji. Target ≥ 300 trade out-of-sample.
3. Ambil lebih banyak data:
   - teruskan download (`scripts/v6_fetch_all.sh`);
   - pertimbangkan paket RPC berbayar atau rekaman live, karena Free ±1,7 jam per window 37 menit.
4. Decode event swap Meteora (DBC/DAMM v2: fee sebenarnya, cadangan virtual DBC) dan ulangi audit fee.
5. Kalibrasi (C9); Kelly hanya jika kalibrasi lolos.
6. Gate C17 online learning: dipakai hanya jika menang di ≥ 60% window.
7. Laporan gate per komponen C1–C18 (PASS/FAIL + diagnosis: desain, setelan, atau data kurang) dan learning curve.
8. Holdout s03d_21h sekali, di akhir, dengan setelan beku.
9. Baru setelah semua lolos: shadow, lalu micro-live (aturan Stage 9).

## 9. Cara reproduksi

```bash
export PATH=$HOME/.cargo/bin:$PATH && cargo build --release && cargo test --release --workspace
scripts/v6_redecode.sh s02d_14h s04d_04h s05d_11h s06d_18h      # decode ulang dari data/rawkeep (raw dari Release)
source /venv/main/bin/activate; export LABEL_H=3600
W="data/v5_s02d_14h data/v5_s04d_04h data/v5_s05d_11h data/v5_s06d_18h"
python3 python/ml/v5_train.py $W --min-train 1 --out models/v6_run4w_fix --reports reports/v6/run4w_fix --gpus 0 \
  --models entry_bin,entry_tp1,rug_cb,rug_rfcpu,hold,hazard,baseline_lr
python3 python/ml/coin_level_eval.py models/v6_run4w_fix $W --out reports/v6/run4w_fix/COIN_LEVEL.json
./target/release/sniper --start-sol 0.8433 --sol-usd 118.59 --label-horizon-s 3600 v5-backtest \
  --rec data/v5_s02d_14h --rec data/v5_s04d_04h --rec data/v5_s05d_11h --rec data/v5_s06d_18h --models models/v6_run4w_fix \
  --strategy reports/v6/run4w_fix/base.json --variants reports/v6/run4w_fix/variants.json --out reports/v6/run4w_fix/SWEEP.json
python3 python/research/sweep_report.py reports/v6/run4w_fix/SWEEP.json --variant base
python3 python/ml/online_learn.py $W --every 10 --target y_2x --out reports/v6/run4w_fix/ONLINE_LEARN.json
```

Alat audit:
- `python/research/fee_spread.py`: fee dari spread.
- `python/research/fill_check.py`: C18.
- `python/research/price_outliers.py`.
- `python/research/moonshot_traits.py`.
- `python/research/coin_card.py`.

Upload ke GitHub: `scripts/gh_publish.sh` (kode dan bundel) dan `scripts/gh_upload_raw.sh` (raw). Keduanya butuh
`GITHUB_TOKEN` di `.env`.
