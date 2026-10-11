//! The only path from decoded events (`backtest::Ev`) to radar input (V4-10, radar part): trade construction with
//! real liquidity / marginal price / cluster / creator flag, births without selection leaks, and the per-second sweep.

use crate::backtest::{Ev, Events};
use decoder::pump::CURVE_INITIAL_REAL_TOKEN;
use radar::{Birth, BirthSource, NewbornMode, Params, Radar, Signal, Trade};
use std::path::Path;
use std::sync::Arc;

#[derive(Default, Clone, Copy)]
struct FeedCoin {
    on_amm: bool,
    liq_sol: f64,
    mprice: f64,
}

/// Builds `radar::Trade` from events, tracking each coin's active pool (curve until its first AMM event).
pub struct TradeBuilder {
    coins: Vec<FeedCoin>,
    clusters: Option<Arc<[u32]>>,
}

impl TradeBuilder {
    pub fn new(n_coins: usize, clusters: Option<Arc<[u32]>>) -> Self {
        Self { coins: vec![FeedCoin { on_amm: false, liq_sol: f64::NAN, mprice: f64::NAN }; n_coins], clusters }
    }

    /// Cluster id of a wallet (identity without a lookup or for wallets outside it).
    pub fn cluster_of(&self, user: u32) -> u32 {
        self.clusters.as_ref().and_then(|c| c.get(user as usize).copied()).unwrap_or(user)
    }

    pub fn build(&mut self, e: &Ev) -> Trade {
        let cluster = self.cluster_of(e.user);
        let by_creator = e.user == e.creator || (self.clusters.is_some() && cluster == self.cluster_of(e.creator));
        let fc = &mut self.coins[e.coin as usize];
        if e.amm || !fc.on_amm {
            fc.liq_sol = if e.amm { e.quote_res as f64 } else { e.real_quote as f64 } / 1e9;
            fc.mprice = if e.token_res > 0 { e.quote_res as f64 / e.token_res as f64 } else { f64::NAN };
        }
        fc.on_amm |= e.amm;
        let solf = e.sol as f64 / 1e9;
        Trade {
            time: e.time,
            order: e.order,
            coin: e.coin,
            trader: e.user,
            cluster,
            buy: e.buy,
            sol: solf,
            price: if e.tokens > 0 { solf / e.tokens as f64 } else { 0.0 },
            mprice: fc.mprice,
            liq_sol: fc.liq_sol,
            amm: e.amm,
            by_creator,
        }
    }
}

/// Coin births. Authoritative: on-chain creations from an unselected window-wide scan (in-window only). Proxy
/// (no scan): first trade after a warm-up, on the curve, with curve progress < proxy_max_progress. Follow-up
/// creation times (known only for followed = HOT coins) are never used for births (selection leak).
pub struct BirthTable {
    pub source: BirthSource,
    window_start: i64,
    created: Vec<Option<i64>>,
    warmup_s: i64,
    max_progress: f64,
}

impl BirthTable {
    pub fn new(ev: &Events, p: &Params) -> Self {
        let source = match p.newborn_mode {
            NewbornMode::Auto if ev.creations_scanned => BirthSource::Authoritative,
            NewbornMode::Auto => BirthSource::Proxy,
            NewbornMode::Authoritative => {
                if !ev.creations_scanned {
                    tracing::warn!("newborn_mode=authoritative without a creation scan: only follow-up creations exist (selected by HOT status)");
                }
                BirthSource::Authoritative
            }
            NewbornMode::Proxy => BirthSource::Proxy,
            NewbornMode::LegacyFirstSeen => BirthSource::LegacyFirstSeen,
        };
        Self {
            source,
            window_start: ev.window.0,
            created: ev.created.iter().map(|c| c.map(|x| x.0)).collect(),
            warmup_s: p.proxy_warmup_s,
            max_progress: p.proxy_max_progress,
        }
    }

    /// Pure: depends only on the coin's first event, the window start and the creation table.
    pub fn birth(&self, coin: u32, first: &Ev) -> Birth {
        match self.source {
            BirthSource::Authoritative => match self.created.get(coin as usize).copied().flatten() {
                Some(ct) if ct >= self.window_start => Birth::New { t: ct, proxy: false },
                _ => Birth::Old,
            },
            BirthSource::Proxy => {
                let progress = 1.0 - first.real_token as f64 / CURVE_INITIAL_REAL_TOKEN;
                if first.time > self.window_start + self.warmup_s && !first.amm && progress < self.max_progress {
                    Birth::New { t: first.time, proxy: true }
                } else {
                    Birth::Old
                }
            }
            BirthSource::LegacyFirstSeen => {
                if first.time > self.window_start + self.warmup_s {
                    Birth::New { t: first.time, proxy: true }
                } else {
                    Birth::Old
                }
            }
        }
    }
}

/// Radar + trade builder + births + sweep, in event order.
pub struct RadarFeed {
    radar: Radar,
    tb: TradeBuilder,
    births: BirthTable,
    seen: Vec<bool>,
    last_sweep: i64,
}

impl RadarFeed {
    pub fn new(ev: &Events, p: Params) -> Self {
        Self { radar: Radar::new(p), tb: TradeBuilder::new(ev.mints.len(), None), births: BirthTable::new(ev, &p), seen: vec![false; ev.mints.len()], last_sweep: i64::MIN }
    }

    /// Wallet -> cluster lookup (call before the first event).
    pub fn set_clusters(&mut self, c: Option<Arc<[u32]>>) {
        self.tb.clusters = c;
    }

    pub fn radar(&self) -> &Radar {
        &self.radar
    }

    pub fn births(&self) -> &BirthTable {
        &self.births
    }

    pub fn birth_source(&self) -> BirthSource {
        self.births.source
    }

