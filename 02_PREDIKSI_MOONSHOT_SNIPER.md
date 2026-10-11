# PREDIKSI MOONSHOT — Rumus & Komponen (sniper_v4)

> **Untuk Claude Code dan user.** File ini khusus spesifikasi **prediksi koin moonshot**: semua komponen dari data →
> radar → saringan → ML → eksekusi → exit, lengkap dengan rumus, cara membaca, dan status.
>
> - Masalah sistem, daftar bug (V4-x, V5-x), dan urutan kerja ada di file terpisah
>   **`01_FORENSIK_SISTEM_SNIPER.md`** (di sini disebut "file forensik").
> - File ini menggabungkan: (1) cetak biru prediksi dari review forensik (2026-10-11), dan (2) seluruh isi
>   **`MEME_COIN_QUANT_FORMULAS.md`** milik user (15 rumus, ditandai **[U1]–[U15]**). Rumus dan "cara membaca"
>   dari user dikutip apa adanya, lalu ditambah status, cara implementasi, dan koreksi.
> - Balas user dalam Bahasa Indonesia; kode, komentar, dan report dalam Bahasa Inggris (aturan `CLAUDE.md`).
> - **Tidak ada satu pun komponen di file ini yang sudah terbukti menghasilkan profit.** Semuanya hipotesis sampai
>   lolos validasi (Bagian 18).

**Legenda status**

