use super::*;

#[test]
fn ewma_constant_rate_converges_and_decays() {
    // V4 §33.4: constant rate r for >> tau -> R converges to r; no events -> exact exp decay.
    let mut e = Ewma::default();
    let tau = 10.0;
    for i in 1..=10_000 {
        e.add(i as f64 * 0.5, 1.0, tau); // 2 events / s
    }
    let r = e.at(5000.0, tau);
    assert!((r - 2.0).abs() / 2.0 < 0.03, "{r}");
    let later = e.at(5010.0, tau);
    assert!((later / r - (-1.0f64).exp()).abs() < 1e-12);
}

/// Params with every V6 addition off, so the V4 rules are tested alone.
fn v4ish(p: Params) -> Params {
    Params { watch: false, decide_every_s: 0, old_flow_rule: false, cusum: CusumParams { enabled: false, ..Default::default() }, ..p }
}

fn tr(time: i64, trader: u32, buy: bool, sol: f64) -> Trade {
    Trade::basic(time, time as u64, 1, trader, buy, sol, 1.0)
}

#[test]
fn promotes_on_broad_real_buying_and_ignores_dust() {
    let p = v4ish(Params { min_buyers: 5, min_buy_sol: 0.5, ..Default::default() });
    let mut r = Radar::new(p);
    for i in 0..50 {
        assert_eq!(r.on_trade(&tr(i, i as u32, true, 0.001)), None);
    }
    let mut sig = None;
    for i in 0..5 {
        sig = r.on_trade(&tr(60 + i, 100 + i as u32, true, 0.2));
    }
    assert_eq!(sig, Some(Signal::Promote));
    let mut out = Vec::new();
    r.sweep(60 + 4 + 301, &mut out);
    assert_eq!(out, vec![(1, Signal::Demote)]);
}

#[test]
fn price_lift_promotes_thin_coin_before_big_move() {
    let p = v4ish(Params { min_buyers: 100, lift: 1.15, lift_min_buyers: 2, ..Default::default() });
    let mut r = Radar::new(p);
    let mk = |time: i64, trader: u32, buy: bool, price: f64| Trade::basic(time, time as u64, 1, trader, buy, 0.1, price);
    assert_eq!(r.on_trade(&mk(0, 1, false, 1.0)), None);
    assert_eq!(r.on_trade(&mk(10, 2, true, 1.05)), None);
    assert_eq!(r.on_trade(&mk(20, 3, true, 1.16)), Some(Signal::Promote));
}

#[test]
fn one_whale_does_not_promote() {
    let p = v4ish(Params { min_buyers: 5, min_buy_sol: 0.5, ..Default::default() });
    let mut r = Radar::new(p);
    for i in 0..20 {
        assert_eq!(r.on_trade(&tr(i, 7, true, 5.0)), None);
    }
}

fn base() -> Params {
    Params { min_buyers: 5, min_buy_sol: 1.0, max_median_buy_sol: 1.0, cusum: CusumParams { enabled: false, ..Default::default() }, ..Default::default() }
}

fn newborn(r: &mut Radar, coin: u32, t: i64) {
    r.set_birth(coin, Birth::New { t, proxy: false });
}

#[test]
fn newborn_is_not_auto_hot() {
    let mut r = Radar::new(base());
    newborn(&mut r, 1, 0);
    assert_eq!(r.on_trade(&tr(0, 1, true, 0.3)), Some(Signal::Watch));
    for i in 2..=4 {
        assert_ne!(r.on_trade(&tr(i as i64, i, true, 0.3)), Some(Signal::Promote));
    }
    assert_eq!(r.on_trade(&tr(5, 5, true, 0.3)), Some(Signal::Promote), "5 buyers, 1.5 SOL");
    assert_eq!(r.state(1), RadarState::Hot);
}

#[test]
fn newborn_strict_filters_block() {
    let mut r = Radar::new(base());
    newborn(&mut r, 1, 0);
    for i in 0..10 {
        assert_ne!(r.on_trade(&tr(i, i as u32, true, 2.0)), Some(Signal::Promote), "median buy 2.0 > 1.0");
    }
    let mut r = Radar::new(base());
    newborn(&mut r, 1, 0);
    r.on_trade(&tr(0, 99, false, 0.1));
    for i in 1..8 {
        assert_ne!(r.on_trade(&tr(i, i as u32, true, 0.3)), Some(Signal::Promote), "buy/sell > 4");
    }
}

