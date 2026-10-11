"""Offline wallet scores per recording window (V5 plan section 4, "Wallet score"). No network, no LLM.

Usage: python3 python/research/wallet_scores.py data/v5_A data/v5_B ...
Every given dir W is scored using the trades of ALL given dirs, but only information available before W's
window start S_W = min(block_time) of W's trades (strict causality: an event at time t is used only if t < S_W).

Writes W/wallet_scores.csv and W/wallet_scores_meta.json.

Definitions (all on trades deduplicated across dirs, see load_all):
- Cross-dir dedupe: the same on-chain trade can sit in two dirs with DIFFERENT `order` values (follow-up events of
  one window use a synthetic intra-slot tx index, window events the real one), so copies are matched on
  (slot, mint, trader, side, sol, tokens, occurrence rank of that key inside its dir) and the copy from the
  earliest-starting dir is kept. Repeats of the same key inside one dir are distinct trades and are kept.
- Round trip per (wallet, mint), in chain order: opens on a buy while flat, closes on the sell that brings the token
  balance to <= 1% of the max balance held in that trip (dust is then ignored; sells while flat are ignored, e.g.
  tokens bought before the recording). A sell of more tokens than the trip still holds (tokens held before the
  recording or received by transfer) is credited only pro rata: SOL * balance / tokens_sold, and the balance goes
  to 0 (the trip closes; counted in meta n_trips_sell_capped, and in n_trips_sell_capped_gt5pct when the trip
  sold > 5% more tokens than it bought). logret = ln(SOL from sells / SOL paid for buys); a trip
  with 0 SOL proceeds (or a ratio beyond 1e4x either way) is clipped to +-LOGRET_CLIP = ln(1e4) instead of
  +-inf (counted in meta n_trips_logret_clipped); trips with 0 SOL paid are dropped. hold_s = close_time - open_time. A trip counts for W only if close_time < S_W. Trips are a deterministic online
  state machine (nothing after the close changes them), so they are computed once globally and filtered per W.
- score = (n * mean_logret + K * mean_logret_all) / (n + K), mean_logret_all over all qualifying trips (all wallets).
- fast_share = share of qualifying trips with hold_s < FAST_S; is_flipper = median_hold_s < FLIP_S.
- is_bot = (fast_share > 0.8 AND n_closed >= BOT_MIN_TRIPS = 5, V5-12) OR mean over the wallet's active UTC days (trades before S_W) of distinct mints traded
  that day > 50.
- sniper_coins = distinct newborn mints whose first observed trade is before S_W where the wallet bought (before S_W)
  within the first 3 slots of the mint's first observed slot. Newborn: first observed trade >= 60 s after the start
  of the dir it was observed in (the earliest-starting dir holding that trade), AND that trade is on the pump.fun
  bonding curve with a pre-trade virtual SOL reserve within 1% of the 30 SOL launch value (a coin seen 60 s into a
  dir can still be mid-life, or an already-migrated PumpSwap pool; the reserve check uses only that trade's data).
- clusters: A[wallet, mint] = 1 if a non-bot wallet bought (before S_W) within 300 s of the mint's first observed
  trade; mints with > 2000 such buyers are ignored; pairs sharing >= 3 mints are linked; connected components.
"""
import json
import math
import multiprocessing as mp
import os
import sys
import time

import numpy as np
import pandas as pd
import pyarrow as pa
import pyarrow.csv as pacsv
import scipy.sparse as sp
from scipy.sparse.csgraph import connected_components

