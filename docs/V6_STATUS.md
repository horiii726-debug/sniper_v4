# Status V6: apa yang sudah dikerjakan, masalah, temuan, dan yang belum maksimal

Status per 2026-10-11 (branch `v6-impl`). Semua hasil di sini dari **paper / backtest data histori**. Live trading tetap mati.

## Ringkasan

**Yang berhasil**
- Decoder semua venue (pump.fun, PumpSwap, Raydium, Meteora, Orca, program tak dikenal), dengan audit trade vs transfer
  bersih (0 transfer salah baca).
- Radar menangkap 97% koin yang naik >100% sebelum naik 10%, di 15.544 koin window 1.
- ML terhadap base rate:
  - ≥2x: AUC 0,81; top 1% 48% vs 12%;
  - rug: AUC 0,89; top 1% 60% vs 10,5%.
- 6 bug data ditemukan dan diperbaiki, termasuk label 10x/50x palsu dari swap debu.
- Gate C18 (akurasi fill) lolos untuk 8 venue, termasuk Meteora DAMM v2/DBC, tempat sebagian besar peluang ML berada.
- Simulasi eksekusi wallet realistis (latency, fee, tip, slippage, gagal beli/jual, rent) dan Kelly ¼ berbunga.

**Yang masih kurang**
- Trade out-of-sample masih sedikit: 36 di 1 window setelah venue C18 dibuka (sebelumnya 2–4). Target ≥ 300 trade belum tercapai.
- Prediksi ≥10x lemah (presisi 13–16%).
- Kalibrasi probabilitas belum lolos.
- Online learning per 10 koin belum terbukti lebih baik.
- Tuning baru di 1–2 window.
- Holdout akhir belum diuji.
- Download data lambat karena rate limit Helius Free.

## 1. Sistem saat ini (alur)

1. **Data**: blok Solana histori dari Helius RPC (`getBlock`, paket Free, ~2–6 slot/detik karena rate limit). Tidak ada
   DexScreener atau Binance. Harga SOL/USD juga dihitung dari swap SOL–stablecoin on-chain (`sol_usd_1m.csv`).
2. **Decoder** (Rust): semua swap di semua venue.
   - pump.fun curve dan PumpSwap pakai decoder sendiri.
   - Raydium AMM v4/CPMM/CLMM/LaunchLab, Meteora DLMM/DAMM v2/DBC, dan Orca Whirlpool pakai venue decoder (vault delta).
   - Program yang tidak dikenal masuk lewat "pool legs" generik. Tiap transaksi yang menggerakkan token ikut direkam.
   - Swap arbitrase (beli + jual mint yang sama dalam 1 tx) dipindah ke wallet `arb_roundtrip` supaya tidak
     menggelembungkan jumlah trader.
   - Audit trade vs transfer: 0 dari 260.551 leg yang hanya transfer (`reports/v6/TRADE_VS_TRANSFER_w1.json`).
3. **Radar** (Rust): status HOT / WATCH dari arus beli/jual dibanding likuiditas, jumlah trader dan cluster nyata
   (≥ 0,01 SOL; debu dihitung per cluster), akselerasi pembeli, ukuran buy, CUSUM.
4. **Fitur / snapshot**: 130 fitur per momen keputusan (tiap 2–5 detik per koin). Labelnya adalah hasil 1 jam ke depan.
5. **ML** (Python, CatBoost / RF):
   - `entry_bin`: kelas kenaikan maksimum <1,5x … ≥50x;
   - `entry_tp1`: peluang ≥2x;
   - `rug`: rug atau mati (CatBoost + RF);
   - `hold` dan `hazard`.

   Semua dilatih walk-forward: model untuk window X hanya dilatih dari window sebelumnya.
6. **Strategi** (Rust, `strategy_v5.rs`):
   - EV = Σ P(kelas) × hasil nyata per kelas di window training.
   - Gate: rug veto, price guard, mint safety (honeypot), dan venue lolos C18.
   - Exit: trailing dari ATH, adaptive trailing, atau momentum fade, jual 100%.
   - Time stop berbasis flow (keluar jika ≤ 3 pembeli/60 dtk setelah 5 menit).
7. **Eksekusi simulasi wallet**:
   - latency 2 slot;
   - biaya per tx: fee 5.000 + priority 195.000 + tip 300.000 lamport;
   - 15% buy gagal; slippage limit buy 20% dan sell 30%;
   - sell gagal 5% dengan 3 kali retry;
   - rent ATA dikunci lalu dikembalikan.
8. **Sizing**:
   - taruhan tetap = modal awal / 18;
   - compounding (fraksi ekuitas);
   - **baru: Kelly ¼** (f* growth-optimal dari distribusi hasil nyata per kelas, dikali 0,25 × ekuitas saat ini).

