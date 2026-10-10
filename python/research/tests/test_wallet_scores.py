"""Synthetic tests for python/research/wallet_scores.py. Run: python3 -m pytest python/research/tests/  or
python3 python/research/tests/test_wallet_scores.py"""
import json
import math
import os
import sys
import tempfile

import pandas as pd
import zstandard

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import wallet_scores as ws  # noqa: E402

DAY0 = 19675 * 86400
TA = DAY0 + 3600            # dir A window start
TB = TA + 7200              # dir B window start
SLOT0 = 300_000_000
HEADER = ("slot,block_time,order,mint,venue,side,sol_lamports,token_raw,trader,creator,quote_reserve_after,"
          "token_reserve_after,real_quote,real_token,fee_bps,curve_complete")
SOL = 1_000_000_000


LAUNCH = 30 * SOL   # pump.fun curve virtual SOL reserve at launch


class Rec:
    def __init__(self, names=("A", "B")):
        self.rows = {n: [] for n in names}
        self.seq = 0

    def add(self, d, t, mint, side, sol, tok, trader, slot=None, venue="pumpfun_curve", qres=1, tx=None):
        """qres = quote_reserve_after; tx = explicit intra-slot tx index (default: a global counter)."""
        slot = SLOT0 + 2 * (t - TA) if slot is None else slot
        self.seq += 1
        tx = self.seq if tx is None else tx
        self.rows[d].append((slot, t, (slot << 24) | (tx << 8), mint, venue, side, sol, tok, trader, qres))

    def dev(self, d, t, mint, pre=LAUNCH, venue="pumpfun_curve"):
        """Dev buy as the mint's first trade; pre = pre-trade quote reserve (launch value by default)."""
        self.add(d, t, mint, "buy", SOL // 10, 100, f"dev_{mint}", venue=venue, qres=pre + SOL // 10)

    def write(self, root):
        dirs = {}
        for d, rows in self.rows.items():
            p = os.path.join(root, f"v5_{d}")
            os.makedirs(p)
            rows = sorted(rows, key=lambda r: r[2])
            lines = [HEADER] + [f"{s},{t},{o},{m},{v},{sd},{sol},{tok},{tr},creator,{q},1,1,1,100,0"
                                for s, t, o, m, v, sd, sol, tok, tr, q in rows]
            text = "\n".join(lines) + "\n"
            if d == "B":   # exercise the .zst reader
                with open(os.path.join(p, "trades.csv.zst"), "wb") as f:
                    f.write(zstandard.ZstdCompressor().compress(text.encode()))
            else:
                with open(os.path.join(p, "trades.csv"), "w") as f:
                    f.write(text)
            dirs[d] = p
        return dirs


def build(root):
    r = Rec()
    r.dev("A", TA, "Manchor")                                     # defines A's window start
    r.dev("B", TB, "Mbanchor")                                    # defines B's window start
    # causality: closes 1 s before / 1 s after B's start (both trades live in A's follow-up)
    r.dev("A", TA + 5, "Mc1"); r.dev("A", TA + 5, "Mc2")
    r.add("A", TB - 31, "Mc1", "buy", SOL, 1000, "Wbefore")
    r.add("A", TB - 1, "Mc1", "sell", 2 * SOL, 1000, "Wbefore")
    r.add("A", TB - 30, "Mc2", "buy", SOL, 1000, "Wafter")
    r.add("A", TB + 1, "Mc2", "sell", 2 * SOL, 1000, "Wafter")
    r.add("B", TB + 1, "Mc2", "sell", 2 * SOL, 1000, "Wafter")   # duplicate copy of the same event in B
    r.rows["B"][-1] = r.rows["B"][-1][:2] + (r.rows["A"][-1][2],) + r.rows["B"][-1][3:]
    # partial sells: closes only when balance <= 1% of max; then a second trip
    r.dev("A", TA + 5, "Mp")
    r.add("A", TA + 100, "Mp", "buy", SOL, 1000, "Wpartial")
    r.add("A", TA + 200, "Mp", "sell", 600_000_000, 500, "Wpartial")
    r.add("A", TA + 300, "Mp", "sell", 500_000_000, 489, "Wpartial")    # balance 11 = 1.1% -> open
    r.add("A", TA + 400, "Mp", "sell", 1_000_000, 1, "Wpartial")        # balance 10 = 1.0% -> close
    r.add("A", TA + 500, "Mp", "buy", SOL, 1000, "Wpartial")
    r.add("A", TA + 520, "Mp", "sell", SOL // 2, 1000, "Wpartial")
    r.add("A", TA + 100, "Mp", "buy", SOL, 1000, "Wpartial2")
    r.add("A", TA + 200, "Mp", "sell", SOL, 989, "Wpartial2")           # balance 11 -> never closes
    r.add("A", TA + 50, "Mp", "sell", SOL, 5000, "Wnoopen")              # sell while flat: ignored
    # bots
    for k in range(5):
        m = f"Mf{k}"; r.dev("A", TA + 5, m)
        r.add("A", TA + 1000 + 10 * k, m, "buy", SOL, 100, "Wfast")
        r.add("A", TA + 1005 + 10 * k, m, "sell", SOL, 100, "Wfast")
    for name, n in (("Wmany", 60), ("Wfifty", 50)):
        for k in range(n):
            m = f"M{name}{k}"; r.dev("A", TA + 5, m)
            r.add("A", TA + 2000 + 20 * k, m, "buy", SOL, 100, name)
            r.add("A", TA + 2030 + 20 * k, m, "sell", SOL, 100, name)
    # sniper newborn rule
    r.dev("A", TA + 100, "Mn1"); r.add("A", TA + 101, "Mn1", "buy", SOL, 10, "Wsnipe")          # slot +2: yes
    r.dev("A", TA + 10, "Mold"); r.add("A", TA + 10, "Mold", "buy", SOL, 10, "Wsnipe")          # not newborn
    r.dev("A", TA + 200, "Mn2"); r.add("A", TA + 201, "Mn2", "buy", SOL, 10, "Wsnipe",
                                       slot=SLOT0 + 2 * 200 + 3)                                 # slot +3: no
    # first seen >= 60 s into A but not at launch: mid-life curve (45 SOL reserve) / PumpSwap pool at 30 SOL
    r.dev("A", TA + 400, "Mmid", pre=45 * SOL); r.add("A", TA + 400, "Mmid", "buy", SOL, 10, "Wsnipe2")
    r.dev("A", TA + 500, "Mswap", venue="pumpswap"); r.add("A", TA + 500, "Mswap", "buy", SOL, 10, "Wsnipe2")
    r.dev("A", TA + 600, "Mtol", pre=LAUNCH + LAUNCH // 200)                         # +0.5%: still a launch
    r.add("A", TA + 600, "Mtol", "buy", SOL, 10, "Wsnipe3")
    r.dev("B", TB + 100, "Mn3"); r.add("B", TB + 100, "Mn3", "buy", SOL, 10, "Wsnipe")          # after B start
    # co-buying cluster
    for m in ("K1", "K2", "K3", "K4"):
        r.dev("A", TA + 5, m)
    for w in ("C3", "C1", "C2", "Wfast"):   # Wfast is a bot -> must be excluded
        for m in ("K1", "K2", "K3"):
            r.add("A", TA + 60, m, "buy", SOL, 10, w)
    for m in ("K1", "K2", "K4"):
        r.add("A", TA + 60, m, "buy", SOL, 10, "C4")
    for m in ("K1", "K2", "K3"):
        r.add("A", TA + 305, m, "buy", SOL, 10, "C5")    # 300 s after first trade: not early
    for m in ("L1", "L2", "L3"):                          # cluster formed after B's start: not used for B
        r.dev("B", TB + 5, m)
        for w in ("D1", "D2", "D3"):
            r.add("B", TB + 10, m, "buy", SOL, 10, w)
    return r.write(root)


_CACHE = {}


def scored():
    if not _CACHE:
        root = tempfile.mkdtemp(prefix="ws_test_")
        dirs = build(root)
        ws.run([dirs["A"], dirs["B"]], workers=2)
        for d, p in dirs.items():
            _CACHE[d] = (pd.read_csv(os.path.join(p, "wallet_scores.csv"), keep_default_na=False, na_values=[""]),
                         json.load(open(os.path.join(p, "wallet_scores_meta.json"))))
        _CACHE["dirs"] = dirs
    return _CACHE


def row(d, w):
    df = scored()[d][0]
    r = df[df.wallet == w]
    return None if r.empty else r.iloc[0]


def test_columns_and_window_starts():
    c = scored()
    assert list(c["B"][0].columns) == ws.OUT_COLS
    assert c["A"][1]["window_start"] == TA and c["B"][1]["window_start"] == TB
    assert c["A"][1]["n_trips_used"] == 0 and len(c["A"][0]) == 0   # nothing precedes the first window


def test_causality():
    assert row("B", "Wbefore") is not None and row("B", "Wbefore").n_closed == 1
    assert abs(row("B", "Wbefore").mean_logret - math.log(2)) < 1e-9
    assert row("B", "Wafter") is None


def test_dedupe():
    tr, _, _, _ = ws.load_all([scored()["dirs"]["A"], scored()["dirs"]["B"]])
    w = tr[(tr.t == TB + 1) & (~tr.buy)]
    assert len(w) == 1


def test_partial_sells():
    r = row("B", "Wpartial")
    assert r.n_closed == 2
    lr = [math.log(1.101), math.log(0.5)]
    assert abs(r.mean_logret - sum(lr) / 2) < 1e-9
    assert r.median_hold_s == 160 and r.fast_share == 0 and r.is_flipper == 0
    assert row("B", "Wpartial2") is None and row("B", "Wnoopen") is None


def test_shrinkage():
    meta = scored()["B"][1]
    n_trips = 1 + 2 + 5 + 60 + 50
    mean_all = (math.log(2) + math.log(1.101) + math.log(0.5)) / n_trips
    assert meta["n_trips_used"] == n_trips
    assert abs(meta["mean_logret_all"] - mean_all) < 1e-12
    r = row("B", "Wpartial")
    assert abs(r.score - (2 * r.mean_logret + 10 * mean_all) / 12) < 1e-5
    s = row("B", "C1").score   # no trips -> pure prior
    assert abs(s - mean_all) < 1e-5


def test_bot_flag():
    f = row("B", "Wfast")
    assert f.fast_share == 1 and f.is_bot == 1 and f.is_flipper == 1 and f.n_closed == 5
    assert row("B", "Wmany").is_bot == 1 and row("B", "Wmany").fast_share == 0
    assert row("B", "Wfifty").is_bot == 0
    assert row("B", "Wpartial").is_bot == 0
    assert scored()["B"][1]["n_bots"] == 2


def test_sniper_newborn():
    assert row("B", "Wsnipe").sniper_coins == 1
    assert row("B", "dev_Mn1").sniper_coins == 1
    assert row("B", "dev_Mold") is None and row("B", "dev_Mn3") is None


def test_sniper_newborn_needs_launch_reserve():
    # first observed >= 60 s into the dir, but a mid-life curve or a PumpSwap pool: not a newborn
    assert row("B", "Wsnipe2") is None and row("B", "dev_Mmid") is None and row("B", "dev_Mswap") is None
    assert row("B", "Wsnipe3").sniper_coins == 1 and row("B", "dev_Mtol").sniper_coins == 1


def test_cluster():
    for w in ("C1", "C2", "C3"):
        r = row("B", w)
        assert r.cluster_size == 3 and r.cluster_id == "C1", (w, r.cluster_size, r.cluster_id)
    assert row("B", "C4") is None and row("B", "C5") is None
    assert pd.isna(row("B", "Wfast").cluster_id) and row("B", "Wfast").cluster_size == 1
    assert row("B", "D1") is None
    assert scored()["B"][1]["n_clustered"] == 3


def test_parallel_trips_match_serial():
    tr, _, _, _ = ws.load_all([scored()["dirs"]["A"], scored()["dirs"]["B"]])
    serial = ws.compute_trips(tr, 1)
    old = ws.PARALLEL_MIN_ROWS
    ws.PARALLEL_MIN_ROWS = 0
    try:
        par = ws.compute_trips(tr, 4)
    finally:
        ws.PARALLEL_MIN_ROWS = old
    key = ["w", "m", "open_t", "close_t"]
    a = serial.sort_values(key).reset_index(drop=True)
    b = par.sort_values(key).reset_index(drop=True)
    assert len(a) == 119 and a.equals(b)


def test_cluster_explosion_guard():
    big = pd.DataFrame({"w": list(range(2001)), "m": [0] * 2001})
    trio = pd.DataFrame({"w": [5000, 5001, 5002] * 3, "m": [1] * 3 + [2] * 3 + [3] * 3})
    lab, ignored = ws.clusters(pd.concat([big, trio], ignore_index=True), 6000)
    assert ignored == 1
    assert (lab[:2001] == -1).all() and len(set(lab[5000:5003])) == 1 and lab[5000] >= 0


# ------------------------------------------------------------------ fixes scenario (own dirs A, B, C)
TC = TB + 6000              # dir C window start


def build_fixes(root):
    r = Rec(("A", "B", "C"))
    r.dev("A", TA, "Manchor"); r.dev("B", TB, "Mbanchor"); r.dev("C", TC, "Mcanchor")
    # 1) one on-chain buy in A's follow-up (synthetic tx 0) and in B's window (real tx 57); the sell is only in A
    s = SLOT0 + 2 * (TB + 100 - TA)
    r.add("A", TB + 100, "Md", "buy", SOL, 1000, "Wdup", slot=s, tx=0)
    r.add("B", TB + 100, "Md", "buy", SOL, 1000, "Wdup", slot=s, tx=57)
    r.add("A", TB + 5000, "Md", "sell", 2 * SOL, 1000, "Wdup")
    # 2) two identical buys in the same slot (distinct txs) in A AND in B: 2 trades, not 1 and not 4
    s2 = SLOT0 + 2 * (TB + 200 - TA)
    for d, txs in (("A", (1, 2)), ("B", (70, 71))):
        for tx in txs:
            r.add(d, TB + 200, "Mr", "buy", SOL, 500, "Wrep", slot=s2, tx=tx)
    r.add("A", TB + 300, "Mr", "sell", 4 * SOL, 1000, "Wrep")      # 2 SOL in, 4 SOL out -> ln 2
    # 3) oversell: sells more tokens than the trip bought -> SOL credited pro rata
    r.add("A", TA + 100, "Mo", "buy", SOL, 100, "Wover")
    r.add("A", TA + 200, "Mo", "sell", 10 * SOL, 1000, "Wover")     # credit 1 SOL -> ln 1 = 0
    r.add("A", TA + 100, "Mo", "buy", SOL, 100, "Wover2")
    r.add("A", TA + 150, "Mo", "sell", SOL, 50, "Wover2")
    r.add("A", TA + 250, "Mo", "sell", 5 * SOL, 500, "Wover2")      # balance 50: credit 0.5 SOL -> ln 1.5
    r.add("A", TA + 100, "Mo", "buy", SOL, 1000, "Wover3")
    r.add("A", TA + 300, "Mo", "sell", 1_020_000_000, 1020, "Wover3")  # 2% oversell: credit 1 SOL -> 0
    # 4) zero proceeds: clipped to -LOGRET_CLIP and counted
    r.add("A", TA + 100, "Mz", "buy", SOL, 100, "Wzero")
    r.add("A", TA + 120, "Mz", "sell", 0, 100, "Wzero")
    return r.write(root)


_FIX = {}


def fixes():
    if not _FIX:
        dirs = build_fixes(tempfile.mkdtemp(prefix="ws_fix_"))
        ws.run([dirs["A"], dirs["B"], dirs["C"]], workers=1)
        p = dirs["C"]
        _FIX["df"] = pd.read_csv(os.path.join(p, "wallet_scores.csv"), keep_default_na=False, na_values=[""])
        _FIX["meta"] = json.load(open(os.path.join(p, "wallet_scores_meta.json")))
        _FIX["dirs"] = dirs
    return _FIX


def frow(w):
    df = fixes()["df"]
    r = df[df.wallet == w]
    return None if r.empty else r.iloc[0]


def test_dedupe_different_order():
    r = frow("Wdup")
    assert r is not None and r.n_closed == 1 and abs(r.mean_logret - math.log(2)) < 1e-9
    d = fixes()["dirs"]
    tr, _, wallets, _ = ws.load_all([d["A"], d["B"], d["C"]])
    w = int((wallets == "Wdup").nonzero()[0][0])
    assert int((tr.w == w).sum()) == 2


def test_dedupe_keeps_in_dir_repeats():
    r = frow("Wrep")
    assert r is not None and r.n_closed == 1 and abs(r.mean_logret - math.log(2)) < 1e-9
    d = fixes()["dirs"]
    tr, _, wallets, _ = ws.load_all([d["A"], d["B"], d["C"]])
    w = int((wallets == "Wrep").nonzero()[0][0])
    assert int(((tr.w == w) & tr.buy).sum()) == 2


def test_oversell_capped():
    assert abs(frow("Wover").mean_logret - 0.0) < 1e-8
    assert abs(frow("Wover2").mean_logret - math.log(1.5)) < 1e-8
    assert abs(frow("Wover3").mean_logret - 0.0) < 1e-8
    meta = fixes()["meta"]
    assert meta["n_trips_sell_capped"] == 3
    assert meta["n_trips_sell_capped_gt5pct"] == 2   # Wover 10x, Wover2 5.5x; Wover3 (1.02x) is not material


def test_zero_proceeds_clipped_and_counted():
    assert abs(frow("Wzero").mean_logret + ws.LOGRET_CLIP) < 1e-8
    meta = fixes()["meta"]
    assert meta["n_trips_logret_clipped"] == 1 and meta["n_trips_used"] == 6
    assert abs(meta["mean_logret_all"] - (2 * math.log(2) + math.log(1.5) - ws.LOGRET_CLIP) / 6) < 1e-12


if __name__ == "__main__":
    fails = 0
    for name, fn in list(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn(); print(f"PASS {name}")
            except Exception as e:  # noqa: BLE001
                fails += 1; print(f"FAIL {name}: {type(e).__name__}: {e}")
    print(f"{'ALL PASS' if not fails else f'{fails} FAILED'}")
    sys.exit(1 if fails else 0)