K = 10
CLOSE_FRAC = 0.01          # trip closes when balance <= 1% of trip max
FAST_S = 10                # fast trip: hold < 10 s
FLIP_S = 60                # flipper: median hold < 60 s
BOT_FAST_SHARE = 0.8
BOT_MIN_TRIPS = 5           # V5-12: the fast-share bot rule needs at least this many closed trips
BOT_MINTS_PER_DAY = 50
SNIPER_SLOTS = 3           # bought at slot < first_slot + 3
NEWBORN_S = 60             # first observed trade >= dir start + 60 s
EARLY_S = 300              # co-buying: bought within 300 s of first observed trade
MIN_SHARED = 3
MAX_EARLY_BUYERS = 2000
LOGRET_CLIP = math.log(1e4)
OVERSOLD_MATERIAL = 1.05   # meta: trips that sold > 5% more tokens than they bought
LAUNCH_QUOTE = 30e9        # pump.fun curve virtual SOL reserve at launch (lamports)
LAUNCH_TOL = 0.01          # newborn: first trade's pre-trade reserve within 1% of LAUNCH_QUOTE
PARALLEL_MIN_ROWS = 200_000  # below this the trip state machine runs in-process
COLS = ["slot", "block_time", "order", "mint", "venue", "side", "sol_lamports", "token_raw", "trader",
        "quote_reserve_after"]
OUT_COLS = ["wallet", "n_closed", "score", "mean_logret", "median_hold_s", "fast_share", "is_bot", "is_flipper",
            "sniper_coins", "cluster_id", "cluster_size"]

G = {}  # shared (fork-inherited) state for worker processes


def log(msg):
    print(f"[wallet_scores {time.strftime('%H:%M:%S')}] {msg}", file=sys.stderr, flush=True)


def trades_path(d):
    for name in ("trades.csv", "trades.csv.zst"):
        p = os.path.join(d, name)
        if os.path.exists(p):
            return p
    raise FileNotFoundError(f"{d}: no trades.csv or trades.csv.zst")


def load_dir(d):
    p = trades_path(d)
    types = {"slot": pa.int64(), "block_time": pa.int64(), "order": pa.uint64(), "mint": pa.string(),
             "venue": pa.string(), "side": pa.string(), "sol_lamports": pa.int64(), "token_raw": pa.int64(),
             "trader": pa.string(), "quote_reserve_after": pa.float64()}  # reserve is u128 in Rust
    stream = pa.input_stream(p, compression="zstd" if p.endswith(".zst") else None)
    t = pacsv.read_csv(stream, convert_options=pacsv.ConvertOptions(include_columns=COLS, column_types=types))
    df = t.to_pandas()
    df["order"] = df["order"].astype(np.int64)  # (slot << 24) | ... fits in int64
    buy = df.side.values == "buy"
    sol = df.sol_lamports.values.astype(np.float64)
    pre = df.quote_reserve_after.values - np.where(buy, sol, -sol)   # pre-trade quote reserve
    df["launch"] = (df.venue.values == "pumpfun_curve") & (np.abs(pre - LAUNCH_QUOTE) <= LAUNCH_TOL * LAUNCH_QUOTE)
    return df.drop(columns=["venue", "quote_reserve_after"])


def _codes(values, sort):
    code, uniq = pd.factorize(values, sort=False)
    uniq = np.asarray(uniq, dtype=object)
    if sort:  # remap so that code order == string order (min code == min wallet string)
        o = np.argsort(uniq, kind="stable")
        rank = np.empty(len(o), dtype=np.int64)
        rank[o] = np.arange(len(o))
        code, uniq = rank[code], uniq[o]
    return code.astype(np.int32), uniq