| Tanda | Arti |
|---|---|
| ✅V4 | sudah ada di `main` (`e75a8c5`) |
| 🟡V5 | ada di branch `v5-upgrade` (`b70c27c`), belum dites dengan data V5 |
| ❌ | belum ada |
| [U#] | nomor rumus dari `MEME_COIN_QUANT_FORMULAS.md` |

---

## 0. Aturan pakai

1. **Satu engine.** Semua metrik dihitung di satu engine Rust (`crates/app/src/ml_sim.rs::Engine`) dan dipakai
   bersama oleh radar, snapshot training, dan simulator. Saat ini ada dua engine (bug V4-10 di file forensik).
2. **Kausal.** Pada waktu t hanya boleh memakai data sampai t. Baseline (rata-rata, std, median) dihitung bergulir
   dari masa lalu. Pertahankan causality test V5 (prefix stream vs full stream harus identik).
3. **Rust = jalur cepat** (feed, decoder, fitur, inferensi, strategi, eksekusi). **Python = offline** (label,
   training, kalibrasi). Tanpa LLM atau AI agent.
4. **Ablation wajib.** Komponen baru dipakai hanya jika expectancy strategi out-of-sample (setelah biaya) naik di
   mayoritas window uji. AUC naik saja tidak cukup (Bagian 18).
5. **Paper / backtest / shadow saja.** Tidak ada trading live dan tidak ada kunci wallet.
6. **Izin dulu.** Sebelum mulai, minta konfirmasi user bahwa pekerjaan ini boleh jalan sebagai riset eksploratif
   (konflik dengan stage-gate di `CLAUDE.md`).
7. **Apa adanya.** Laporkan hasil negatif dan kegagalan sebagaimana adanya.

---

## 1. Keputusan user (wajib diikuti)

| # | Keputusan |
|---|---|
| 1 | Radar membaca **koin baru dan koin lama** |
| 2 | Inti radar = **arus uang + keramaian trader** (itu yang menggerakkan harga) |
| 3 | **Semua venue**: pump.fun, PumpSwap, Raydium, Meteora, Orca, dan lainnya |
| 4 | Data langsung dari chain lewat **gRPC/RPC**. DexScreener terlalu lambat; hanya boleh untuk metadata |
| 5 | Transaksi **debu (< 0,01 SOL) tetap dihitung**, digabung per cluster wallet |
| 6 | Arus dibandingkan **likuiditas**; market cap hanya konteks |
| 7 | **Beli sebelum harga naik**; jangan beli setelah +10–30% |
| 8 | Exit **jual 100%** (tanpa jual setengah), berbasis **momentum** |

---

## 2. Pipeline

```
DISCOVERY semua venue                                        [U1]
 → NORMALISASI (arus bertanda SOL, cluster, likuiditas, tag debu)
 → METRIK: arus uang, trader, supply, harga (Bagian 5–10)    [U2–U5]
 → CHANGE-POINT (CUSUM) → WATCH → ARM                        [U6]
 → SARINGAN KERAS: honeypot, creator-family, rug             [U7]
 → MODEL: label moonshot / pre-pump, kalibrasi, EV           [U8–U9]
 → RANKING + GATE ENTRY + SLOT                               [U10]
 → EKSEKUSI REALISTIS                                        [U15]
 → EXIT jual 100%: momentum / trailing / darurat             [U11–U14]
 → UMPAN BALIK: shadow log, update database wallet, drift
```

---

## 3. Data dan discovery [U1]

**[U1] Multi-DEX Discovery** (dari user)
- **Fungsi:** menemukan token baru dan token lama yang mulai aktif di berbagai DEX.
- **Cara membaca:** pantau token/pool dari semua venue yang didukung. Tandai token baru, token yang baru migrasi, dan
  token lama dengan aktivitas yang meningkat. Discovery hanya menemukan kandidat; bukan sinyal beli.

**Implementasi**
- Subscribe gRPC ke semua program DEX di `config/venues.toml`. Setiap program ID diverifikasi ke sumber resmi dan
  sumbernya dicatat di config (aturan `CLAUDE.md`).
- Tiga jenis kandidat:
  - **Token baru** = event create on-chain (`CreateEvent` / pool create). **Bukan** "pertama kali terlihat di window"
    (bug V4-1: 61–65% "koin baru" ternyata koin lama).
  - **Baru migrasi** = event curve complete → pool AMM.
  - **Token lama aktif lagi** = alarm change-point pada arus/trader (Bagian 7).
- Semua pool milik satu mint digabung jadi satu koin.

**Status lapisan data**

| Komponen | Status | Kenapa penting untuk moonshot |
|---|---|---|
| Swap pump.fun curve + PumpSwap | ✅V4 | data dasar |
| Swap Raydium / Meteora / Orca / lainnya | ❌ | moonshot di launchpad lain dan koin lama di sana tidak terlihat |
| Token baru dari event create | 🟡V5 (`decode_creations`; dipakai untuk fitur, **belum** untuk radar) | membedakan koin baru vs lama |
| Event migrasi sebagai pemicu | ❌ (PumpSwap terbaca ✅V4, tapi migrasi hanya diblokir) | migrasi = katalis |
| Token lama aktif lagi | ✅V4 aturan awakening radar (tapi tertutup bug V4-1) | jalur koin lama |
| Transfer SPL / Token-2022 untuk mint yang dipantau | ❌ | supply tersembunyi yang dipindah dev |
| Saldo token account (holder asli) | ❌ | konsentrasi dan pertumbuhan holder yang sebenarnya |
| Transfer SOL antar-wallet (funding graph) | ❌ | keluarga dev, cluster sybil, pembeli yang baru didanai |
| Mint / freeze authority, ekstensi Token-2022 | ❌ | honeypot |
| Data follow 10 jam setelah HOT | 🟡V5 | label tanpa sensor |
| gRPC mainnet live | ❌ (Helius Free menolak) | 87% runner ≥11x lompat dalam 2 slot |
| Database wallet live (kontinu) | ❌ (offline per window 🟡V5) | "siapa yang beli" |
| Metadata off-chain (link, profil/boost berbayar, nama jiplakan) | ❌ | katalis perhatian |

---

## 4. Normalisasi (definisi untuk semua rumus)

- **Arus bertanda** trade i: `s_i = +q_i` (beli) atau `−q_i` (jual), `q_i` dalam SOL. Quote non-SOL (misalnya USDC)
  dikonversi dengan harga mid pool saat trade.
- **B_w** = total SOL beli dalam window w; **S_w** = total SOL jual dalam window yang sama. w = 15 s / 60 s / 300 s.
- **Cluster** trader: `c(i)` dari funding graph + co-buying (Bagian 11); kalau tidak diketahui, cluster berisi
  wallet itu sendiri. ❌
- **Likuiditas L** = quote reserve koin, dijumlah dari semua pool-nya. Di curve pump.fun: pakai reserve **virtual**
  untuk menghitung dampak harga, dan reserve **asli** (`real_quote`) untuk cek "bisa keluar atau tidak"
  (bug V4-4 / V5-13).
- **Harga marginal**: CPMM `p = Q / T` (Q = quote reserve, T = token reserve). CLMM/DLMM: dari `sqrtPrice` /
  bin aktif. ❌ untuk venue selain pump.fun.
- **Tag debu** `d_i = 1` jika `q_i < 0,01 SOL`. **Disimpan, tidak dibuang.** ❌ (radar membuangnya,
  `crates/radar/src/lib.rs:158`)
- **Sampling waktu tetap**: metrik untuk change-point dan model diambil per langkah waktu tetap (misalnya 1 s atau
  5 s), bukan per trade, supaya ledakan trade tidak mendistorsi baseline.

---

## 5. Arus uang [U2, U3]

**[U2] Signed Money Flow** (dari user)
- **Rumus:** `F = B − S`. B = nilai transaksi beli dalam periode pengamatan; S = nilai transaksi jual dalam periode yang sama.
- **Cara membaca:** `F > 0` nilai beli lebih besar daripada jual; `F < 0` nilai jual lebih besar; `F ≈ 0` relatif seimbang.

**[U3] Flow Imbalance** (dari user)
- **Rumus:** `FI = (B − S) / (B + S + ε)`; ε = angka kecil untuk menghindari pembagian dengan nol.
- **Cara membaca:** mendekati +1 berarti beli dominan; mendekati −1 jual dominan; sekitar 0 relatif seimbang.
  Bandingkan pada periode waktu yang konsisten.
- **Catatan:** FI tidak tahu ukuran. FI = +1 dari 0,05 SOL sama nilainya dengan FI = +1 dari 50 SOL. Selalu
  pasangkan dengan F2 (arus ÷ likuiditas) dan F3 (churn).

| ID | Metrik | Rumus | Status |
|---|---|---|---|
| F1 = [U2] | Arus bersih | `NF_w = Σ_{i∈w} s_i = B_w − S_w` | ✅V4 `net_flow_15s/60s` (trade ≥ 0,01 SOL saja) |
| F11 = [U3] | Flow imbalance | `FI_w = (B_w − S_w) / (B_w + S_w + ε)` | ✅V4 `flow_imb_60s` (`ml_sim.rs:483`, hanya 60 s, tanpa debu) |
| F2 | Arus ÷ likuiditas | `NFL_w = NF_w / L` | 🟡V5 `ofi_60s` |
| F3 | Churn (volume dua arah) | `CH_w = (B_w + S_w) / (abs(B_w − S_w) + ε)` | ❌ |
| F4 | Percepatan SOL beli | `a_t = R_fast(t) / (R_slow(t) + ε)`; EWMA `R(t) = R(t⁻)·e^(−Δt/τ) + q_i/τ` pada SOL beli; τ = 10 s / 120 s | ❌ (✅V4 `accel` menghitung jumlah buy, bukan SOL) |
| F5 | Cumulative volume delta | `CVD_300 = Σ_{300 s} s_i / L` | 🟡V5 `cvd_300s` |
| F6 | Kyle lambda | OLS `Δln p_i = λ·s_i + e_i` → `λ̂ = Σ s_i·Δln p_i / Σ s_i²` | 🟡V5 `kyle_lambda_60s` |
| F7 | Volatilitas realisasi | `σ_60 = std` dari log return 1 detik selama 60 s | 🟡V5 `vol_60s` |
| F8 | Arus ternormalisasi volatilitas | `Z_flow = NFL_60 / (σ_60 + ε)` | ❌ |
| F9 | Arus bersih smart money | `SMF_w = Σ_{i: score(c(i)) > θ} s_i / L` | 🟡V5 sisi beli saja (`smart_buy_q_60s`); versi bersih + cluster ❌ |
| F10 | Arus bersih keluarga dev | `DFF_w = Σ_{i: c(i) ∈ keluarga dev} s_i / L` | ❌ |

**Penting: identitas CPMM (supaya rancangan tidak salah).** Dengan constant product `Q·T = k` dan `p = Q/T`,
berlaku `p = Q²/k`, sehingga `Δln p = 2·Δln Q ≈ 2·NF/Q` (fee diabaikan).
- Di **satu** pool CPMM, perubahan harga **sepenuhnya ditentukan** oleh arus bersih. "Arus beli bersih masuk tapi
  harga datar" **tidak mungkin terjadi** di satu pool.
- Jadi akumulasi sebelum naik harus dibaca dari **komposisi**: siapa yang beli, siapa yang jual, dan perpindahan
  supply (Bagian 6 dan 8), plus churn (F3). Jangan dibaca dari perbandingan arus vs harga.
- Selisih antara `2·NF/Q` dan `Δln p` yang terjadi hanya muncul kalau ada beberapa pool atau likuiditas CLMM berubah.
  Itu sinyal likuiditas/arbitrase. Secara teori `λ = 2/Q`; kalau `λ̂ ≫ 2/Q`, likuiditas tipis atau terpecah.

---

## 6. Aktivitas trader [U4]

**[U4] Buyer Acceleration** (dari user)
- **Rumus:** `A_t = (N_t − N_{t−Δ}) / Δ`. N_t = jumlah buyer unik pada periode terbaru; N_{t−Δ} = jumlah buyer unik
  pada periode sebelumnya; Δ = jarak waktu antarpengamatan.
- **Cara membaca:** A_t yang naik berarti laju pertambahan buyer meningkat. Deduplikasi wallet dan periksa wallet
  terkait agar aktivitas palsu tidak dihitung sebagai pertumbuhan independen.
- **Implementasi:** hitung N per **cluster** (C1), bukan per wallet. Pakai dua bentuk sekaligus:
  - **selisih** (U4) untuk kecepatan absolut;
  - **rasio** (C2) supaya bisa dibandingkan antar koin. Koin besar otomatis punya selisih besar.

| ID | Metrik | Rumus | Status |
|---|---|---|---|
| C11 = [U4] | Percepatan buyer (selisih) | `A_t = (U_[t−Δ,t] − U_[t−2Δ,t−Δ]) / Δ`, U = cluster unik | ❌ |
| C1 | Cluster unik yang beli | `U_w` = jumlah cluster berbeda yang beli dalam w | ❌ (V4/V5 menghitung wallet, `buyers_60s`) |
| C2 | Pertumbuhan keramaian (rasio) | `G = U_[t−60,t] / (U_[t−120,t−60] + 1)` | 🟡V5 `buyer_growth` (wallet); versi cluster ❌ |
| C3 | Jumlah pembeli efektif | `N_eff = 1 / HHI`, `HHI = Σ_c (b_c / B)²` per cluster | 🟡V5 `hhi_buy_60s` (per trade); per cluster ❌ |
| C4 | Porsi wallet baru | porsi SOL beli dari wallet berumur on-chain < X hari | 🟡 V4 `new_wallet_share_60s` = baru terlihat **di window** (bias); umur on-chain ❌ |
| C5 | Retensi | `R_5m` = porsi pembeli di `[t−5m, t−4m]` yang masih memegang pada t | ❌ (`early_retention` 20 pembeli pertama 🟡V5) |
| C6 | Hawkes (saling memancing) | `λ(t) = μ + Σ_{t_i<t} α·e^(−β(t−t_i))`; branching ratio `n = α/β`, 0 ≤ n < 1; n → 1 = trader saling memancing (herding) | 🟡V5 `hawkes_branching` |
| C7 | Intensitas beli vs jual | rasio intensitas Hawkes beli / jual | ✅V4 `f8_hawkes_buy_sell_ratio` |
| C8 | Porsi bot / sniper / flipper | porsi SOL beli dari wallet yang ditandai | 🟡V5 (perbaiki V5-12) |
| C9 | Wash per cluster | SOL di mana cluster yang sama beli dan jual dalam Δt ÷ total SOL | ❌ (✅V4 `f4_wash_share_60s` hanya wallet sama dalam satu tx) |
| C10 | Struktur debu | SOL debu, jumlah debu, porsi debu dari cluster terbesar | ❌ (✅V4 `dust_share_60s` = porsi jumlah, level wallet) |

**Catatan Hawkes (C6).** V5 memakai estimator momen `n̂ = 1 − 1/√F`, dengan Fano `F = Var(N)/E(N)` pada bin 10 slot.
Rumus ini mengasumsikan bin **jauh lebih panjang** dari `1/β`; bin ±4 detik membuat `n̂` cenderung terlalu rendah.
Validasi offline dengan MLE: maksimalkan `ln ℒ = Σ_i ln λ(t_i) − ∫_0^T λ(t) dt`.

**Interpretasi debu (keputusan user no. 5):**
- Banyak debu + dari satu cluster + cluster asli tidak bertambah = **volume palsu** (sinyal negatif).
- Debu yang totalnya besar dari satu cluster = **akumulasi tersembunyi** (perhatikan: bisa smart money, bisa dev).

---

## 7. Change-point / CUSUM [U6] — komponen baru

**[U6] CUSUM** (dari user)
- **Rumus:** `S_t = max(0, S_{t−1} + x_t − μ₀ − k)`. x_t = metrik yang dipantau (misalnya flow atau jumlah transaksi);
  μ₀ = baseline normal; k = parameter toleransi perubahan kecil.
- **Cara membaca:** S_t yang terus meningkat menandakan metrik menyimpang secara persisten dari baseline. Ambang sinyal
  harus ditentukan dan diuji dengan data historis; CUSUM bukan prediksi harga langsung.
- **Status:** ❌ (tidak ada di `main` maupun `v5-upgrade`).

**Spesifikasi implementasi**
1. **Standarisasi:** `z_t = (x_t − μ̂_t) / σ̂_t`, lalu `S_t = max(0, S_{t−1} + z_t − k)`. **Alarm** jika `S_t > h`.
2. **Baseline kausal dan robust:** μ̂ dan σ̂ dari masa lalu koin itu sendiri: EWMA mean/variance dengan τ panjang, atau
   median dan `MAD × 1,4826`. Data memecoin berekor tebal, jadi versi robust lebih aman.
3. **Warm-up:** baseline baru sah setelah N observasi. Koin baru tanpa riwayat memakai baseline populasi, yaitu median
   koin lain dengan kelompok umur yang sama.
4. **Metrik yang dipantau (x):** NFL (F2), pertumbuhan cluster (C2), laju SOL beli (F4), cluster unik (C1). Ukuran
   transaksi pakai skala log.
5. **Parameter awal:** k = 0,5 (mendeteksi pergeseran ±1σ), h = 4–5. Dipilih **hanya** di window dev.
6. **Reset:** `S = 0` setelah alarm (atau simpan sebagai status WATCH).
7. **Peran di pipeline:** alarm pada koin lama = kandidat **WATCH**. Alarm + lolos saringan keras = **ARM**.
8. **Evaluasi:** laporkan dua angka sekaligus:
   - rata-rata waktu sampai alarm palsu pada koin yang **bukan** runner (ARL);
   - **jeda deteksi** pada runner, diukur **sebelum** harga +10%.

---

## 8. Supply dan holder

| ID | Metrik | Rumus | Status |
|---|---|---|---|
| H1 | Holder asli | dari token account on-chain | ❌ (V4/V5 dari trade yang teramati) |
| H2 | Konsentrasi holder | `HHI_hold = Σ_w (bal_w / S)²`, Gini; top-10 terhadap **total** supply S | 🟡V5 `top10_supply_share`; HHI / Gini ❌ |
| H3 | Supply keluarga dev | `Σ_{w ∈ keluarga} bal_w / S` | ❌ (✅V4 `dev_hold_share` = creator saja, holding teramati) |
| H4 | Kecepatan holder | `Δholders / 60 s` | 🟡V5 `holder_vel_60s` (teramati) |
| H5 | Profit overhang | `PO = Σ_w bal_w·max(P − c_w, 0) / Σ_w bal_w·P`; c_w = harga beli rata-rata | 🟡V5 `profit_overhang` |
| H6 | Tembok jual di k× | token yang `c_w ≤ P/k` (holder sudah untung ≥ k×) ÷ S | ❌ |
| H7 | Perpindahan supply (dasar akumulasi) | saat `abs(Δln p)` kecil: porsi jual dari holder awal/untung DAN porsi beli dari cluster baru; jumlah holder naik sementara `HHI_hold` turun | ❌ |
| H8 | Transfer keluar keluarga dev | token yang **ditransfer** (bukan di-swap) dari keluarga dev ke wallet lain ÷ S | ❌ |

---

## 9. Harga

| ID | Metrik | Rumus | Status |
|---|---|---|---|
| P1 | Return | `r_τ = P_t / P_{t−τ} − 1`, τ = 5/15/60/300 s | ✅V4 |
| P2 | Jarak dari dasar | `D = P / min_{5 menit} P` | ✅V4 `price_vs_low300` |
| P3 | Rentang menyempit | `RC = range_5m / range_30m`, `range = (max − min) / mean` | ❌ |
| P4 | Jarak dari harga tertinggi | `P / max_{sejak create} P` | ❌ (butuh riwayat sejak create; data follow 🟡V5) |
| P5 | Batas harga entry | entry hanya jika `D ≤ 1 + max_entry_ret` (misalnya 1,10) | ❌ |

Bukti kenapa P5 penting: di V4, median harga entry sudah +5–11% dibanding 60 detik sebelumnya, dan 10% entry sudah
2,2–2,5x (`reports/diagnose_exploratory.json`).

---

## 10. Curve, migrasi, katalis

| Komponen | Status |
|---|---|
| `curve_progress = 1 − real_token / 793,1e12` (pump.fun) | ✅V4 |
| `curve_speed = Δprogress / Δt` (60 s); `eta_migration_s = (1 − progress) / curve_speed` | 🟡V5 |
| Event migrasi sebagai pemicu tersendiri | ❌ |
| Metadata: link X/Telegram/website, nama/gambar jiplakan, profil/boost berbayar DexScreener, community takeover | ❌ |

---

## 11. Wallet clustering [U5] + aljabar linear

**[U5] Wallet Clustering** (dari user)
- **Metode:** kelompokkan wallet berdasarkan bukti hubungan, misalnya sumber pendanaan bersama dan pola transaksi yang berulang.
- **Cara membaca:** jika banyak buyer berasal dari cluster yang sama, jangan langsung menganggapnya sebagai pertumbuhan
  pengguna independen. Hubungan wallet adalah indikasi, bukan bukti pasti satu pemilik.

**Implementasi** (gabungan tiga sumber bukti, masing-masing dengan skor keyakinan)
1. **Funding graph** (❌): transfer SOL antar-wallet. Wallet yang didanai sumber yang sama dalam X lompatan dan jangka
   waktu tertentu → satu komponen.
2. **Co-buying** (🟡V5: connected components dari pasangan wallet dengan ≥ 3 koin yang sama, offline):
   - Matriks `A ∈ {0,1}^(koin × wallet)`, `A_jw = 1` jika wallet w membeli koin j dalam 5 menit pertama koin itu.
   - Co-buying `C = AᵀA` (wallet × wallet); kemiripan kosinus `S_uv = C_uv / sqrt(C_uu·C_vv)`.
   - **Spectral clustering** (❌): `L_sym = I − D^(−1/2)·S·D^(−1/2)` (D = matriks derajat dari S). Ambil k vektor
     eigen dengan nilai eigen terkecil, normalisasi per baris, lalu k-means → cluster.
   - Pakai matriks sparse; hanya wallet dengan ≥ 3 koin.
3. **Perilaku** (❌): beli di slot yang sama, ukuran yang sama, pola waktu yang sama.

Hasil akhirnya `c(i)` (cluster id + keyakinan), dipakai di Bagian 4–8.

**Aljabar linear lain (alat, bukan model utama):**
- **PageRank risiko** pada funding graph (Bagian 12).
- **PCA regime pasar**: `z = Wᵀ(x − μ)`, W = vektor eigen teratas dari kovarians fitur lintas koin. ❌
- **Kyle λ** (F6), dan regresi logistik sebagai baseline (M11).

Model utama tetap berbasis tree (CatBoost), karena pasar ini non-linear, berekor tebal, dan regime-nya cepat berubah.

---

## 12. Saringan keras: creator-family, rug, honeypot [U7]

**[U7] Creator-Family Risk** (dari user)
- **Metode:** lacak penjualan creator dan wallet terkait secara kumulatif; gabungkan dengan perubahan liquidity dan riwayat creator.
- **Cara membaca:** penjualan besar/kumulatif atau liquidity yang ditarik dapat menjadi tanda bahaya. Tentukan aturan
  darurat berdasarkan bukti on-chain dan uji false positive. Jangan mengandalkan satu transaksi saja.
- Cocok dengan bug V5-8: V5 hanya memicu kill switch dari **satu** penjualan creator ≥ 50%; dev bisa jual 49%
  berkali-kali atau lewat wallet lain.

| Komponen | Rumus / aturan | Status |
|---|---|---|
| Kill switch kumulatif keluarga dev | total jual (creator + keluarga) sejak create ≥ X% dari kepemilikan awal | ❌ (V4: creator jual apa pun; V5: satu jual ≥ 50%) |
| Penarikan likuiditas | quote reserve turun > `lp_drop_frac` dalam 60 s (pool AMM) | 🟡V5 `LpDrop` |
| Riwayat creator | jumlah koin sebelumnya, porsi yang mati < 1 jam, SOL yang ditarik → blacklist rugger berulang | ❌ |
| Sumber dana creator | wallet baru yang didanai rugger yang dikenal vs dari exchange | ❌ |
| Bundle saat create | buy di slot create dari keluarga dev | ✅V4 `f2_bundle_buy_share` (level tx); versi keluarga dev ❌ |
| PageRank risiko | `r = (1 − α)·r₀ + α·Pᵀ·r`; P = adjacency transfer SOL yang dinormalisasi per baris, r₀ = rugger yang dikenal, α ≈ 0,85, iterasi sampai konvergen | ❌ |
| Blacklist lintas koin | wallet/cluster dev yang terbukti rug diblokir di semua koin berikutnya | ❌ |

**Honeypot (❌ semua).** Token pump.fun standar biasanya tanpa mint/freeze authority, tapi venue lain dan token
Token-2022 bisa jadi honeypot. Cek per token, jangan diasumsikan:
- mint authority harus `null`; freeze authority harus `null`;
- ekstensi Token-2022: tolak `TransferHook`, `PermanentDelegate`, `DefaultAccountState = Frozen`, `NonTransferable`,
  `Pausable`; `TransferFee` dihitung sebagai biaya, dan ditolak jika di atas batas;
- **simulasi jual** (`simulateTransaction` dengan jumlah kecil) harus berhasil dan hasilnya cocok dengan pool math;
- bukti on-chain: ≥ N wallet yang bukan dev dan bukan keluarga dev sudah berhasil menjual;
- pajak efektif: hasil jual nyata vs pool math, dicek di setiap jual;
- program pemilik pool harus program resmi dari `config/venues.toml`.

---

## 13. Label [U8]

**[U8] Moonshot Labeling** (dari user)
- **Metode:** labeli token berdasarkan apakah token mencapai target return dalam horizon waktu yang ditentukan.
- **Contoh label:** `1` jika harga eksekusi yang realistis mencapai +500% dalam 24 jam setelah waktu observasi;
  selain itu `0`.
- **Cara membaca:** target dan horizon harus ditetapkan sebelum training. Gunakan hanya informasi yang tersedia pada
  waktu observasi, pisahkan train/validation/test berdasarkan waktu, dan masukkan biaya/slippage bila label berbasis
  hasil transaksi. Contoh +500%/24 jam hanya ilustrasi.

**Aturan implementasi**
1. **Harga eksekusi, bukan harga marginal.** Untuk ukuran posisi yang direncanakan (`bet_sol`):
   - token didapat saat entry pada t + latency: `tok = T·Δq(1−f) / (Q + Δq(1−f))`;
   - nilai jual pada waktu u: `V_u = Q_u·tok(1−f) / (T_u + tok(1−f))`, di curve dibatasi `real_quote`;
   - label `y = 1` jika `max_u V_u / bet_sol ≥ 6` (= +500%) dalam horizon.
   - Status ❌: label V5 memakai harga marginal pool.
2. **Horizon butuh data.** 24 jam butuh data follow 24 jam; V5 baru 10 jam (`H_S = 36000`). Pilih: horizon 10 jam,
   atau perpanjang follow setelah biaya kredit Helius diukur.
3. **Sensor:** baris dipakai hanya jika data mencakup `[t, t + H]` atau koin mati lebih dulu (aturan V5 §3.2).
   Laporkan jumlah baris yang dibuang.
4. **Versi barrier** (selaras dengan exit): `y = 1` jika +500% tercapai **sebelum** turun ke −X%.
5. **Label pre-pump** (selaras dengan keputusan user no. 7): hanya pada kondisi harga datar
   (`abs(ret_60s) ≤ ε` dan `D ≤ 1 + δ`): `y = 1` jika harga naik ≥ X dalam N menit sebelum turun ≤ −Y. ❌
6. **Hazard lepas landas:** `h_t = P(lepas landas dalam (t, t+Δ] ∣ belum lepas landas) = σ(βᵀx_t)`.
   ❌ (yang ada hanya hazard **mati** `y_dead_10m` 🟡V5)
7. **Label V5** (`y_bin` 7 kelas, `y_tp1`, `y_hold`, `y_logmax`) 🟡V5. Barrier −65% di `y_bin` perlu dicek dengan
   data (bug V5-9).
8. Target dan horizon ditetapkan **sebelum** training dan tidak diubah setelah melihat hasil uji.

---

## 14. Model, kalibrasi, EV [U9]

**[U9] Probability Calibration + Expected Value** (dari user)
- **Rumus:** `EV = P_w × R_w − P_l × R_l − C`. P_w = probabilitas menang; P_l = probabilitas rugi; R_w = return bersih
  jika menang; R_l = besaran kerugian jika rugi (positif); C = biaya dan slippage yang belum masuk ke return.
- **Cara membaca:** EV positif berarti estimasi nilai harapan positif berdasarkan asumsi model, bukan jaminan profit.
  Probabilitas perlu dikalibrasi dan EV diuji out-of-sample dengan biaya realistis.

**Catatan**
- Dua hasil (menang/rugi) terlalu kasar untuk distribusi moonshot yang berekor tebal. Pakai versi banyak kelas
  `EV = Σ_k P_cal(bin_k)·R_k − C` (V5). Rumus U9 adalah kasus khususnya.
- Jika `P_w + P_l < 1` (ada hasil netral), hasil netral harus ikut dihitung.
- **Jangan hitung biaya dua kali.** R_k di V5 sudah bersih dari biaya, jadi C = 0 di sana.
- R harus diukur dengan **kebijakan exit yang sama**, hanya dari window training.
- EV posisi yang sedang dipegang harus memakai **satuan yang sama** dengan EV kandidat (bug V5-7).

| ID | Komponen | Rumus / aturan | Status |
|---|---|---|---|
| M1 | Meta-labeling | radar = sinyal utama (recall tinggi); model = ambil / lewati | 🟡V5 (baris keputusan hanya saat `Promote`, bug V5-2) |
| M2 | Label pre-pump | Bagian 13 no. 5 | ❌ |
| M3 | Hazard lepas landas | Bagian 13 no. 6 | ❌ |
| M4 | Label bin / barrier / hold | `y_bin`, `y_tp1`, `y_hold` | 🟡V5 |
| M5 | Ketidakseimbangan kelas | class weight; sample weight `1 / jumlah_baris_per_koin` | 🟡V5 |
| M6 | Kalibrasi | isotonic; `Brier = (1/N)·Σ (p − y)²`; error reliabilitas per bin `abs(mean p − mean y) < 0,05` | 🟡V5 (belum dijalankan) |
| M7 | Kualitas ranking | presisi top 1% untuk ≥10x; moonshot capture | 🟡V5 (belum dijalankan) |
| M8 = [U9] | Expected value | `EV = Σ_k P_cal(bin_k)·R_k − C` (dua hasil: `P_w·R_w − P_l·R_l − C`) | 🟡V5 versi bin; satuan EV posisi ❌ |
| M9 | Sizing | tetap (`bet_sol`) sampai M6 lolos; sesudahnya `f* = argmax_f Σ_k P_k·ln(1 + f·R_k)`, pakai `0,25·f*`, maksimal 0,10 | 🟡V5 rencana (belum dipakai) |
| M10 | Monitor drift | `PSI = Σ_i (a_i − e_i)·ln(a_i / e_i)` per fitur (a = porsi live, e = porsi training per bin); alarm jika > 0,25 | ❌ |
| M11 | Baseline | regresi logistik; CatBoost wajib mengalahkannya | 🟡V5 |

---

## 15. Ranking dan entry [U10]

**[U10] Adaptive Ranking** (dari user)
- **Metode:** beri skor kandidat dari flow, buyer growth, momentum, risiko creator, liquidity, dan estimasi EV.
- **Cara membaca:** urutkan kandidat berdasarkan skor yang sudah diuji. Pastikan fitur dinormalisasi/ditimbang dengan
  benar dan fitur risiko dapat menurunkan ranking. Ranking bukan keputusan beli otomatis tanpa entry gate.

**Implementasi**
1. **Langkah keputusan** tiap 2–5 detik: kumpulkan semua kandidat, beri skor, urutkan.
2. **Normalisasi robust:** persentil di dalam satu langkah, atau z-score dengan median/MAD dari window yang lewat.
3. **Bobot:** dipelajari model (EV, M8), dilatih ulang terjadwal secara walk-forward. **Tidak pernah** di-tune di data uji.
4. **Risiko:** menurunkan skor; risiko berat = veto (Bagian 12).
5. **Gate entry** (semua wajib lolos): saringan keras, batas harga P5, `EV ≥ ev_min`, cek likuiditas keluar (E3).
6. **Slot dan exposure diterapkan setelah ranking** (bug V5-1). Ganti posisi terlemah jika EV kandidat lebih tinggi
   `replace_margin`, dalam satuan yang sama (V5-7).
7. **Status: ❌.** V4 = siapa cepat dia dapat; V5 = ranking hanya untuk penggantian, dan tidak pernah jalan karena V5-1.

**Status koin: WATCH → ARM → ENTRY** (keputusan user no. 7: beli sebelum naik)

| Status | Syarat | Aksi |
|---|---|---|
| WATCH | alarm CUSUM (Bagian 7) atau sinyal awal saat harga masih datar: perpindahan supply (H7), smart money masuk (F9), pembelian dipecah (C10), cluster asli bertambah (C1/C2), penjual habis, rentang menyempit (P3), wallet yang baru didanai membeli, katalis (Bagian 10) | pantau ketat |
| ARM | sinyal menguat DAN saringan keras lolos | siapkan transaksi (E5) |
| ENTRY | dorongan pertama terkonfirmasi DAN `D ≤ 1 + max_entry_ret` | kirim transaksi |
| Time stop | setelah entry, tidak bergerak dalam N menit | keluar (rugi kecil karena masuk dekat harga dasar) |

Konsekuensinya: sinyal palsu lebih banyak. Karena itu size kecil dan tetap, posisi banyak, saringan rug/honeypot ketat.
Koin yang baru lahir adalah adu kecepatan dengan sniper lain; edge utama cara ini ada di **koin lama yang sedang
diakumulasi**.

---

## 16. Exit [U11–U14] — keputusan user: jual 100%, tanpa jual setengah

**[U11] ATH / Peak Trailing Stop** (dari user)
- **Rumus:** `P_exit = P_peak × (1 − d)`. P_peak = harga tertinggi sejak entry; d = jarak trailing dalam desimal (0,05 = 5%).
- **Contoh:** peak 5×, trailing 5% → batas exit `5 × (1 − 0,05) = 4,75×`.
- **Cara membaca:** jika harga menyentuh/menembus batas, picu exit sesuai aturan eksekusi. Perbarui peak ketika harga
  membuat high baru. Harga eksekusi aktual bisa lebih buruk akibat slippage/latency.
- **Status:** ✅V4 (`trailing_only` di `backtest.rs:501`; `GiveBackFromPeak` di `ml_sim.rs:989`), 🟡V5 `Trail`.
- **Bukti data (4 window, aturan entry V4 yang sama):** trailing 30% saja hasilnya **−58,1%, −47,9%, −42,4%, −23,7%**
  (`reports/backtest_exploratory.json`, `baseline_trailing30`). Trailing 5% dari contoh hampir pasti keluar karena
  noise biasa. Angka 5% hanya ilustrasi; **jangan dipakai tanpa diuji**.

**[U12] Adaptive Volatility Trailing** (dari user)
- **Rumus:** `d_t = clamp(k × σ_t, d_min, d_max)`. σ_t = estimasi volatilitas pada jendela tertentu; k = pengali;
  d_min, d_max = batas minimum dan maksimum; `clamp(x, a, b)` membatasi x di antara a dan b.
- **Cara membaca:** volatilitas lebih tinggi memberi trailing lebih lebar; volatilitas lebih rendah memberi trailing
  lebih ketat. Uji agar tidak terlalu lebar (profit kembali) atau terlalu ketat (keluar terlalu dini).
- **Status:** 🟡V5. V5 memakai `trail = max(bag_trail, trail_k·vol_60s)`, +0,1 jika P(hold) ≥ `hold_min`. Hanya ada
  batas bawah, tanpa `d_max`.
- **Koreksi satuan:**
  - V5 `vol_60s` = std log return **1 detik**. Untuk jarak trailing di horizon h detik, skalakan:
    `σ_h = σ_1s·√h`. Ini mengasumsikan return independen, padahal return memecoin tidak independen; anggap k sebagai
    parameter yang di-tune.
  - σ adalah skala log. Ubah ke pecahan dengan `d = 1 − exp(−k·σ_h)`, supaya d selalu < 1. Setelah itu baru di-clamp.

**[U13] Momentum-Based Exit** (dari user)
- **Logika contoh:** exit normal jika drawdown dari peak melewati trailing threshold **dan** sinyal momentum melemah,
  misalnya flow berbalik negatif serta buyer acceleration turun.
- **Cara membaca:** gunakan kombinasi yang telah diuji untuk mengurangi exit karena noise. Jika trailing stop ditetapkan
  sebagai batas wajib, sinyal momentum tidak boleh membatalkan batas tersebut.
- **Status: sudah ada di V4 sebagai MEE** (`backtest.rs:503-504`):
  `(fast < 0,25·peak_fast DAN flow30 < 0) ATAU (drawdown ≥ mee_drawdown DAN fast < 0,5·peak_fast)`.
- **Bukti data:** MEE hasilnya **−49,9%, −50,2%, −84,4%, −57,4%**. Menurut report V5, runner 37x dijual MEE di 4,1x.
  Trailing maupun MEE sama-sama rugi. Penyebab utamanya entry yang buruk (file forensik), tapi exit-nya juga terlalu
  sensitif: tanpa syarat bertahan beberapa detik, tanpa komposisi trader, dan memakai jumlah buy, bukan cluster.

**Rekomendasi: "Momentum Fade" (jual 100%)** — diukur sejak entry, trade asli, per cluster

| Sinyal | Definisi |
|---|---|
| Sepi | laju beli EWMA (τ 10 s) < `fade_rate_frac` × puncaknya sejak entry |
| Size mengecil | rata-rata SOL beli 30 s < `fade_size_frac` × puncaknya sejak entry |
| Penjual dominan | SOL jual 30 s > SOL beli 30 s |
| Keramaian turun | percepatan cluster C11 negatif, atau G (C2) < 1 |
| Mati | tidak ada beli asli selama `idle_s` |

**Aturan:** jual 100% jika **Penjual dominan DAN (Sepi ATAU Size mengecil ATAU Keramaian turun)** bertahan ≥
`fade_persist_s`, atau **Mati**. Syarat bertahan ada karena moonshot sering dump dulu baru terbang.

**Varian yang wajib dibandingkan** di window uji yang sama (pilih berdasarkan data, bukan asumsi):

| Varian | Isi |
|---|---|
| A | Momentum Fade saja |
| B | Momentum Fade DAN drawdown ≥ d_t (U12) — bentuk U13 yang diperbaiki |
| C | Peak trailing saja (U11) dengan d hasil tuning dev |
| D | Ladder TP1/TP2 V5 — **hanya pembanding**, dan hanya jika user setuju (bertentangan dengan keputusan no. 8) |

Exit model V4 lama (`DumpPredicted`, `StrongReversal`, `MomentumExhausted`, `Scout`) dimatikan saat mode ini aktif.

**[U14] Emergency Exit** (dari user)
- **Logika:** keluar atau hentikan penambahan posisi jika terdeteksi kondisi bahaya berat, misalnya liquidity ditarik,
  creator-family melakukan penjualan berbahaya, atau token tidak dapat dijual secara normal.
- **Cara membaca:** jalur darurat terpisah dari exit normal. Uji latensi, transaksi gagal, dan kondisi saat liquidity
  sangat tipis; tidak ada jaminan exit berhasil pada harga yang diinginkan.

| Pemicu darurat | Status |
|---|---|
| Kill switch kumulatif keluarga dev (Bagian 12) | ❌ |
| Likuiditas ditarik (`LpDrop`) | 🟡V5 |
| Token tidak bisa dijual normal (simulasi jual gagal saat memegang, pajak efektif melonjak) | ❌ |
| Model rug ≥ `rug_veto`; hazard mati ≥ `dead_exit` | 🟡V5 |
| Umur posisi ≥ `max_hold_s` | 🟡V5 |
| Jalur cepat: latensi 1 slot untuk jual darurat | ✅V4 / 🟡V5 (untuk `DevSell`) |

Batas darurat **tidak pernah** dibatalkan oleh sinyal momentum (sesuai catatan U13).

---

## 17. Eksekusi realistis [U15]

**[U15] Realistic Execution Simulation** (dari user)
- **Metode:** simulasikan harga masuk/keluar, latency, slippage, biaya DEX/network, transaksi gagal, dan liquidity
  yang tersedia.
- **Cara membaca:** nilai hasil bersih, profit factor, drawdown, moonshot capture, dan hasil pada periode out-of-sample.
  Jangan menganggap backtest valid jika memakai harga ideal yang tidak dapat dieksekusi.

**Yang sudah disimulasikan (✅V4 / 🟡V5):** latensi tetap 2 slot; fee per pool; slippage CPMM; biaya transaksi tetap
0,0005 SOL; gagal beli 15% (deterministik); order terisi di urutan terakhir slot (konservatif); jual sebagian membayar
biaya sendiri (V5).

| ID | Komponen | Rumus / aturan | Status |
|---|---|---|---|
| E1 | Anggaran latensi | `t_terima − t_awal_slot`, waktu keputusan, kirim, masuk blok; laporkan p50 / p99 | ❌ mainnet (hanya devnet G0.5) |
| E2 | Fill CPMM | `tokens_out = T·Δq(1−f) / (Q + Δq(1−f))`; dampak ≈ `Δq / Q` → batas size `Δq ≤ I_max·Q` | ✅V4 |
| E3 | Cek likuiditas keluar | nilai posisi `V = Q·t(1−f) / (T + t(1−f))` (curve: dibatasi `real_quote`); entry hanya jika `V ≥ (1 − x)·biaya` | nilai ✅V4, cek sebelum entry ❌ |
| E4 | Peluang transaksi masuk | `P(masuk ∣ priority fee, tip, kepadatan)` dari log shadow | ❌ (backtest pakai 15% tetap) |
| E5 | Jalur cepat | transaksi disiapkan saat ARM, Jito bundle, priority fee dinamis, simulasi sebelum kirim | ❌ |
| E6 | Distribusi latensi | latensi acak dari pengukuran, bukan angka tetap | ❌ |
| E7 | Persaingan / MEV | risiko sandwich dan front-run, sniper lain di slot yang sama, batas slippage | ❌ |
| E8 | Biaya Token-2022 | `TransferFee` di setiap transfer/jual | ❌ |
| E9 | Risiko akun | batas rugi harian, kill switch setelah rugi beruntun, batas exposure per jam | ❌ |
| E10 | Umpan balik | keputusan shadow vs hasil nyata; update database wallet harian; monitor drift (M10) | ❌ |

**Sensitivitas wajib dilaporkan:** latensi 1 / 2 / 4 slot, gagal beli 0,15 / 0,30, tip ×2.

---

## 18. Validasi khusus prediksi (anti-overfitting)

1. **Split berdasarkan waktu:** train < kalibrasi < uji, dengan purge ≥ H di antaranya.
2. **Holdout** (`final_holdout` di `config/sample_windows.json`) hanya dijalankan **sekali**, setelah semua dibekukan.
3. **Grid ≤ 24 kombinasi**; pilih berdasarkan **median** window dev. Aturan pemilihan ditetapkan sebelum melihat hasil.
4. **Ablation per kelompok komponen** (F, C, H, P, CUSUM, cluster, saringan): dipertahankan hanya jika expectancy
   strategi out-of-sample naik setelah biaya, di mayoritas window uji.
5. **Selalu laporkan n.** Hasil dengan < 30 kejadian hanya "arah", bukan bukti.
6. **Expectancy dengan dan tanpa 1% trade teratas**, supaya satu moonshot beruntung tidak menutupi hasil.
7. **Metrik prediksi yang wajib dilaporkan:**
   - presisi top 1% untuk ≥10x, dan moonshot capture (≥10x yang dipegang sampai ≥5x);
   - median harga entry vs dasar (D), dan porsi entry sebelum +10%;
   - kalibrasi (tabel reliabilitas, Brier);
   - CUSUM: jeda deteksi pada runner dan alarm palsu pada non-runner;
   - dipisah: koin lama vs koin baru, dan per venue.
8. **Kriteria lulus** = file forensik Bagian 11 (expectancy tanpa top 1% > 0, PF > 1,3, ≥ 60% window uji positif,
   capture ≥ 20%, tetap positif di latensi 4 slot dan gagal beli 0,30).
9. Jika gagal: laporkan bagian yang gagal dan berhenti. Kriteria tidak boleh dilonggarkan tanpa izin tertulis user.

---

## 19. Prioritas implementasi untuk akurasi

| # | Kelompok | Komponen |
|---|---|---|
| 1 | **Siapa yang beli** | cluster (Bagian 11), C1, arus smart money F9, arus keluarga dev F10, supply keluarga dev H3 |
| 2 | **Momen keputusan yang benar** | discovery koin baru dari event create (Bagian 3), CUSUM (Bagian 7), baris keputusan berulang (V5-2), label pre-pump (Bagian 13), batas harga P5 |
| 3 | **Supply asli** | holder dari chain H1, transfer keluar keluarga dev H8 |
| 4 | **Arus ÷ likuiditas + keramaian** | F2, F4, F11, C2, C11, C6 |
| 5 | **Volume data** | data kontinu ≥ 4 minggu (file forensik 6.4) |
| 6 | **Kecepatan** | gRPC berbayar + jalur cepat (E1, E5) |

Kaitan dengan urutan kerja di file forensik: Fase 2 (radar) = Bagian 3–11 dan 15; Fase 3 (safety) = Bagian 12;
Fase 5 (label + model) = Bagian 13–14; Fase 6 (strategi) = Bagian 15–16; Fase 8 (live) = Bagian 17.

---

## 20. Ekspektasi jujur

- Belum ada yang terbukti. Semua komponen baru menjadi bukti setelah lolos walk-forward di cukup banyak window (Bagian 18).
- Target realistis: **peluang** menangkap moonshot naik, bukan kepastian. Contohnya: ≥ 20% runner ≥10x dipegang sampai
  ≥5x, dengan expectancy positif tanpa 1% trade teratas.
- Sebagian moonshot dipicu perhatian yang datang **setelah** entry, dan sebagian lagi lompat dalam hitungan slot.
  Keduanya tidak sepenuhnya bisa diprediksi dari data on-chain saat entry.

---

## Lampiran A — Peta 15 rumus user → bagian → status

| # | Rumus user | Di file ini | Status |
|---|---|---|---|
| U1 | Multi-DEX Discovery | Bagian 3 | ❌ venue lain; 🟡V5 event create; ✅V4 awakening (tertutup bug V4-1) |
| U2 | Signed Money Flow `F = B − S` | Bagian 5, F1 | ✅V4 |
| U3 | Flow Imbalance | Bagian 5, F11 | ✅V4 (60 s, tanpa debu) |
| U4 | Buyer Acceleration | Bagian 6, C11 | ❌ bentuk selisih per cluster; 🟡V5 rasio per wallet |
| U5 | Wallet Clustering | Bagian 11 | 🟡V5 co-buying offline; funding graph dan spectral ❌ |
| U6 | CUSUM / Change-Point | Bagian 7 | ❌ |
| U7 | Creator-Family Risk | Bagian 12 | ❌ kumulatif keluarga; 🟡V5 satu jual creator + `LpDrop` |
| U8 | Moonshot Labeling | Bagian 13 | 🟡V5 label harga marginal 10 jam; harga eksekusi / 24 jam ❌ |
| U9 | Calibration + EV | Bagian 14, M6 + M8 | 🟡V5 (belum dijalankan) |
| U10 | Adaptive Ranking | Bagian 15 | ❌ (V5 tidak pernah jalan karena V5-1) |
| U11 | Peak Trailing Stop | Bagian 16 | ✅V4 / 🟡V5 — trailing 30% rugi di 4 window |
| U12 | Adaptive Volatility Trailing | Bagian 16 | 🟡V5 (tanpa `d_max`, satuan σ perlu dikoreksi) |
| U13 | Momentum-Based Exit | Bagian 16 | ✅V4 sebagai MEE — rugi di 4 window; diganti Momentum Fade + varian |
| U14 | Emergency Exit | Bagian 16 | 🟡V5 sebagian; kumulatif keluarga dan cek "bisa dijual" ❌ |
| U15 | Realistic Execution Simulation | Bagian 17 | ✅V4 / 🟡V5 sebagian; E1, E3–E10 ❌ |

## Lampiran B — Koreksi dan catatan atas `MEME_COIN_QUANT_FORMULAS.md`

1. **U3:** FI tidak tahu ukuran → selalu dipasangkan dengan arus ÷ likuiditas (F2) dan churn (F3).
2. **U4:** dihitung per cluster, dalam dua bentuk (selisih dan rasio).
3. **U6:** baseline kausal dan robust (median/MAD), z-score, sampling waktu tetap, warm-up, baseline populasi untuk
   koin baru. Evaluasi dengan jeda deteksi vs alarm palsu.
4. **U8:** label memakai harga eksekusi untuk ukuran posisi nyata. Horizon 24 jam butuh data follow 24 jam (V5 baru
   10 jam).
5. **U9:** jangan hitung biaya dua kali; pakai versi banyak kelas; EV posisi dan kandidat dalam satuan yang sama.
6. **U11:** contoh 5% terlalu ketat untuk memecoin; trailing 30% saja sudah rugi −24% sampai −58% di 4 window.
7. **U12:** skala σ ke horizon trailing (`σ_h = σ_1s·√h`) dan ubah dari skala log ke pecahan
   (`d = 1 − exp(−k·σ_h)`); tambahkan `d_max`.
8. **U13:** logika ini sudah ada di V4 sebagai MEE dan rugi −50% sampai −84% per window. Perlu syarat bertahan
   beberapa detik, komposisi trader per cluster, dan perbandingan varian A–D.
9. **Umum:** di satu pool CPMM, harga ditentukan langsung oleh arus bersih (`Δln p ≈ 2·NF/Q`). "Arus masuk tapi harga
   datar" tidak mungkin terjadi di satu pool; akumulasi dibaca dari komposisi pembeli/penjual dan perpindahan supply.
10. **U14:** tambahkan cek "token masih bisa dijual" saat memegang posisi (simulasi jual, pajak efektif).

## Lampiran C — Rujukan bug di file forensik

V4-1 (koin baru salah label), V4-4 / V5-13 (likuiditas virtual), V4-10 (dua engine), V5-1 (exposure sebelum
ranking), V5-2 (keputusan hanya saat `Promote`), V5-7 (satuan EV), V5-8 (kill switch dev satu transaksi),
V5-9 (barrier −65%), V5-12 (flag bot), V5-15 (regime hanya pump.fun). Detail dan lokasi kode ada di file forensik
Bagian 4.