#[test]
fn old_coin_not_blocked_by_median_buy() {
    let mut r = Radar::new(base());
    let mut got = None;
    for i in 0..6 {
        got = got.or(r.on_trade(&tr(i, i as u32, true, 2.0)).filter(|s| *s == Signal::Promote));
    }
    assert_eq!(got, Some(Signal::Promote));
    let mut r = Radar::new(base());
    newborn(&mut r, 1, 0);
    for i in 0..6 {
        assert_ne!(r.on_trade(&tr(i, i as u32, true, 2.0)), Some(Signal::Promote));
    }
}

#[test]
fn old_coin_buy_sell_ratio_configurable() {
    let run = |ratio: Option<f64>| {
        let mut r = Radar::new(Params { old_max_buy_sell_ratio: ratio, watch: false, old_flow_rule: false, ..base() });
        r.on_trade(&tr(0, 99, false, 0.25));
        (1..=5).any(|i| r.on_trade(&tr(i, i as u32, true, 0.3)) == Some(Signal::Promote)) // 1.5 / 0.25 = 6
    };
    assert!(!run(Some(4.0)));
    assert!(run(None));
}

#[test]
fn dust_kept_tagged_not_real() {
    let mut r = Radar::new(Params { watch: false, ..base() });
    for i in 0..30 {
        assert_eq!(r.on_trade(&tr(i, i as u32, true, 0.001)), None);
    }
    let v = r.view(1, 29).unwrap();
    assert_eq!((v.metrics.dust_n_win, v.metrics.clusters_win), (30, 0));
    assert!((v.metrics.dust_top_cluster_share_win.unwrap() - 1.0 / 30.0).abs() < 1e-12);
}

#[test]
fn split_buying_one_cluster_triggers_watch() {
    let p = Params { split_min_sol: 0.2, ..base() };
    let mk = |i: i64, cluster: u32| Trade { cluster, ..Trade::basic(i, i as u64, 1, (i % 10) as u32, true, 0.009, 1.0) };
    let mut r = Radar::new(p);
    let got: Vec<_> = (0..30).filter_map(|i| r.on_trade(&mk(i, 99))).collect();
    assert_eq!(got.first(), Some(&Signal::Watch));
    let mut r = Radar::new(p);
    assert!((0..30).all(|i| r.on_trade(&mk(i, 1000 + (i % 10) as u32)) != Some(Signal::Watch)));
}

#[test]
fn cluster_aggregation_counts_one_trader() {
    let p = Params { watch: false, ..base() };
    let mut r = Radar::new(p);
    let one = |i: u32| Trade { cluster: 7, ..tr(i as i64, i, true, 0.3) };
    assert!((0..6).all(|i| r.on_trade(&one(i)) != Some(Signal::Promote)));
    let mut r = Radar::new(p);
    assert!((0..6).any(|i| r.on_trade(&tr(i as i64, i, true, 0.3)) == Some(Signal::Promote)));
}

#[test]
fn creator_buys_excluded() {
    let c = |i: u32| Trade { by_creator: true, ..tr(i as i64, i, true, 0.3) };
    let mut r = Radar::new(Params { watch: false, ..base() });
    for i in 0..6 {
        r.on_trade(&c(i));
    }
    assert_eq!((r.view(1, 5).unwrap().metrics.clusters_win, r.state(1)), (0, RadarState::Cold));
    let mut r = Radar::new(Params { watch: false, exclude_creator: false, ..base() });
    assert!((0..6).any(|i| r.on_trade(&c(i)) == Some(Signal::Promote)));
}