def load_all(dirs):
    frames, starts = [], []
    for i, d in enumerate(dirs):
        df = load_dir(d)
        bt = df.block_time.values
        bt = bt[bt > 0]
        if len(bt) == 0:
            raise ValueError(f"{d}: no trades with block_time > 0")
        starts.append(int(bt.min()))
        df["dir"] = np.int16(i)
        frames.append(df)
        log(f"loaded {d}: {len(df):,} rows, window start {starts[-1]}")
    df = pd.concat(frames, ignore_index=True)
    del frames
    starts = np.array(starts, dtype=np.int64)
    w, wallets = _codes(df.trader.values, sort=True)
    m, mints = _codes(df.mint.values, sort=False)
    tr = pd.DataFrame({
        "slot": df.slot.values, "t": df.block_time.values, "order": df.order.values, "m": m, "w": w,
        "buy": df.side.values == "buy", "sol": df.sol_lamports.values, "tok": df.token_raw.values,
        "dir": df["dir"].values, "launch": df.launch.values})
    del df
    tr = tr[tr.t > 0]
    # Dedupe across dirs; keep the copy from the earliest-starting dir (reference for the newborn rule).
    # `order` cannot be the key: follow-up events (follow.rs merge) carry a synthetic intra-slot tx index while
    # window events carry the real one, so one on-chain trade can have two order values. Key = (slot, mint,
    # trader, side, sol, tokens) plus the occurrence rank of that key inside its dir, so k identical trades in one
    # dir stay k trades and the result holds max over dirs of the per-dir count.
    n0 = len(tr)
    ds = starts[tr["dir"].values]
    if len(starts) > 1 and n0 > 1:
        key = ["slot", "m", "w", "buy", "sol", "tok"]
        tr = tr.take(np.lexsort((tr.order.values, ds))).reset_index(drop=True)   # earliest dir start first
        cand = tr.duplicated(subset=key, keep=False).values                   # key seen more than once
        if cand.any():
            sub = tr[cand]
            rank = sub.groupby(key + ["dir"], sort=False).cumcount().values
            dup = sub.assign(_r=rank).duplicated(subset=key + ["_r"], keep="first").values
            tr = tr.drop(index=sub.index.values[dup])
        ds = starts[tr["dir"].values]
    tr = tr.take(np.lexsort((ds, tr.order.values))).reset_index(drop=True)
    log(f"all dirs: {n0:,} rows, {n0 - len(tr):,} duplicates dropped, {len(tr):,} kept, "
        f"{len(wallets):,} wallets, {len(mints):,} mints")
    return tr, starts, wallets, mints


# ----------------------------------------------------------------------------------------------- round trips
def _trips_chunk(bounds):
    lo, hi = bounds
    grp = G["tr_grp"][lo:hi].tolist()
    buy = G["tr_buy"][lo:hi].tolist()
    tok = G["tr_tok"][lo:hi].tolist()
    sol = G["tr_sol"][lo:hi].tolist()
    tt = G["tr_t"][lo:hi].tolist()
    og, ot0, ot1, obs, oss, orat = [], [], [], [], [], []
    cur, is_open, b, mx, t0, bs, ss, tb, ts = -1, False, 0, 0, 0, 0, 0, 0, 0
    for i in range(hi - lo):
        g = grp[i]
        if g != cur:
            cur, is_open = g, False
        if buy[i]:
            if not is_open:
                is_open, b, mx, t0, bs, ss, tb, ts = True, tok[i], tok[i], tt[i], sol[i], 0, tok[i], 0
            else:
                b += tok[i]
                bs += sol[i]
                tb += tok[i]
                if b > mx:
                    mx = b
        elif is_open:
            ts += tok[i]
            if tok[i] > b:     # sells tokens the trip never bought: credit only the trip's share (b > 0 here)
                ss += sol[i] * b / tok[i]
                b = 0
            else:
                b -= tok[i]
                ss += sol[i]
            if b * 100 <= mx:  # <= 1% of max (CLOSE_FRAC)
                og.append(g); ot0.append(t0); ot1.append(tt[i]); obs.append(bs); oss.append(ss); orat.append(ts / tb)
                is_open = False
    return og, ot0, ot1, obs, oss, orat  # orat = tokens sold / tokens bought (> 1 <=> a sell was capped)