## 2. Data histori

| Window | Status | Catatan |
|---|---|---|
| s02d_14h (window 1) | lengkap, decode final | 15.544 koin, ~3 juta event |
| s05d_11h | lengkap, decode final | |
| s04d_04h | lengkap, decode final | |
| s06d_18h | lengkap, decode final | |
| s08d_08h, s09d_15h | download / decode berjalan | |
| s03d_21h | **holdout akhir**, belum boleh dipakai | |

- Raw blok disimpan di `data/rawkeep` (~18 GB).
- Bundel hasil olahan per window ada di `data/export/*.processed.tar.zst` (event decode, trades.csv, snapshot, metadata,
  SHA256SUMS).
- `data/` tidak masuk git karena terlalu besar. Bundel diunggah sebagai GitHub Release `data-v6-2026-10-11` (cara pakai di bagian 7).

## 3. Bug yang ditemukan dan sudah diperbaiki

1. **Ukuran trade venue diambil dari saldo trader, bukan vault pool.** Contoh: 17,9 SOL "untuk" 0,33 token ORCA.
   Sekarang ukuran diambil dari delta vault.
2. **Pool DAMM v2 kosong (0 SOL) menjadi sumber harga**, sehingga muncul 58x palsu. Sekarang pool < 1 SOL dan program
   tak dikenal tidak boleh menjadi sumber harga baru.
3. **Pemilik inventori multi-quote** (limit order, market maker) salah dibaca sebagai pool. Sekarang di-skip.
4. **Swap debu di pool berharga-eksekusi** (DLMM, CLMM, DAMM v2, DBC, Whirlpool) menentukan harga. Contoh: 3 lamport untuk
   3 token mentah, atau 0,03 SOL untuk 1 token mentah, membuat harga meleset sampai 10^15 dan **label 10x/50x palsu**
   (termasuk token besar seperti PUMP dan MET).
   - Perbaikan: `repair_exec_prices`; harga hanya dari swap ≥ 0,01 SOL dan ≥ 10.000 token mentah.
   - Dampak: moonshot ≥10x di window 1 turun dari 30 ke **10 koin asli**, dan presisi ML ≥10x turun (lihat bagian 4).
     Klaim lama "top 1% benar 50–72%" **salah**.
5. Nama window Rust vs Python tidak cocok (`v5_` prefix), sehingga backtest ML gagal. Sudah diperbaiki.
6. Online learning prediksi ulang semua baris setiap update (waktu kuadratik). Sekarang tiap baris diskor sekali.
7. **Lompatan harga palsu di pool berharga-eksekusi.**
   - Contoh: delta vault 17,9 SOL melawan 0,33 ORCA, kemungkinan tambah likuiditas yang digabung dengan swap kecil
     dalam 1 tx. Akibatnya harga ORCA tercetak 2.000x dan muncul **label ≥50x palsu** pada ORCA.
   - Akibat ke backtest: sistem membeli ORCA 5 kali dan PUMP 1 kali. Semuanya datar, rugi ~4,5% karena biaya, dan
     memakan slot modal.
   - Perbaikan: lompatan > 3x dari satu swap ditahan sampai swap berikutnya mengonfirmasi level baru
     (`EXEC_PRICE_JUMP`, test `dust_exec_trades_do_not_set_price`). Semua window di-decode dan dilatih ulang.
8. **EV meremehkan moonshot.** Hasil kelas ≥50x di training (+45%, dari 1 trade) lebih rendah dari kelas 10–50x
   (+308%) karena penyusutan ke rata-rata negatif. Sekarang hasil kelas naik dibuat monoton (`monotone_upside`).

## 4. Hasil terukur (label bersih)

**Radar** (window 1, 15.544 koin):

| Naik | Jumlah koin |
|---|---|
| >100% | 565 |
| >200% | 369 |
| >400% | 239 |
| >900% | 125 |
| >1.900% | 73 |
| >4.900% | 25 |
| >9.900% | 13 |

- 97% koin yang naik >100% sudah WATCH/HOT sebelum naik 10%.
- Kelemahan: presisi radar ≈ base rate. Radar menangkap hampir semua koin yang naik, tapi juga banyak yang tidak naik.
  Memilih koin adalah tugas ML.

**ML** (latih di 1 window, uji di window lain; `reports/v6/run2w/MODEL_FAMILIES_V6.json`):

| Target | AUC | Top 1% benar | Base rate |
|---|---|---|---|
| ≥2x | 0,81 | 48% | 12% |
| ≥10x | 0,77 | 13–16% | 1,7% |
| Rug/mati | 0,89 | 60% | 10,5% |