#[test]
fn watch_hot_decide_rate_limited() {
    let p = Params { decide_every_s: 5, decide_max_age_s: 60, ..base() };
    let mut r = Radar::new(p);
    let mut sig = Vec::new();
    for i in 0..200i64 {
        sig.push((i, r.on_trade(&tr(i, i as u32, true, 0.3))));
    }
    let promote = sig.iter().find(|x| x.1 == Some(Signal::Promote)).unwrap().0;
    let decides: Vec<i64> = sig.iter().filter(|x| x.1 == Some(Signal::Decide)).map(|x| x.0).collect();
    assert!(!decides.is_empty());
    assert!(decides.windows(2).all(|w| w[1] - w[0] >= 5));
    assert!(decides.iter().all(|t| *t > promote && *t <= promote + 60), "none after decide_until: {decides:?}");
}

#[test]
fn decide_disabled_gives_promote_only() {
    let mut r = Radar::new(Params { decide_every_s: 0, watch: false, ..base() });
    let sig: Vec<Signal> = (0..100i64).filter_map(|i| r.on_trade(&tr(i, i as u32, true, 0.3))).collect();
    assert_eq!(sig, vec![Signal::Promote]);
}

#[test]
fn unwatch_after_idle_and_watch_max() {
    let mut r = Radar::new(Params { watch_max_s: 100, ..base() });
    newborn(&mut r, 1, 0);
    assert_eq!(r.on_trade(&tr(0, 1, true, 0.3)), Some(Signal::Watch));
    let mut out = Vec::new();
    r.sweep(301, &mut out);
    assert_eq!(out, vec![(1, Signal::Unwatch)], "idle > demote_idle_s");
    let mut r = Radar::new(Params { watch_max_s: 100, ..base() });
    newborn(&mut r, 1, 0);
    r.on_trade(&tr(0, 1, true, 0.3));
    for t in (20..120).step_by(20) {
        r.on_trade(&tr(t, 1, true, 0.3)); // same buyer: no new trigger
    }
    let mut out = Vec::new();
    r.sweep(120, &mut out);
    assert_eq!(out, vec![(1, Signal::Unwatch)], "no trigger for > watch_max_s");
}

#[test]
fn new_path_expires_after_new_max_age() {
    let mut r = Radar::new(Params { watch: false, ..base() });
    newborn(&mut r, 1, 0);
    r.on_trade(&tr(0, 999, true, 0.02));
    let got = (2000..2006).any(|i| r.on_trade(&tr(i, i as u32, true, 2.0)) == Some(Signal::Promote));
    assert!(got, "age 2000 > 1800 -> old path, median block lifted");
}

#[test]
fn cusum_alarm_puts_flat_old_coin_on_watch() {
    let p = Params {
        watch_min_clusters: 1000,
        split_min_buys: 1000,
        min_buyers: 1000,
        old_flow_rule: false,
        cusum: CusumParams { warmup_n: 6, pop_min: 1_000_000, ..Default::default() },
        ..base()
    };
    let run = |rise: f64| {
        let mut r = Radar::new(p);
        let mut u = 0u32;
        for s in 0..30i64 {
            u += 1;
            r.on_trade(&Trade { mprice: 1.0, ..tr(s * 5, u % 3, true, 0.05) });
        }
        let mut got = None;
        for s in 30..40i64 {
            for k in 0..10 {
                u += 1;
                let px = 1.0 + rise * ((s - 30) * 10 + k) as f64 / 100.0;
                got = got.or(r.on_trade(&Trade { mprice: px, ..tr(s * 5 + k / 3, 1000 + u, true, 0.05) }).filter(|x| *x == Signal::Watch));
            }
        }
        (got, r.view(1, 200).unwrap().last_alarm.is_some())
    };
    let (w, alarmed) = run(0.0);
    assert_eq!(w, Some(Signal::Watch));
    assert!(alarmed);
    let (w, alarmed) = run(3.0);
    assert!(alarmed);
    assert_eq!(w, None, "price already +30%: not flat");
}

#[test]
fn flow_rule_promotes_old_coin_on_nfl_and_growth() {
    let p = Params { min_buyers: 1000, watch: false, ..base() };
    let run = |liq: f64| {
        let mut r = Radar::new(p);
        (0..5).any(|i| r.on_trade(&Trade { liq_sol: liq, ..tr(100 + i as i64, i, true, 0.15) }) == Some(Signal::Promote))
    };
    assert!(run(20.0), "5 clusters, NFL 0.75/20 = 0.0375, growth 5");
    assert!(!run(2.0), "liquidity below old_min_liq_sol");
}