def compute_trips(tr, workers):
    """All closed round trips: DataFrame w, m, open_t, close_t, hold_s, logret."""
    empty = pd.DataFrame({c: [] for c in ["w", "m", "open_t", "close_t", "hold_s", "logret", "oversold", "clipped"]})
    x = tr[tr.tok.values > 0]
    if len(x) == 0:
        return empty
    x = x.take(np.lexsort((x.order.values, x.m.values, x.w.values)))  # tr is in order; lexsort is stable
    key = x.w.values.astype(np.int64) << 32 | x.m.values.astype(np.int64)
    newg = np.r_[True, key[1:] != key[:-1]]
    gid = np.cumsum(newg) - 1
    nb = np.bincount(gid, weights=x.buy.values)
    nall = np.bincount(gid)
    both = (nb > 0) & (nb < nall)            # a trip needs at least one buy and one sell
    x = x[both[gid]]
    if len(x) == 0:
        return empty
    key = key[both[gid]]
    newg = np.r_[True, key[1:] != key[:-1]]
    grp = np.cumsum(newg) - 1
    gw, gm = x.w.values[newg], x.m.values[newg]
    G.update(tr_grp=grp, tr_buy=x.buy.values, tr_tok=x.tok.values, tr_sol=x.sol.values, tr_t=x.t.values)
    starts = np.flatnonzero(newg)
    n_chunks = max(1, min(len(starts), 4 * (os.cpu_count() or 1)))
    cut = starts[np.linspace(0, len(starts), n_chunks, endpoint=False).astype(int)]
    edges = list(zip(cut.tolist(), np.r_[cut[1:], len(x)].tolist()))
    if workers > 1 and len(x) > PARALLEL_MIN_ROWS:
        with mp.get_context("fork").Pool(workers) as pool:   # fork after G is set: workers inherit the arrays
            res = pool.map(_trips_chunk, edges)
    else:
        res = [_trips_chunk(e) for e in edges]
    g = np.concatenate([np.asarray(r[0], dtype=np.int64) for r in res])
    cols = [np.concatenate([np.asarray(r[j], dtype=np.float64) for r in res]) for j in range(1, 5)]
    t0, t1, bs, ss = cols
    rat = np.concatenate([np.asarray(r[5], dtype=np.float64) for r in res])
    ok = bs > 0
    with np.errstate(divide="ignore"):
        lr = np.log(np.maximum(ss[ok], 0) / bs[ok])
    clipped = np.abs(lr) > LOGRET_CLIP          # includes -inf (0 SOL proceeds)
    lr = np.nan_to_num(lr, neginf=-LOGRET_CLIP, posinf=LOGRET_CLIP)
    lr = np.clip(lr, -LOGRET_CLIP, LOGRET_CLIP)
    g = g[ok]
    trips = pd.DataFrame({"w": gw[g], "m": gm[g], "open_t": t0[ok].astype(np.int64), "close_t": t1[ok].astype(np.int64),
                          "logret": lr, "oversold": rat[ok], "clipped": clipped})
    trips["hold_s"] = trips.close_t - trips.open_t
    log(f"round trips: {len(trips):,} closed ({int((~ok).sum())} with 0 SOL paid dropped, "
        f"{int((trips.oversold > 1).sum()):,} with sells capped to the trip balance "
        f"({int((trips.oversold > OVERSOLD_MATERIAL).sum()):,} by > 5%), {int(clipped.sum()):,} logret clipped) "
        f"from {len(starts):,} (wallet, mint) groups with buys and sells")
    return trips


# ----------------------------------------------------------------------------------------------- precompute
def precompute(tr, starts):
    first = tr.groupby("m", sort=False).agg(ft=("t", "first"), fs=("slot", "first"), fd=("dir", "first"),
                                            fl=("launch", "first"))
    # tr is sorted by order, so "first" = first observed trade
    newborn = (first.ft.values >= starts[first.fd.values] + NEWBORN_S) & first.fl.values
    mft = np.full(int(tr.m.max()) + 1, np.iinfo(np.int64).max, dtype=np.int64)
    mfs = np.zeros_like(mft)
    mnb = np.zeros(len(mft), dtype=bool)
    mft[first.index.values] = first.ft.values
    mfs[first.index.values] = first.fs.values
    mnb[first.index.values] = newborn
    buys = tr[tr.buy]
    # sniper buys: min buy time per (wallet, newborn mint) within the first 3 slots
    sb = buys[(buys.slot.values - mfs[buys.m.values] < SNIPER_SLOTS) & mnb[buys.m.values]]
    snip = sb.groupby(["w", "m"], sort=False).t.min().reset_index()
    # early buys for co-buying: min buy time per (wallet, mint) within 300 s of the first observed trade
    eb = buys[buys.t.values - mft[buys.m.values] < EARLY_S]
    early = eb.groupby(["w", "m"], sort=False).t.min().reset_index()
    # distinct mints per wallet per UTC day, with the first time the (wallet, day, mint) was seen
    d = tr[["w", "m", "t"]].copy()
    d["day"] = d.t // 86400
    wdm = d.groupby(["w", "day", "m"], sort=False).t.min().reset_index()
    log(f"precompute: {len(first):,} mints ({int(newborn.sum()):,} newborn), {len(snip):,} sniper buys, "
        f"{len(early):,} early buys, {len(wdm):,} wallet-day-mint rows")
    return snip, early, wdm