- Random Forest dan CatBoost terbaik. Logistic lebih rendah. PCA paling jelek.
- Peringatan: top-1% per baris didominasi sedikit koin. Metrik per koin ada di `python/ml/coin_level_eval.py`.

**Ciri moonshot asli** (5 menit sebelum naik, 10 moonshot vs 349 gagal; `reports/v6/MOONSHOT_TRAITS_w1_fixprice.json`):

| Ciri | Moonshot | Gagal | AUC |
|---|---|---|---|
| Buy terbesar | $819 | $261 | 0,79 |
| Transaksi | 668 | 97 | 0,77 |
| Arus bersih ÷ likuiditas | 1,0 | 0,1 | 0,75 |
| Akselerasi pembeli | +8 | −1 | 0,74 |

**Backtest ML 100 USD** (window s02d_14h, waktu keputusan **74 menit**, out-of-sample, venue pump saja):

- $108,9–111,7, dengan hanya **2–4 trade**. Belum bermakna secara statistik.
- Penolak terbesar:
  - EV < 5%: 65.500 sinyal;
  - venue belum divalidasi: 30.900;
  - pool tidak dikenal: 19.600.
- Temuan: koin dengan EV tertinggi kebanyakan ada di **Meteora DAMM v2 / DBC**, yang tadinya diblokir.
- Gate C18 (akurasi fill) dengan data bersih: DAMM v2, DBC, DLMM, Orca, Raydium CLMM, dan Raydium AMM v4 **LOLOS**.
  Raydium CPMM dan LaunchLab GAGAL. Venue yang lolos sekarang dibuka (`c18_venues`).

**Sampel sesudah perbaikan** (`reports/v6/run2w/QUICK_*.json`; window yang sama; venue C18 dibuka; EV ≥ 0; adaptive
trailing; setelan tunggal, BELUM dipilih grid):

| Sizing | 100 USD jadi | Trade | Win rate | Profit factor | Max drawdown |
|---|---|---|---|---|---|
| Taruhan tetap 1/18 | $127,6 | 36 | 42% | 4,8 | 4,0% |
| Kelly ¼ berbunga | $120,0 | 12 | 50% | 14,8 | 6,9% |

- Exit: time stop 15, data habis 16 (posisi masih terbuka di akhir window, dinilai pada harga pool), adaptive trail 3,
  rug model 2.
- Kelly lebih sedikit trade karena 628 sinyal punya taruhan Kelly < 0,02 SOL (biaya tx tetap akan dominan).
- Masih 1 window dengan **74 menit** waktu keputusan (sebelumnya saya tulis ±2,5 jam; itu termasuk 1 jam perpanjangan
  khusus label): belum bermakna statistik. Moonshot 10x yang tertangkap ≥5x: 0 dari 83.

**Audit tambahan (2026-10-11 malam)**
- **Panjang data**: waktu keputusan per window hanya 74 menit (s02d) dan ~37 menit (s04d, s05d, s06d). Total ±3 jam.
  36 trade terjadi dalam 74 menit, bukan dari ribuan koin selama berhari-hari. Target ≥ 300 trade butuh ±12 jam
  waktu keputusan. Download paket Helius Free kira-kira 1,7 jam per 37 menit window, jadi ini hambatan utama.
- **Kenapa moonshot tidak masuk** (103 koin dengan label ≥10x di window 1):
  - **rug veto** memblokir 19 dari 36 moonshot yang EV-nya ≥ 0. Momen awal moonshot terlihat seperti rug bagi model
    rug (P(rug) 0,5–0,7). Label rug sendiri benar (berakhir ≤ 20% tanpa pernah 2x).
  - **price guard**: harga sudah > 10% di atas titik terendah 5 menit.
  - **EV** terlalu rendah (lihat bug 8).
  - **pool launch Meteora DBC** dengan SOL nyata < 1 SOL dianggap pool kecil (`pool_unknown` / `low_liquidity`).
- **Gate honeypot** (`mint_unsafe`) wajar: yang diblokir kebanyakan saham tokenisasi, stablecoin, dan BTC terbungkus
  (freeze authority / permanent delegate). Bukan memecoin.
- **Fee** (`python/research/fee_spread.py`, `reports/v6/FEE_SPREAD_s05d_11h.json`):
  - spread beli/jual di pump.fun dan PumpSwap (116–118 bps) cocok dengan fee yang di-decode (120–125 bps);
  - di vault Meteora DBC/DAMM v2 hanya 6–25 bps. Simulator memakai 1,25% per sisi.
  - Fee partner/protokol di luar vault belum terukur, jadi sweep juga dijalankan dengan 3% per sisi sebagai uji
    ketahanan.