    pub fn cluster_of(&self, user: u32) -> u32 {
        self.tb.cluster_of(user)
    }

    /// Sweep once per new second (Demote / Unwatch appended to `swept`), register the birth on the coin's first
    /// event, then feed the trade.
    pub fn step(&mut self, e: &Ev, swept: &mut Vec<(u32, Signal)>) -> Option<Signal> {
        if e.time > self.last_sweep {
            self.last_sweep = e.time;
            self.radar.sweep(e.time, swept);
        }
        let ci = e.coin as usize;
        if !self.seen[ci] {
            self.seen[ci] = true;
            let b = self.births.birth(e.coin, e);
            self.radar.set_birth(e.coin, b);
        }
        let t = self.tb.build(e);
        self.radar.on_trade(&t)
    }
}

/// `{"params": {...}}` file (radar-tune output); missing fields take `Params::default()` values.
pub fn load_params(path: &Path) -> anyhow::Result<Params> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?)?;
    Ok(serde_json::from_value(v["params"].clone())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(time: i64, coin: u32, amm: bool, real_token: u64) -> Ev {
        Ev { slot: 1, order: 1, time, coin, amm, buy: true, sol: 100_000_000, tokens: 1000, user: 1, creator: 9, quote_res: 40_000_000_000,
             token_res: 1_000_000_000_000_000, real_quote: 2_000_000_000, real_token, fee_bps: 100, complete: false }
    }

    fn events(scanned: bool, created: Vec<Option<(i64, Option<f64>)>>) -> Events {
        let n = created.len();
        let mut e = Events::new(vec![], (0..n).map(|i| format!("m{i}")).collect(), (1000, 3000), vec![]);
        e.created = created;
        e.creations_scanned = scanned;
        e
    }

    const FRESH: u64 = 790_000_000_000_000; // progress 0.4%

    #[test]
    fn births_authoritative_from_window_scan() {
        let e = events(true, vec![Some((1500, None)), Some((500, None)), None]);
        let b = BirthTable::new(&e, &Params::default());
        assert_eq!(b.source, BirthSource::Authoritative);
        assert_eq!(b.birth(0, &ev(1600, 0, false, FRESH)), Birth::New { t: 1500, proxy: false });
        assert_eq!(b.birth(1, &ev(1600, 1, false, FRESH)), Birth::Old, "created before the window");
        assert_eq!(b.birth(2, &ev(1600, 2, false, FRESH)), Birth::Old);
    }

    #[test]
    fn births_proxy_rule() {
        let e = events(false, vec![Some((1500, None)), None]);
        let b = BirthTable::new(&e, &Params::default());
        assert_eq!(b.source, BirthSource::Proxy);
        assert_eq!(b.birth(1, &ev(1100, 1, false, FRESH)), Birth::New { t: 1100, proxy: true });
        assert_eq!(b.birth(1, &ev(1100, 1, true, FRESH)), Birth::Old, "AMM first");
        assert_eq!(b.birth(1, &ev(1100, 1, false, 700_000_000_000_000)), Birth::Old, "progress 11.7%");
        assert_eq!(b.birth(1, &ev(1050, 1, false, FRESH)), Birth::Old, "within warm-up");
        assert_eq!(b.birth(0, &ev(1000, 0, false, FRESH)), Birth::Old, "follow-up creation is not used (no leak)");
    }

    #[test]
    fn births_legacy_first_seen() {
        let e = events(true, vec![None]);
        let p = Params { newborn_mode: NewbornMode::LegacyFirstSeen, ..Default::default() };
        let b = BirthTable::new(&e, &p);
        assert_eq!(b.birth(0, &ev(1061, 0, true, 0)), Birth::New { t: 1061, proxy: true });
        assert_eq!(b.birth(0, &ev(1060, 0, true, 0)), Birth::Old);
    }

    #[test]
    fn trade_builder_active_pool_and_liquidity() {
        let mut tb = TradeBuilder::new(1, None);
        let t = tb.build(&ev(1, 0, false, FRESH));
        assert!((t.liq_sol - 2.0).abs() < 1e-12 && !t.amm, "curve: real quote reserve");
        let mut a = ev(2, 0, true, 0);
        a.quote_res = 90_000_000_000;
        let t = tb.build(&a);
        assert!((t.liq_sol - 90.0).abs() < 1e-12 && t.amm);
        let late = ev(3, 0, false, FRESH);
        let t = tb.build(&late);
        assert!((t.liq_sol - 90.0).abs() < 1e-12, "late curve event does not overwrite the AMM pool");
        assert!((t.mprice - 90_000_000_000.0 / 1e15).abs() < 1e-18);
    }

    #[test]
    fn cluster_lookup_and_creator_family() {
        let lookup: Arc<[u32]> = Arc::from(vec![0u32, 5, 2, 3, 4, 5, 6, 7, 8, 5]);
        let mut tb = TradeBuilder::new(1, Some(lookup));
        assert_eq!(tb.cluster_of(1), 5);
        assert_eq!(tb.cluster_of(100), 100, "outside the lookup");
        let t = tb.build(&ev(1, 0, false, FRESH)); // user 1, creator 9 -> both cluster 5
        assert!(t.by_creator && t.cluster == 5);
        let mut tb = TradeBuilder::new(1, None);
        assert!(!tb.build(&ev(1, 0, false, FRESH)).by_creator);
    }

    #[test]
    fn load_params_defaults_missing_fields() {
        let d = std::env::temp_dir().join(format!("rf_params_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("p.json");
        std::fs::write(&f, r#"{"params": {"min_buyers": 7, "win_s": 30}}"#).unwrap();
        let p = load_params(&f).unwrap();
        assert_eq!((p.min_buyers, p.win_s, p.watch), (7, 30, Params::default().watch));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