def clusters(early_w, n_wallets):
    """early_w: DataFrame w, m (non-bot early buys before S_W). Returns (comp label per wallet or -1, ignored mints)."""
    lab = np.full(n_wallets, -1, dtype=np.int64)
    if len(early_w) == 0:
        return lab, 0
    cnt = early_w.groupby("m").w.transform("size").values
    ignored = int(early_w.m[cnt > MAX_EARLY_BUYERS].nunique())
    e = early_w[(cnt <= MAX_EARLY_BUYERS) & (cnt >= 2)]
    wc = e.groupby("w").m.transform("size").values
    e = e[wc >= MIN_SHARED]           # a wallet needs >= 3 early mints to share 3 with anyone
    if len(e) == 0:
        return lab, ignored
    wu, wi = np.unique(e.w.values, return_inverse=True)
    mu, mi = np.unique(e.m.values, return_inverse=True)
    A = sp.csr_matrix((np.ones(len(e), dtype=np.int32), (wi, mi)), shape=(len(wu), len(mu)))
    C = sp.triu(A @ A.T, k=1).tocoo()
    keep = C.data >= MIN_SHARED
    r, c = C.row[keep], C.col[keep]
    if len(r) == 0:
        return lab, ignored
    nodes = np.unique(np.r_[r, c])
    ri, ci = np.searchsorted(nodes, r), np.searchsorted(nodes, c)
    graph = sp.coo_matrix((np.ones(len(ri), dtype=np.int8), (ri, ci)), shape=(len(nodes), len(nodes)))
    _, comp = connected_components(graph, directed=False)
    lab[wu[nodes]] = comp
    return lab, ignored