- **Alat ukur baru**:
  - funnel per koin di setiap backtest: penghalang terakhir untuk semua koin, koin ≥2x, dan koin ≥10x;
  - mode `--variants`: sweep komponen nyala/mati dalam satu proses.

## 5. Yang belum maksimal (jujur)

1. **Jumlah trade out-of-sample masih sedikit.** Baru 1–2 window uji. Target ≥ 300 trade butuh lebih banyak window
   (download dibatasi rate limit Helius Free).
2. **Kalibrasi probabilitas belum lolos** (bin atas terlalu percaya diri). Kelly ¼ sudah diimplementasi dan diuji di
   backtest, tapi menurut aturan V4-12 Kelly baru sah setelah kalibrasi lolos.
3. **Online learning per 10 koin (C17)**: model tumbuh 20 pohon per update (ribuan update), lambat dan berat. Hasil
   perbandingan dengan model statis masih berjalan. Jika tidak menang di ≥ 60% window, fitur ini tidak dipakai.
4. **Tuning**: setelan strategi dipilih grid 32 titik per varian berdasarkan median return window dev. Dengan 1–2
   window, risiko kebetulan cocok tinggi. Hyperparameter model belum di-search (hanya early stopping).
5. **Prediksi ≥10x** masih lemah (13–16% presisi). Sampel moonshot asli sedikit (10 per window).
6. **Holdout s03d_21h belum dievaluasi.** Itu ujian akhir yang wajib sebelum klaim apa pun.
7. **Live trading tetap mati.** Shadow/live baru setelah full-system validation lolos.

## 6. Yang sedang berjalan / langkah berikutnya (urut prioritas)

1. `run2w`:
   - sampel cepat (`QUICK_fixed` / `QUICK_kelly25`): venue C18 dibuka, EV ≥ 0;
   - grid 3 mode exit × {venue C18 + taruhan tetap, venue C18 + Kelly ¼}.

   Laporkan modal akhir, jumlah trade, win rate, drawdown, PF, dan penolak per alasan.
2. `run4w` (4 window: s02d, s04d, s05d, s06d): training, online learning, dan 9 backtest. Hasilnya dilaporkan per
   window, supaya terlihat apakah sistem cuan di rezim/hari berbeda.
3. Target **≥ 300 trade out-of-sample**:
   - decode s08d_08h dan s09d_15h (download sudah jalan);
   - tambah window, ulangi run dengan `MIN_TRAIN=2` di semua window;
   - kumpulkan trade semua window uji.
4. **Gate C17 online learning**: bandingkan ML yang belajar tiap 10 koin dengan walk-forward dan model statis
   (`ONLINE_LEARN.json`). Jika tidak menang di ≥ 60% window, tidak dipakai. Model online perlu dibatasi ukurannya,
   karena saat ini bertambah 20 pohon per update.
5. **Kalibrasi (C9)**: isotonic per window. Kelly hanya sah di sistem final kalau kalibrasi lolos.
6. **Laporan gate per komponen C1–C18** (PASS/FAIL + diagnosis: desain jelek, setelan, atau data kurang). Ditambah
   learning curve (akurasi vs jumlah window) untuk memisahkan "data kurang" dari "desain jelek".
7. Wallet scores / cluster debu pada window V6.
8. Holdout s03d_21h: satu kali, di akhir, dengan setelan yang sudah dibekukan.
9. Unggah raw segment (~18 GB) ke release, dipecah per window.

## 7. Cara reproduksi

```bash
export PATH=$HOME/.cargo/bin:$PATH && cargo build --release
scripts/v6_redecode.sh s02d_14h            # decode ulang dari raw (data/rawkeep)
MIN_TRAIN=2 scripts/v6_ml_run.sh run4w data/v5_s02d_14h data/v5_s04d_04h data/v5_s05d_11h data/v5_s06d_18h
python3 python/ml/coin_level_eval.py models/v6_run4w data/v5_s02d_14h ...
```

Isi GitHub Release `data-v6-2026-10-11`:
- `v5_<window>.processed.tar.zst`: data olahan per window.
- `models_v6_run2w.tar.zst`: model ML terlatih run2w.
- `v5_<window>__raw-*.zst[.partNN]`: raw blok. File di atas 1,9 GB dipecah; gabungkan dengan
  `cat <nama>.part* > <nama>` lalu taruh di `data/v5_<window>/`.

Data: unduh bundel `v5_<window>.processed.tar.zst` dari GitHub Release `data-v6-2026-10-11`, cek dengan `SHA256SUMS`, lalu
ekstrak: `mkdir -p data/v5_<window> && zstd -dc v5_<window>.processed.tar.zst | tar -x -C data/v5_<window>`.
