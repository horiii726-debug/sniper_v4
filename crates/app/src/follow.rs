//! V5 Phase 1, tier 2 of the backfill: follow every coin the radar flagged HOT in a backfill window for
//! `hours` after its first HOT time (or until it is dead, V5 §3.2), through the same pump.fun / PumpSwap
//! decoder as the window. Also records the coin's real creation time and creation price.
//!
//! Source: `getSignaturesForAddress` on the coin's MINT. Every curve and PumpSwap trade references the
//! mint, so one stream covers the curve, the migration and the pool. Transactions are fetched oldest
//! first; a coin stops being fetched once it is dead (no real trade for 30 min and price < 0.1 x peak).
//!
//! Output `<rec>/follow.bin.zst` (bincode + zstd), merged into the window events by `backtest::load`.

use crate::backtest::{Ev, Events};
use anyhow::{anyhow, Result};
use decoder::pump::Venue;
use decoder::txview::TxView;
use feed::follow::{FollowCounters, Limiter, SigEntry};
use feed::rpc::Rpc;
use crate::radar_feed::RadarFeed;
use radar::{Params as RadarParams, RadarState, Signal};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

pub const FOLLOW_FILE: &str = "follow.bin.zst";
const FORMAT_VERSION: u32 = 1;
const REAL_MIN_LAMPORTS: u64 = 10_000_000;
/// V5 §3.2: dead = no real trade for this long AND price below DEAD_PRICE_FRAC x peak.
pub const DEAD_IDLE_S: i64 = 1800;
pub const DEAD_PRICE_FRAC: f64 = 0.1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FEv {
    pub slot: u64,
    /// Position of the transaction in the coin's oldest-first signature list (intra-slot order).
    pub pos: u32,
    /// Event index inside the transaction.
    pub k: u8,
    pub time: i64,
    pub amm: bool,
    pub buy: bool,
    pub sol: u64,
    pub tokens: u64,
    pub user: u32,
    pub creator: u32,
    pub quote_res: u128,
    pub token_res: u128,
    pub real_quote: u64,
    pub real_token: u64,
    pub fee_bps: u64,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowCoin {
    pub mint: String,
    pub hot_time: i64,
    /// Real creation (block time of the first signature of the mint; price from the creation event).
    pub created: Option<(i64, Option<f64>)>,
    pub cover_end: i64,
    pub dead_at: Option<i64>,
    /// complete | dead | range_pages_cap | tx_fetch_failed | scan_failed
    pub status: String,
    pub sigs_in_range: u32,
    pub txs_fetched: u32,
    pub evs: Vec<FEv>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FollowFile {
    pub version: u32,
    pub window: (i64, i64),
    pub window_last_slot: u64,
    pub hours: f64,
    pub users: Vec<String>,
    pub coins: Vec<FollowCoin>,
}

pub fn read(path: &Path) -> Result<FollowFile> {
    let raw = zstd::decode_all(std::fs::File::open(path)?)?;
    let f: FollowFile = bincode::deserialize(&raw)?;
    if f.version != FORMAT_VERSION {
        return Err(anyhow!("{}: follow format {} (expected {FORMAT_VERSION})", path.display(), f.version));
    }
    Ok(f)
}

fn write(path: &Path, f: &FollowFile) -> Result<u64> {
    let raw = bincode::serialize(f)?;
    let z = zstd::bulk::compress(&raw, 9)?;
    std::fs::write(path, &z)?;
    Ok(z.len() as u64)
}

/// Merge follow-up events into window events. Follow events are strictly after the window's last slot, so
/// they never interleave with window events; intra-slot order keys are assigned per slot over
/// (coin, signature position, event index).
pub fn merge(ev: &mut Events, f: &FollowFile) -> Result<()> {
    if f.window != ev.window {
        return Err(anyhow!("follow file window {:?} does not match recording window {:?}", f.window, ev.window));
    }
    let mut mint_id: FxHashMap<String, u32> = ev.mints.iter().enumerate().map(|(i, m)| (m.clone(), i as u32)).collect();
    let mut user_id: FxHashMap<String, u32> = ev.users.iter().enumerate().map(|(i, u)| (u.clone(), i as u32)).collect();
    let mut umap = Vec::with_capacity(f.users.len());
    for u in &f.users {
        let n = ev.users.len() as u32;
        let id = *user_id.entry(u.clone()).or_insert_with(|| {
            ev.users.push(u.clone());
            n
        });
        umap.push(id);
    }
    let mut add: Vec<(u32, &FEv)> = Vec::new();
    for c in &f.coins {
        let n = ev.mints.len() as u32;
        let coin = *mint_id.entry(c.mint.clone()).or_insert_with(|| {
            ev.mints.push(c.mint.clone());
            ev.cover_end.push(ev.window.1);
            ev.created.push(None);
            ev.dead_at.push(None);
            ev.followed.push(false);
            n
        });
        let ci = coin as usize;
        ev.cover_end[ci] = c.cover_end.max(ev.window.1);
        // keep the window-scan creation (unselected, authoritative); follow only fills unknown ones
        ev.created[ci] = ev.created[ci].or(c.created);
        ev.dead_at[ci] = c.dead_at;
        ev.followed[ci] = true;
        add.extend(c.evs.iter().filter(|e| e.slot > f.window_last_slot).map(|e| (coin, e)));
    }
    add.sort_by_key(|(coin, e)| (e.slot, *coin, e.pos, e.k));
    let (mut slot, mut tx, mut key) = (u64::MAX, 0u64, (u32::MAX, u32::MAX));
    for (coin, e) in add {
        if e.slot != slot {
            slot = e.slot;
            tx = 0;
            key = (coin, e.pos);
        } else if (coin, e.pos) != key {
            tx += 1;
            key = (coin, e.pos);
        }
        if tx >= 1 << 16 {
            return Err(anyhow!("slot {slot}: more than 65536 followed transactions"));
        }
        ev.evs.push(Ev {
            slot: e.slot,
            order: (e.slot << 24) | (tx << 8) | e.k as u64,
            time: e.time,
            coin,
            amm: e.amm,
            buy: e.buy,
            sol: e.sol,
            tokens: e.tokens,
            user: umap[e.user as usize],
            creator: umap[e.creator as usize],
            quote_res: e.quote_res,
            token_res: e.token_res,
            real_quote: e.real_quote,
            real_token: e.real_token,
            fee_bps: e.fee_bps,
            complete: e.complete,
            venue: e.amm as u8,
            pool: crate::backtest::NO_POOL,
        });
    }
    ev.evs.sort_by_key(|e| e.order);
    Ok(())
}

/// First radar Promote (or, with `min_state` Watch, first Watch / Promote) per coin inside the window, with the
/// same radar input as `ml_sim::Engine` (no cluster lookup: follow runs before wallet_scores.py).
pub fn hot_coins(ev: &Events, rp: RadarParams, min_state: RadarState) -> Vec<(u32, i64)> {
    let mut feed = RadarFeed::new(ev, rp);
    let mut first: FxHashMap<u32, i64> = FxHashMap::default();
    let mut swept = Vec::new();
    for e in &ev.evs {
        swept.clear();
        let s = feed.step(e, &mut swept);
        let hit = match min_state {
            RadarState::Watch => matches!(s, Some(Signal::Promote | Signal::Watch)),
            _ => s == Some(Signal::Promote),
        };
        if hit && e.time <= ev.window.1 {
            first.entry(e.coin).or_insert(e.time);
        }
    }
    let mut v: Vec<(u32, i64)> = first.into_iter().collect();
    v.sort_unstable();
    v
}

/// Price / activity state used for the dead-coin stop.
#[derive(Debug, Clone, Copy, Default)]
pub struct Life {
    pub price: f64,
    pub peak: f64,
    pub last_real: i64,
    pub on_amm: bool,
}

impl Life {
    pub fn on_event(&mut self, time: i64, amm: bool, sol: u64, quote_res: u128, token_res: u128) {
        self.on_amm |= amm;
        if sol >= REAL_MIN_LAMPORTS {
            self.last_real = self.last_real.max(time);
        }
        // Marginal price: curve until the coin trades on the AMM (late curve events are ignored).
        if (amm || !self.on_amm) && token_res > 0 {
            self.price = quote_res as f64 / token_res as f64;
            self.peak = self.peak.max(self.price);
        }
    }

    pub fn dead_at(&self, now: i64) -> Option<i64> {
        (now - self.last_real >= DEAD_IDLE_S && self.peak > 0.0 && self.price < DEAD_PRICE_FRAC * self.peak).then_some(self.last_real + DEAD_IDLE_S)
    }
}

pub struct Opts {
    pub hours: f64,
    pub rps: f64,
    pub coin_concurrency: usize,
    pub tx_batch: usize,
    pub max_range_pages: u32,
    pub max_history_pages: u32,
    /// Follow at most this many HOT coins (0 = all), in coin-id order; for cost measurement runs.
    pub limit: usize,
    /// Hot (default) or Watch (also follow WATCH coins: uncensored labels for pre-HOT rows, more RPC credits).
    pub min_state: RadarState,
}

/// Users seen in follow-up transactions, shared across coin tasks.
#[derive(Default)]
struct UserTable {
    ids: FxHashMap<String, u32>,
    names: Vec<String>,
}

impl UserTable {
    fn id(&mut self, u: &str) -> u32 {
        if let Some(i) = self.ids.get(u) {
            return *i;
        }
        let i = self.names.len() as u32;
        self.ids.insert(u.to_string(), i);
        self.names.push(u.to_string());
        i
    }
}

struct Ctx {
    rpc: Rpc,
    lim: Limiter,
    c: FollowCounters,
    users: std::sync::Mutex<UserTable>,
    window_last_slot: u64,
    o: Opts,
}

pub async fn run(rpc_url: String, rec: &Path, rp: RadarParams, o: Opts) -> Result<()> {
    use futures::StreamExt;
    let ev = crate::backtest::load_window(rec)?;
    let window_last_slot = ev.evs.iter().map(|e| e.slot).max().ok_or_else(|| anyhow!("empty window"))?;
    let birth_source = RadarFeed::new(&ev, rp).birth_source().as_str();
    let mut hot = hot_coins(&ev, rp, o.min_state);
    let n_hot = hot.len();
    if o.limit > 0 {
        hot.truncate(o.limit);
    }
    // Window state per followed coin, so the dead-coin rule sees the whole observed history.
    let mut life: FxHashMap<u32, Life> = hot.iter().map(|(c, t)| (*c, Life { last_real: *t, ..Default::default() })).collect();
    for e in &ev.evs {
        if let Some(l) = life.get_mut(&e.coin) {
            l.on_event(e.time, e.amm, e.sol, e.quote_res, e.token_res);
        }
    }
    tracing::info!(hot = n_hot, following = hot.len(), hours = o.hours, rps = o.rps, "follow start");
    let ctx = Arc::new(Ctx { rpc: Rpc::new(rpc_url), lim: Limiter::new(o.rps), c: FollowCounters::default(), users: Default::default(), window_last_slot, o });
    let t0 = latency::mono_ns();
    let total = hot.len();
    let mut coins = Vec::with_capacity(total);
    let mut stream = futures::stream::iter(hot.into_iter().map(|(coin, hot_t)| {
        let ctx = ctx.clone();
        let mint = ev.mints[coin as usize].clone();
        let l = life[&coin];
        async move { follow_coin(&ctx, mint, hot_t, l).await }
    }))
    .buffer_unordered(ctx.o.coin_concurrency);
    let mut last_log = t0;
    while let Some(fc) = stream.next().await {
        coins.push(fc);
        let now = latency::mono_ns();
        if now - last_log > 30_000_000_000 || coins.len() == total {
            last_log = now;
            tracing::info!(done = coins.len(), of = total, elapsed_s = (now - t0) / 1_000_000_000, calls = %ctx.c.to_json(), "follow progress");
        }
    }
    drop(stream);
    coins.sort_by(|a, b| a.mint.cmp(&b.mint));
    let users = std::mem::take(&mut ctx.users.lock().expect("users").names);
    let file = FollowFile { version: FORMAT_VERSION, window: ev.window, window_last_slot, hours: ctx.o.hours, users, coins };
    let disk = write(&rec.join(FOLLOW_FILE), &file)?;
    let mut status: std::collections::BTreeMap<&str, usize> = Default::default();
    for c in &file.coins {
        *status.entry(c.status.as_str()).or_default() += 1;
    }
    let summary = serde_json::json!({
        "window": file.window, "window_last_slot": window_last_slot, "hours": file.hours,
        "hot_coins_in_window": n_hot, "followed": file.coins.len(), "status": status,
        "birth_source": birth_source, "creations_scanned": ev.creations_scanned, "min_state": format!("{:?}", ctx.o.min_state),
        "created_known": file.coins.iter().filter(|c| c.created.is_some()).count(),
        "created_price_known": file.coins.iter().filter(|c| c.created.is_some_and(|x| x.1.is_some())).count(),
        "dead": file.coins.iter().filter(|c| c.dead_at.is_some()).count(),
        "sigs_in_range": file.coins.iter().map(|c| c.sigs_in_range as u64).sum::<u64>(),
        "txs_fetched": file.coins.iter().map(|c| c.txs_fetched as u64).sum::<u64>(),
        "events": file.coins.iter().map(|c| c.evs.len()).sum::<usize>(),
        "calls": ctx.c.to_json(), "disk_bytes": disk, "elapsed_s": (latency::mono_ns() - t0) as f64 / 1e9, "rps_limit": ctx.o.rps,
    });
    std::fs::write(rec.join("follow_summary.json"), serde_json::to_string_pretty(&summary)?)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

async fn follow_coin(ctx: &Ctx, mint: String, hot_t: i64, mut life: Life) -> FollowCoin {
    let until = hot_t + (ctx.o.hours * 3600.0) as i64;
    let mut fc = FollowCoin {
        mint: mint.clone(), hot_time: hot_t, created: None, cover_end: 0, dead_at: None,
        status: String::new(), sigs_in_range: 0, txs_fetched: 0, evs: Vec::new(),
    };
    let scan = match feed::follow::scan_address(&ctx.rpc, &ctx.lim, &ctx.c, &mint, ctx.window_last_slot, until, true, ctx.o.max_range_pages, ctx.o.max_history_pages).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(%mint, err = %e, "signature scan failed");
            fc.status = "scan_failed".into();
            return fc;
        }
    };
    let mut dec = match crate::pump_check::load_decoder() {
        Ok(d) => d,
        Err(e) => {
            fc.status = format!("decoder: {e}");
            return fc;
        }
    };
    // Creation: the mint's first signature, decoded for the creation event (time from the event, else block time).
    if scan.exhausted {
        if let Some(first) = &scan.oldest {
            let mut created = (first.time, None);
            if let Ok(Some(v)) = feed::follow::get_transaction(&ctx.rpc, &ctx.lim, &ctx.c, &first.sig).await {
                if let Some(cr) = TxView::parse(&v).and_then(|tx| dec.decode_creations(&tx).into_iter().find(|c| c.mint == mint && !c.migration)) {
                    created = (if cr.time > 0 { cr.time } else { first.time }, Some(cr.price));
                }
            }
            fc.created = Some(created);
        }
    }
    if !scan.range_complete {
        // Paging stopped before reaching the window: the follow-up range is not contiguous with it.
        fc.status = "range_pages_cap".into();
        return fc;
    }
    fc.sigs_in_range = scan.in_range.len() as u32;
    let batch = ctx.o.tx_batch.max(1);
    let mut pos = 0usize;
    while pos < scan.in_range.len() {
        let chunk: &[SigEntry] = &scan.in_range[pos..(pos + batch).min(scan.in_range.len())];
        if let Some(d) = life.dead_at(chunk[0].time) {
            fc.dead_at = Some(d);
            fc.cover_end = chunk[0].time - 1;
            fc.status = "dead".into();
            return fc;
        }
        let txs = futures::future::join_all(chunk.iter().map(|s| feed::follow::get_transaction(&ctx.rpc, &ctx.lim, &ctx.c, &s.sig))).await;
        for (j, (s, r)) in chunk.iter().zip(txs).enumerate() {
            let v = match r {
                Ok(Some(v)) => v,
                other => {
                    if let Err(e) = other {
                        tracing::warn!(%mint, sig = %s.sig, err = %e, "getTransaction failed");
                    }
                    fc.cover_end = s.time - 1;
                    fc.status = "tx_fetch_failed".into();
                    return fc;
                }
            };
            fc.txs_fetched += 1;
            let Some(tx) = TxView::parse(&v) else { continue };
            let evs = dec.decode(&tx);
            let mut users = ctx.users.lock().expect("users");
            for (k, e) in evs.into_iter().enumerate().filter(|(_, e)| e.mint == mint) {
                life.on_event(s.time, e.venue == Venue::Amm, e.sol, e.quote_res, e.token_res);
                fc.evs.push(FEv {
                    slot: s.slot,
                    pos: (pos + j) as u32,
                    k: k.min(255) as u8,
                    time: s.time,
                    amm: e.venue == Venue::Amm,
                    buy: e.buy,
                    sol: e.sol,
                    tokens: e.tokens,
                    user: users.id(&e.user),
                    creator: users.id(&e.creator),
                    quote_res: e.quote_res,
                    token_res: e.token_res,
                    real_quote: e.real_quote,
                    real_token: e.real_token,
                    fee_bps: e.fee_bps,
                    complete: e.complete,
                });
            }
        }
        pos += chunk.len();
    }
    fc.cover_end = until;
    fc.dead_at = life.dead_at(until);
    fc.status = if fc.dead_at.is_some() { "dead".into() } else { "complete".into() };
    fc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wev(slot: u64, k: u64, coin: u32, time: i64) -> Ev {
        Ev {
            slot, order: (slot << 24) | (k << 8), time, coin, amm: false, buy: true, sol: 20_000_000, tokens: 1_000,
            user: 0, creator: 1, quote_res: 30_000_000_000, token_res: 1_000_000_000, real_quote: 0, real_token: 1, fee_bps: 100, complete: false, venue: 0, pool: 0,
        }
    }

    fn fev(slot: u64, pos: u32, k: u8, user: u32, time: i64) -> FEv {
        FEv {
            slot, pos, k, time, amm: true, buy: false, sol: 5_000_000, tokens: 10, user, creator: 1, quote_res: 1, token_res: 1,
            real_quote: 0, real_token: 0, fee_bps: 30, complete: false,
        }
    }

    /// Merge: users mapped by address, events after the window only, unique increasing order keys, per-coin
    /// metadata set; events of two coins in one slot get distinct transaction keys.
    #[test]
    fn merge_maps_users_and_orders_uniquely() {
        let mut ev = Events::new(vec![wev(10, 0, 0, 100), wev(11, 0, 1, 101)], vec!["A".into(), "B".into()], (100, 101), vec!["u0".into(), "dev".into()]);
        let f = FollowFile {
            version: FORMAT_VERSION, window: (100, 101), window_last_slot: 11, hours: 10.0,
            users: vec!["u9".into(), "dev".into(), "u0".into()],
            coins: vec![
                FollowCoin { mint: "A".into(), hot_time: 100, created: Some((50, Some(2.0))), cover_end: 5000, dead_at: None, status: "complete".into(),
                    sigs_in_range: 3, txs_fetched: 3, evs: vec![fev(11, 0, 0, 2, 101), fev(20, 1, 0, 0, 105), fev(20, 1, 1, 2, 105), fev(21, 2, 0, 0, 106)] },
                FollowCoin { mint: "B".into(), hot_time: 101, created: None, cover_end: 900, dead_at: Some(800), status: "dead".into(),
                    sigs_in_range: 1, txs_fetched: 1, evs: vec![fev(20, 0, 0, 0, 105)] },
            ],
        };
        merge(&mut ev, &f).unwrap();
        assert_eq!(ev.evs.len(), 2 + 4, "slot-11 follow event is inside the window and dropped");
        assert_eq!(ev.users, ["u0", "dev", "u9"]);
        let orders: Vec<u64> = ev.evs.iter().map(|e| e.order).collect();
        assert!(orders.windows(2).all(|w| w[0] < w[1]), "{orders:?}");
        let slot20: Vec<(u32, u64)> = ev.evs.iter().filter(|e| e.slot == 20).map(|e| (e.coin, (e.order >> 8) & 0xFFFF)).collect();
        assert_eq!(slot20, [(0, 0), (0, 0), (1, 1)], "coin A's two events share a tx; coin B is a separate tx");
        let a = ev.evs.iter().find(|e| e.slot == 20 && e.coin == 0 && e.order & 0xFF == 1).unwrap();
        assert_eq!(ev.users[a.user as usize], "u0");
        assert_eq!((ev.cover_end[0], ev.cover_end[1]), (5000, 900));
        assert_eq!(ev.created[0], Some((50, Some(2.0))));
        assert_eq!(ev.dead_at[1], Some(800));
        assert!(ev.followed[0] && ev.followed[1]);
        assert_eq!(ev.data_end(), 5000);
    }

    /// D2-N2: a window-scan creation is never overwritten by follow-up data; follow fills unknown creations only.
    #[test]
    fn merge_keeps_window_scan_creation() {
        let mut ev = Events::new(vec![wev(10, 0, 0, 100), wev(11, 0, 1, 101)], vec!["A".into(), "B".into()], (100, 101), vec!["u0".into(), "dev".into()]);
        ev.created[0] = Some((100, Some(1.0)));
        let coin = |m: &str, created| FollowCoin { mint: m.into(), hot_time: 100, created, cover_end: 500, dead_at: None, status: "complete".into(),
            sigs_in_range: 0, txs_fetched: 0, evs: vec![] };
        let f = FollowFile { version: FORMAT_VERSION, window: (100, 101), window_last_slot: 11, hours: 1.0, users: vec![],
            coins: vec![coin("A", None), coin("B", Some((40, Some(3.0))))] };
        merge(&mut ev, &f).unwrap();
        assert_eq!(ev.created[0], Some((100, Some(1.0))));
        assert_eq!(ev.created[1], Some((40, Some(3.0))));
    }

    #[test]
    fn merge_rejects_other_window() {
        let mut ev = Events::new(vec![wev(10, 0, 0, 100)], vec!["A".into()], (100, 101), vec!["u0".into(), "dev".into()]);
        let f = FollowFile { version: FORMAT_VERSION, window: (100, 999), window_last_slot: 10, hours: 1.0, users: vec![], coins: vec![] };
        assert!(merge(&mut ev, &f).is_err());
    }

    /// Dead = >= 30 min since the last real trade AND price < 0.1 x peak; dust trades do not count as activity.
    #[test]
    fn dead_rule() {
        let mut l = Life::default();
        l.on_event(0, false, 50_000_000, 100, 1); // price 100, real
        l.on_event(10, false, 50_000_000, 5, 1); // price 5 (< 10% of peak), real at t = 10
        l.on_event(1000, false, 1_000, 5, 1); // dust: not activity
        assert_eq!(l.dead_at(10 + DEAD_IDLE_S - 1), None);
        assert_eq!(l.dead_at(10 + DEAD_IDLE_S), Some(10 + DEAD_IDLE_S));
        let mut alive = l;
        alive.on_event(20, false, 50_000_000, 50, 1); // recovered to 50% of peak
        assert_eq!(alive.dead_at(10_000), None, "idle but price not collapsed");
        let mut amm = Life::default();
        amm.on_event(0, true, 50_000_000, 100, 1);
        amm.on_event(1, false, 50_000_000, 1, 1); // late curve event after migration: ignored for price
        assert_eq!(amm.price, 100.0);
    }
}