#[test]
fn radar_params_json_still_loads() {
    let v: serde_json::Value = serde_json::from_str(include_str!("../../../config/radar_params.json")).unwrap();
    let p: Params = serde_json::from_value(v["params"].clone()).unwrap();
    assert_eq!((p.min_buyers, p.min_buy_sol, p.lift, p.slow_min_buyers), (4, 1.0, 1.15, 6));
    let d = Params::default();
    assert_eq!((p.watch, p.decide_every_s, p.cusum, p.newborn_mode), (d.watch, d.decide_every_s, d.cusum, d.newborn_mode));
}

#[test]
fn params_json_roundtrip() {
    let d = Params::default();
    let back: Params = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
    assert_eq!(back, d);
    let v: serde_json::Value = serde_json::from_str(include_str!("../../../config/radar_params_v4_legacy.json")).unwrap();
    let l: Params = serde_json::from_value(v["params"].clone()).unwrap();
    let back: Params = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
    assert_eq!(back, l);
    assert!(!l.cusum.enabled && !l.watch && l.newborn_auto_hot && l.new_max_age_s.is_none());
}

/// Legacy params + the newborn shim reproduce every Promote / Demote of the frozen V4 radar on a random stream
/// (dust, sells, many coins and traders, price paths).
#[test]
#[allow(deprecated)]
fn legacy_mode_matches_v4_reference_on_random_stream() {
    let v: serde_json::Value = serde_json::from_str(include_str!("../../../config/radar_params_v4_legacy.json")).unwrap();
    let lp: Params = serde_json::from_value(v["params"].clone()).unwrap();
    let rp: v4_ref::Params = serde_json::from_value(v["params"].clone()).unwrap();
    for seed in [1u64, 7, 42] {
        let mut a = Radar::new(lp).with_newborn_after(60);
        let mut b = v4_ref::Radar::new(rp).with_newborn_after(60);
        let mut x = seed;
        let mut next = || {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            x >> 33
        };
        let mut px = vec![1.0f64; 60];
        let (mut last, mut sa, mut sb) = (i64::MIN, Vec::new(), Vec::new());
        let mut n_promote = 0;
        for i in 0..30_000u64 {
            let t = (i / 10) as i64;
            let coin = (next() % 60) as u32;
            let trader = (next() % 200) as u32;
            let buy = next() % 3 != 0;
            let sol = [0.001, 0.005, 0.02, 0.1, 0.3, 0.8, 2.0][(next() % 7) as usize];
            px[coin as usize] *= if buy { 1.0 + sol * 0.05 } else { 1.0 - sol * 0.04 };
            if t > last {
                last = t;
                sa.clear();
                sb.clear();
                a.sweep(t, &mut sa);
                b.sweep(t, &mut sb);
                sa.sort_by_key(|s| s.0);
                sb.sort_by_key(|s| s.0);
                let conv: Vec<(u32, bool)> = sb.iter().map(|s| (s.0, s.1 == v4_ref::Signal::Demote)).collect();
                assert_eq!(sa.iter().map(|s| (s.0, s.1 == Signal::Demote)).collect::<Vec<_>>(), conv, "sweep at {t}");
            }
            let tn = Trade::basic(t, i, coin, trader, buy, sol, px[coin as usize]);
            let to = v4_ref::Trade { time: t, order: i, coin, trader, buy, sol, price: px[coin as usize] };
            let (ga, gb) = (a.on_trade(&tn), b.on_trade(&to));
            assert_eq!(ga == Some(Signal::Promote), gb == Some(v4_ref::Signal::Promote), "trade {i} seed {seed}");
            assert!(ga.is_none() || ga == Some(Signal::Promote), "no V6 signals in legacy mode");
            n_promote += (ga == Some(Signal::Promote)) as usize;
        }
        assert!(n_promote > 50, "stream must exercise the rules ({n_promote})");
        assert_eq!((a.hot_now, a.hot_max), (b.hot_now, b.hot_max));
    }
}