# ----------------------------------------------------------------------------------------------- per window
def score_window(i):
    S = int(G["starts"][i])
    trips, snip, early, wdm = G["trips"], G["snip"], G["early"], G["wdm"]
    wallets, n_w = G["wallets"], len(G["wallets"])

    q = trips[trips.close_t.values < S]
    mean_all = float(q.logret.mean()) if len(q) else float("nan")
    st = q.groupby("w").agg(n_closed=("logret", "size"), mean_logret=("logret", "mean"),
                            median_hold_s=("hold_s", "median"))
    st["fast_share"] = pd.Series((q.hold_s.values < FAST_S).astype(np.float64)).groupby(q.w.values).mean()

    dm = wdm[wdm.t.values < S]
    per_day = dm.groupby(["w", "day"], sort=False).size()
    mpd = per_day.groupby(level=0).mean()
    bot = np.zeros(n_w, dtype=bool)
    mpd_rule = mpd.index.values[mpd.values > BOT_MINTS_PER_DAY]
    fast_hi = st.fast_share.values > BOT_FAST_SHARE
    enough = st.n_closed.values >= BOT_MIN_TRIPS
    fast_rule = st.index.values[fast_hi & enough]
    bot[mpd_rule] = True
    bot[fast_rule] = True

    sc = snip[snip.t.values < S].groupby("w").m.nunique()

    ew = early[(early.t.values < S)]
    ew = ew[~bot[ew.w.values]]
    lab, ignored = clusters(ew[["w", "m"]], n_w)

    idx = np.unique(np.r_[st.index.values, sc.index.values, np.flatnonzero(lab >= 0)]).astype(np.int64)
    out = pd.DataFrame({"w": idx})
    out = out.join(st, on="w")
    out["n_closed"] = out.n_closed.fillna(0).astype(np.int64)
    nn = out.n_closed.values
    out["score"] = (nn * out.mean_logret.fillna(0).values + K * mean_all) / (nn + K)
    out["is_bot"] = bot[idx].astype(np.int8)
    out["is_flipper"] = (out.median_hold_s.values < FLIP_S).astype(np.int8)  # NaN -> 0
    out["sniper_coins"] = sc.reindex(idx).fillna(0).astype(np.int64).values
    l = lab[idx]
    labs = pd.Series(idx[l >= 0]).groupby(l[l >= 0])
    cmin, csize = labs.min(), labs.size()
    out["cluster_size"] = np.where(l >= 0, csize.reindex(l).values, 1).astype(np.int64)
    out["cluster_id"] = np.where(l >= 0, wallets[cmin.reindex(l).fillna(0).astype(np.int64).values], "")
    out["wallet"] = wallets[idx]
    out = out[OUT_COLS].sort_values("wallet", kind="stable")

    d = G["dirs"][i]
    out.to_csv(os.path.join(d, "wallet_scores.csv"), index=False, float_format="%.10g")
    meta = {"window_start": S, "n_trips_used": int(len(q)),
            "n_trips_sell_capped": int((q.oversold.values > 1).sum()),
            "n_trips_sell_capped_gt5pct": int((q.oversold.values > OVERSOLD_MATERIAL).sum()),
            "n_trips_logret_clipped": int(q.clipped.sum()),
            "mean_logret_all": None if math.isnan(mean_all) else mean_all,
            "n_wallets": int(len(out)), "n_bots": int(out.is_bot.sum()),
            "n_bots_fast_rule": int(len(fast_rule)), "n_bots_mints_per_day_rule": int(len(mpd_rule)),
            "n_fast_share_below_min_trips": int((fast_hi & ~enough).sum()),
            "n_clustered": int((out.cluster_size > 1).sum()), "n_clusters": int(len(csize)),
            "ignored_mints": ignored, "source_dirs": [os.path.abspath(x) for x in G["dirs"]],
            "params": {"k": K, "close_frac": CLOSE_FRAC, "fast_s": FAST_S, "flip_s": FLIP_S,
                       "bot_fast_share": BOT_FAST_SHARE, "bot_min_trips": BOT_MIN_TRIPS, "bot_mints_per_day": BOT_MINTS_PER_DAY,
                       "sniper_slots": SNIPER_SLOTS, "newborn_s": NEWBORN_S, "early_s": EARLY_S,
                       "min_shared": MIN_SHARED, "max_early_buyers": MAX_EARLY_BUYERS,
                       "logret_clip": LOGRET_CLIP, "launch_quote": LAUNCH_QUOTE, "launch_tol": LAUNCH_TOL}}
    with open(os.path.join(d, "wallet_scores_meta.json"), "w") as f:
        json.dump(meta, f, indent=2)
    return d, meta


def run(dirs, workers=None):
    dirs = [os.path.normpath(d) for d in dirs]
    t0 = time.time()
    tr, starts, wallets, mints = load_all(dirs)
    workers = workers or min(os.cpu_count() or 1, 61)
    ctx = mp.get_context("fork")
    trips = compute_trips(tr, workers)
    snip, early, wdm = precompute(tr, starts)
    del tr
    G.clear()
    G.update(trips=trips, snip=snip, early=early, wdm=wdm, wallets=wallets, starts=starts, dirs=dirs)
    nw = min(workers, len(dirs))
    if nw > 1:
        with ctx.Pool(nw) as p:
            res = p.map(score_window, range(len(dirs)))
    else:
        res = [score_window(i) for i in range(len(dirs))]
    for d, m in res:
        log(f"{d}: start {m['window_start']} trips {m['n_trips_used']:,} wallets {m['n_wallets']:,} "
            f"bots {m['n_bots']:,} clustered {m['n_clustered']:,} ignored_mints {m['ignored_mints']}")
    log(f"done in {time.time() - t0:.1f} s")
    return res


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    run(sys.argv[1:])
