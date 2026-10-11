//! Stage 0 binary: record live feeds, replay recordings, and produce Gate 0 measurements.

mod backtest;
mod census;
mod diagnose;
mod follow;
mod label_diag;
mod leg_audit;
mod mint_safety;
mod pump_check;
mod radar_eval;
mod radar_feed;
mod logging;
mod ml_sim;
mod strategy_v5;
mod units;
mod v5_eval;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use config::{Cluster, Redactor, Settings, VenueRegistry};
use events::{Action, Pipeline, ProgramFilter, RawMsg};
use feed::rpc::Rpc;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::{mpsc, watch};

#[derive(Parser)]
#[command(name = "sniper", about = "Solana DEX sniper V4 — Stage 0 tooling")]
struct Cli {
    /// Path to the .env file.
    #[arg(long, global = true, default_value = ".env")]
    env: PathBuf,
    /// Venue registry.
    #[arg(long, global = true, default_value = "config/venues.toml")]
    venues: PathBuf,
    /// Starting capital of every simulated account, SOL (accounting is in SOL).
    #[arg(long, global = true, default_value_t = units::DEFAULT_START_SOL)]
    start_sol: f64,
    /// Display-only SOL/USD rate for USD fields in reports (never fetched; absent = null).
    #[arg(long, global = true)]
    sol_usd: Option<f64>,
    /// Label horizon, seconds (labels, censoring, purge). 36000 = 10 h (needs follow-up data); 3600 with a
    /// one-hour extension backfill (decision_window.json).
    #[arg(long, global = true, default_value_t = ml_sim::H_S)]
    label_horizon_s: i64,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Record live feeds to a directory (raw recording + live derived output).
    Record {
        #[arg(long, default_value = "devnet")]
        cluster: String,
        /// Comma-separated: grpc, ws.
        #[arg(long, default_value = "grpc")]
        feeds: String,
        /// `venues` = transactions touching the venue programs; `all-nonvote` = every non-vote tx.
        #[arg(long, default_value = "venues")]
        profile: String,
        #[arg(long)]
        out: PathBuf,
        /// Stop after this many seconds (0 = until Ctrl-C).
        #[arg(long, default_value_t = 0)]
        duration_secs: u64,
        /// Finalized-slot distance before a slot's chain is checked.
        #[arg(long, default_value_t = 150)]
        gap_grace_slots: u64,
        /// Forced disconnects (G0.2): number of kills of the feed's TCP connections.
        #[arg(long, default_value_t = 0)]
        chaos_kills: u64,
        #[arg(long, default_value_t = 60)]
        chaos_interval_secs: u64,
        /// Feed whose connections the chaos task kills: grpc or ws.
        #[arg(long, default_value = "grpc")]
        chaos_feed: String,
        #[arg(long, default_value_t = 3600)]
        rotate_secs: u64,
    },
    /// Replay a recording through the pipeline; prints the derived-output SHA-256.
    Replay {
        #[arg(long)]
        rec: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Offline Gate 0 report: latency (G0.5), reconnect recovery (G0.2), duplicate audit.
    Report {
        #[arg(long)]
        rec: PathBuf,
        #[arg(long, default_value_t = 60_000.0)]
        recover_within_ms: f64,
    },
    /// Independent check (G0.1): re-fetch sampled finalized slots with getBlock and compare tx sets.
    VerifyChain {
        #[arg(long)]
        rec: PathBuf,
        #[arg(long, default_value_t = 200)]
        sample: usize,
        /// Check every slot inside each disconnect window (from the last slot seen before the
        /// disconnect marker to 10 slots after the first slot seen after it) instead of sampling.
        #[arg(long)]
        outages: bool,
    },
    /// Check that every venue program ID is an executable account on mainnet.
    VerifyPrograms,
    /// Historical backfill (G0.H1–H3): every slot of a past range via getBlocks + getBlock, venue txs only.
    Backfill {
        #[arg(long, default_value = "mainnet")]
        cluster: String,
        #[arg(long)]
        out: PathBuf,
        /// Last slot of the range (default: current finalized slot - 200).
        #[arg(long)]
        end_slot: Option<u64>,
        /// Number of slots in the range (1 hour ~ 9,000 slots).
        #[arg(long)]
        slots: u64,
        #[arg(long, default_value_t = 8)]
        concurrency: usize,
        /// Skipped slots (absent from getBlocks) re-checked with getBlock, as an independent confirmation.
        #[arg(long, default_value_t = 30)]
        skipped_spot_checks: usize,
    },
    /// Coin census on a backfill recording: price path per coin, trader activity, buy/sell sizes.
    Census {
        #[arg(long)]
        rec: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Tune radar params on DEVELOPMENT recordings only (v6 16-point grid, median dev-window rule, ablation; or the
    /// old V4 grid with --legacy); writes the frozen params file.
    RadarTune {
        /// Development recordings (repeatable). Never pass a final holdout here.
        #[arg(long, required = true)]
        rec: Vec<PathBuf>,
        #[arg(long, default_value = "config/radar_params.json")]
        params_out: PathBuf,
        /// Params not in the grid come from this file.
        #[arg(long, default_value = "config/radar_params.json")]
        base_params: PathBuf,
        /// auto | events | raw
        #[arg(long, default_value = "auto")]
        source: String,
        #[arg(long)]
        legacy: bool,
        /// HOT budget: promotions per minute allowed on every dev window (fixed before results).
        #[arg(long, default_value_t = 30.0)]
        max_hot_per_min: f64,
        /// Report path (grid + ablation).
        #[arg(long, default_value = "reports/v6/RADAR_TUNE.json")]
        report: PathBuf,
    },
    /// Write <rec>/creations.bin.zst (window-wide creation events) from the raw segments of a decoded recording.
    ScanCreations {
        #[arg(long)]
        rec: PathBuf,
    },
    /// Evaluate frozen radar params on any recording (holdouts).
    RadarEval {
        #[arg(long)]
        rec: PathBuf,
        #[arg(long, default_value = "config/radar_params.json")]
        params: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        /// auto | events | raw
        #[arg(long, default_value = "auto")]
        source: String,
    },
    /// Verify pump.fun / PumpSwap decoding vs the reference decoder, and the pool math.
    PumpCheck {
        #[arg(long)]
        rec: PathBuf,
        /// Dump the first N decoded events per venue as JSON lines to this file and stop after M blocks.
        #[arg(long)]
        dump: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        max_blocks: usize,
    },
    /// EXPLORATORY rule-based backtest (capital --start-sol): tune on --dev windows only, report every window.
    Backtest {
        #[arg(long, required = true)]
        dev: Vec<PathBuf>,
        #[arg(long, required = true)]
        test: Vec<PathBuf>,
        #[arg(long, default_value = "config/radar_params.json")]
        radar_params: PathBuf,
        #[arg(long, default_value = "reports/backtest_exploratory.json")]
        out: PathBuf,
        #[arg(long, default_value_t = 2)]
        latency_slots: u64,
        #[arg(long, default_value_t = 500_000)]
        tx_cost_lamports: u64,
        #[arg(long, default_value_t = 0.15)]
        buy_fail_prob: f64,
    },
    /// Forensics of the exploratory backtest (runners not monetised, losing trades).
    Diagnose {
        #[arg(long, required = true)]
        rec: Vec<PathBuf>,
        #[arg(long, default_value = "config/radar_params.json")]
        radar_params: PathBuf,
    },
    /// Write causal feature snapshots + V5 labels (<rec>/snapshots_v5.csv.zst, window + follow-up) for ML
    /// training; windows run in parallel on all cores.
    Snapshots {
        #[arg(long, required = true)]
        rec: Vec<PathBuf>,
        #[arg(long, default_value = "config/radar_params.json")]
        radar_params: PathBuf,
    },
    /// EXPLORATORY ML backtest: thresholds tuned on --dev (out-of-sample cross scores), then --test.
    BacktestMl {
        #[arg(long, required = true)]
        dev: Vec<PathBuf>,
        #[arg(long, required = true)]
        test: Vec<PathBuf>,
        #[arg(long, default_value = "config/radar_params.json")]
        radar_params: PathBuf,
        #[arg(long, default_value = "reports/backtest_ml_exploratory.json")]
        out: PathBuf,
        /// Score file suffix, e.g. "_lightgbm" -> scores_cross_lightgbm.csv / scores_final_lightgbm.csv.
        #[arg(long, default_value = "")]
        scores_suffix: String,
    },
    /// Micro audit of the ML system on one recording: per runner, scores, blocks, trades.
    MlDiagnose {
        #[arg(long)]
        rec: PathBuf,
        #[arg(long)]
        scores: String,
        #[arg(long)]
        params: PathBuf,
        #[arg(long, default_value = "config/radar_params.json")]
        radar_params: PathBuf,
    },
    /// One ML simulation per recording with fixed params (no tuning): quick per-trade economics.
    MlSim {
        #[arg(long, required = true)]
        rec: Vec<PathBuf>,
        #[arg(long)]
        scores: String,
        #[arg(long)]
        params: PathBuf,
        #[arg(long, default_value = "config/radar_params.json")]
        radar_params: PathBuf,
    },
    /// Nested leave-one-window-out: for each window W, tune thresholds on the other windows (their scores come from
    /// models that never saw them nor W) and evaluate W with a model trained on all other windows.
    BacktestLowo {
        #[arg(long, required = true)]
        rec: Vec<PathBuf>,
        #[arg(long, default_value = "config/radar_params.json")]
        radar_params: PathBuf,
        #[arg(long, default_value = "reports/portfolio_lowo.json")]
        out: PathBuf,
        /// Evaluate only these test windows (names); all windows are still used for tuning.
        #[arg(long)]
        only: Vec<String>,
        /// Score file tag: scores_outer_<tag>.csv / scores_inner_excl_<W>_<tag>.csv ("" = untagged files).
        #[arg(long, default_value = "")]
        tag: String,
    },
    /// Export decoded pump.fun / PumpSwap trades of a recording as CSV (one row per trade).
    ExportEvents {
        #[arg(long)]
        rec: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// V5 tier-2 backfill: follow every coin that became HOT in the window for --hours after its HOT time
    /// (or until dead), same decoder; writes <rec>/follow.bin.zst, merged by every loader.
    Follow {
        #[arg(long)]
        rec: PathBuf,
        #[arg(long, default_value_t = 10.0)]
        hours: f64,
        #[arg(long, default_value = "config/radar_params.json")]
        radar_params: PathBuf,
        /// Global request rate limit (requests per second, all methods).
        #[arg(long, default_value_t = 9.0)]
        rps: f64,
        #[arg(long, default_value_t = 16)]
        coin_concurrency: usize,
        /// Transactions fetched concurrently per coin between dead-coin checks.
        #[arg(long, default_value_t = 16)]
        tx_batch: usize,
        /// Signature pages (1000 each) allowed to reach the window from now.
        #[arg(long, default_value_t = 3000)]
        max_range_pages: u32,
        /// Extra signature pages past the window to find the mint's first signature (creation).
        #[arg(long, default_value_t = 200)]
        max_history_pages: u32,
        /// Follow only the first N HOT coins (0 = all); for cost measurement.
        #[arg(long, default_value_t = 0)]
        limit: usize,
        /// hot (default) or watch (also follow WATCH coins; costs more credits).
        #[arg(long, default_value = "hot")]
        min_state: String,
    },
    /// V5 Phases 7-8 (EXPLORATORY): walk-forward backtest on fold scores (<models>/fold_<W>/scores_<W>.csv.zst),
    /// optional 24-point grid (median dev return), cost sensitivity, acceptance check, Monte Carlo.
    V5Backtest {
        #[arg(long, required = true)]
        rec: Vec<PathBuf>,
        #[arg(long, default_value = "models/v5")]
        models: PathBuf,
        #[arg(long, default_value = "config/radar_params.json")]
        radar_params: PathBuf,
        /// Strategy JSON (StratV5); default = V5 plan §7.5 starting values.
        #[arg(long)]
        strategy: Option<PathBuf>,
        #[arg(long, default_value = "reports/V5_RESULTS.json")]
        out: PathBuf,
        #[arg(long)]
        grid: bool,
        /// Monte Carlo execution reruns (buy-failure seeds); 0 = off.
        #[arg(long, default_value_t = 0)]
        mc_runs: usize,
        #[arg(long, default_value_t = 10_000)]
        mc_resamples: usize,
        #[arg(long, default_value_t = 2)]
        latency_slots: u64,
        #[arg(long, default_value_t = 0.15)]
        buy_fail_prob: f64,
    },
    /// EXPLORATORY: V4 rule strategy vs V5 corrections without ML on the same windows, plus a per-runner table.
    V5Compare {
        #[arg(long, required = true)]
        rec: Vec<PathBuf>,
        #[arg(long, default_value = "config/radar_params.json")]
        radar_params: PathBuf,
        #[arg(long, default_value = "config/strategy_params.json")]
        v4_strategy: PathBuf,
        #[arg(long, default_value = "reports/V5_COMPARE.json")]
        out: PathBuf,
        /// V5 strategy JSON (default: plan §7.5 values).
        #[arg(long)]
        v5_strategy: Option<PathBuf>,
    },
    /// Snapshots with short-horizon labels (model-family tests on windows without follow-up data).
    SnapshotsShort {
        #[arg(long, required = true)]
        rec: Vec<PathBuf>,
        #[arg(long, default_value = "config/radar_params.json")]
        radar_params: PathBuf,
        #[arg(long, default_value_t = 600)]
        horizon_s: i64,
    },
    /// Append an extension backfill (the slots right after a window, separate `backfill --out EXT`) to a window:
    /// moves EXT's raw segments into REC, writes decision_window.json (decisions only in the original window) and
    /// removes REC's decoded caches so the next load decodes window + extension (all coins, all venues).
    ExtendWindow {
        #[arg(long)]
        rec: PathBuf,
        #[arg(long)]
        ext: PathBuf,
    },
    /// Component ablation: each radar / strategy component switched off alone on the same windows.
    Ablate {
        #[arg(long, required = true)]
        rec: Vec<PathBuf>,
        #[arg(long, default_value = "config/radar_params.json")]
        radar_params: PathBuf,
        #[arg(long)]
        strategy: Option<PathBuf>,
        #[arg(long, default_value = "reports/v6/ABLATION.json")]
        out: PathBuf,
    },
    /// V5-9 diagnostic: how often runners touch the -65% y_bin barrier (and wider ones) before their peak.
    LabelDiag {
        #[arg(long, required = true)]
        rec: Vec<PathBuf>,
        #[arg(long, default_value = "config/radar_params.json")]
        radar_params: PathBuf,
        #[arg(long, default_value = "reports/V5_9_BARRIER.json")]
        out: PathBuf,
    },
    /// Coverage audit: recent blocks fetched unfiltered; pool legs kept vs dropped by the venue-program filter.
    LegAudit {
        #[arg(long, default_value = "mainnet")]
        cluster: String,
        #[arg(long, default_value_t = 40)]
        slots: u64,
        #[arg(long, default_value = "reports/v6/LEG_AUDIT.json")]
        out: PathBuf,
        /// Audit a recording instead of fetching recent blocks (trade-vs-transfer check).
        #[arg(long)]
        rec: Option<PathBuf>,
        #[arg(long, default_value_t = 3000)]
        max_blocks: usize,
    },
    /// Mint safety flags (freeze / mint authority, Token-2022 sell-blocking extensions) for every mint of the
    /// recordings, cached in data/mint_safety.csv.
    MintSafety {
        #[arg(long, default_value = "mainnet")]
        cluster: String,
        #[arg(long, required = true)]
        rec: Vec<PathBuf>,
    },
    /// Re-fetch the listed blocks a backfill recording is missing (fetch errors) into an extra raw segment.
    BackfillRepair {
        #[arg(long, default_value = "mainnet")]
        cluster: String,
        #[arg(long)]
        rec: PathBuf,
        #[arg(long, default_value_t = 4)]
        concurrency: usize,
    },
    /// G0.H1 check on a backfill recording: every slot fetched or confirmed skipped.
    VerifyBackfill {
        #[arg(long)]
        rec: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    units::set(units::Units { start_sol: cli.start_sol, sol_usd: cli.sol_usd });
    ml_sim::set_label_horizon(cli.label_horizon_s);
    let _ = rustls::crypto::ring::default_provider().install_default();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    match cli.cmd {
        Cmd::Record {
            cluster,
            feeds,
            profile,
            out,
            duration_secs,
            gap_grace_slots,
            chaos_kills,
            chaos_interval_secs,
            chaos_feed,
            rotate_secs,
        } => {
            let cluster_e: Cluster = cluster.parse()?;
            let settings = Settings::load(&cli.env, cluster_e)?;
            logging::init(Redactor::new(settings.secrets()), Some(&out.join("run.log")))?;
            let venues = VenueRegistry::load(&cli.venues)?;
            let programs = match profile.as_str() {
                "venues" => Some(venues.program_ids()),
                "all-nonvote" => None,
                p => bail!("unknown profile {p}"),
            };
            let feeds: Vec<String> = feeds.split(',').map(|s| s.trim().to_string()).collect();
            if programs.is_none() && feeds.iter().any(|f| f == "ws") {
                bail!("ws feed needs the venues profile (logsSubscribe is per program)");
            }
            let opts = RecordOpts {
                cluster,
                feeds,
                programs,
                out,
                duration_secs,
                gap_grace_slots,
                chaos_kills,
                chaos_interval_secs,
                chaos_feed,
                rotate_secs,
            };
            rt.block_on(record(settings, opts))
        }
        Cmd::Replay { rec, out } => {
            let mut f = std::fs::File::create(&out)?;
            let r = replay::replay(&rec, &mut f)?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        Cmd::Report { rec, recover_within_ms } => {
            let lat = replay::latency_report(&rec)?;
            let rec_rep = replay::recovery_report(&rec, recover_within_ms)?;
            let live = rec.join("derived.live.txt");
            let dup = if live.exists() { Some(replay::audit_duplicates(&live)?) } else { None };
            let v = serde_json::json!({ "latency": lat, "recovery": rec_rep, "duplicate_audit_live_derived": dup });
            println!("{}", serde_json::to_string_pretty(&v)?);
            Ok(())
        }
        Cmd::VerifyChain { rec, sample, outages } => rt.block_on(verify_chain(&cli.env, &rec, sample, outages)),
        Cmd::VerifyPrograms => rt.block_on(verify_programs(&cli.env, &cli.venues)),
        Cmd::Backfill { cluster, out, end_slot, slots, concurrency, skipped_spot_checks } => {
            let settings = Settings::load(&cli.env, cluster.parse()?)?;
            logging::init(Redactor::new(settings.secrets()), Some(&out.join("run.log")))?;
            let venues = VenueRegistry::load(&cli.venues)?;
            rt.block_on(backfill(settings, cluster, venues.program_ids(), out, end_slot, slots, concurrency, skipped_spot_checks))
        }
        Cmd::VerifyBackfill { rec } => verify_backfill(&rec),
        Cmd::BackfillRepair { cluster, rec, concurrency } => {
            let settings = Settings::load(&cli.env, cluster.parse()?)?;
            logging::init(Redactor::new(settings.secrets()), None)?;
            let venues = VenueRegistry::load(&cli.venues)?;
            rt.block_on(backfill_repair(settings, venues.program_ids(), rec, concurrency))
        }
        Cmd::MintSafety { cluster, rec } => {
            let settings = Settings::load(&cli.env, cluster.parse()?)?;
            logging::init(Redactor::new(settings.secrets()), None)?;
            let mut mints = Vec::new();
            for r in &rec {
                mints.extend(backtest::load(r)?.mints);
            }
            let m = rt.block_on(mint_safety::fetch(settings.rpc_url.clone(), mints.clone(), std::path::Path::new(mint_safety::CACHE)))?;
            let mut c: std::collections::BTreeMap<&str, usize> = Default::default();
            for k in mints.iter().collect::<std::collections::BTreeSet<_>>() {
                let Some(s) = m.get(k) else { continue };
                *c.entry("mints").or_default() += 1;
                for (n, f) in [("missing", !s.exists), ("token2022", s.token2022), ("mint_auth", s.mint_auth), ("freeze_auth", s.freeze_auth),
                    ("transfer_fee", s.transfer_fee_bps > 0), ("transfer_hook", s.transfer_hook), ("permanent_delegate", s.permanent_delegate),
                    ("default_frozen", s.default_frozen), ("honeypot_risk", s.honeypot_risk())] {
                    *c.entry(n).or_default() += f as usize;
                }
            }
            println!("{}", serde_json::to_string_pretty(&c)?);
            Ok(())
        }
        Cmd::LegAudit { cluster, slots, out, rec, max_blocks } => {
            if let Some(r) = rec {
                let venues = VenueRegistry::load(&cli.venues)?;
                return leg_audit::run_rec(&r, venues.program_ids(), max_blocks, &out);
            }
            let settings = Settings::load(&cli.env, cluster.parse()?)?;
            logging::init(Redactor::new(settings.secrets()), None)?;
            let venues = VenueRegistry::load(&cli.venues)?;
            rt.block_on(leg_audit::run(settings.rpc_url.clone(), venues.program_ids(), slots, &out))
        }
        Cmd::SnapshotsShort { rec, radar_params, horizon_s } => {
            use rayon::prelude::*;
            let rp = radar_feed::load_params(&radar_params)?;
            rec.par_iter().map(|r| -> Result<()> {
                let ev = backtest::load(r)?;
                let ctx = ml_sim::load_context(r, &ev)?;
                let (rows, dropped) = ml_sim::write_snapshots_short(&ev, rp, ctx, &r.join(format!("snapshots_short_{horizon_s}.csv.zst")), horizon_s)?;
                println!("{}: rows {rows} dropped_uncovered {dropped} horizon {horizon_s}s", r.display());
                Ok(())
            }).collect::<Result<Vec<()>>>()?;
            Ok(())
        }
        Cmd::ExtendWindow { rec, ext } => {
            if rec.join("decision_window.json").exists() {
                bail!("{}: already extended", rec.display());
            }
            let ev = backtest::load_window(&rec)?;
            let (es, rs) = (read_summary(&ext)?, read_summary(&rec)?);
            let (r_end, e_start) = (rs["end_slot"].as_u64().unwrap_or(0), es["start_slot"].as_u64().unwrap_or(0));
            if e_start != r_end + 1 {
                bail!("extension starts at slot {e_start}, window ends at {r_end}: not contiguous");
            }
            let mut segs: Vec<PathBuf> = std::fs::read_dir(&ext)?.flatten().map(|e| e.path())
                .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("raw-") && n.ends_with(".zst"))).collect();
            segs.sort();
            if segs.is_empty() {
                bail!("{}: no raw segments", ext.display());
            }
            for (i, s) in segs.iter().enumerate() {
                std::fs::rename(s, rec.join(format!("raw-{:06}.zst", 100_000 + i)))?;
            }
            std::fs::write(rec.join("decision_window.json"), serde_json::to_string_pretty(&serde_json::json!({
                "decision_end": ev.window.1, "decision_start": ev.window.0, "window_end_slot": r_end,
                "extension_slots": es["slots_in_range"], "extension_end_slot": es["end_slot"] }))?)?;
            std::fs::copy(ext.join("summary.json"), rec.join("extension_summary.json"))?;
            for f in [backtest::CACHE_V4, backtest::CACHE_V3, backtest::CREATIONS_FILE, "pump_events_v2.bin", "venue_decode_stats.json"] {
                let _ = std::fs::remove_file(rec.join(f));
            }
            println!("{}: decision window {:?}, {} extension segments moved; decode with export-events", rec.display(), ev.window, segs.len());
            Ok(())
        }
        Cmd::Ablate { rec, radar_params, strategy, out } => {
            let rp = radar_feed::load_params(&radar_params)?;
            let st: strategy_v5::StratV5 = match strategy { Some(p) => serde_json::from_str(&std::fs::read_to_string(p)?)?, None => Default::default() };
            v5_eval::ablate(&rec, rp, &st, &out)
        }
        Cmd::LabelDiag { rec, radar_params, out } => {
            let rp = radar_feed::load_params(&radar_params)?;
            label_diag::run(&rec, rp, &out)
        }
        Cmd::V5Compare { rec, radar_params, v4_strategy, out, v5_strategy } => {
            let st: strategy_v5::StratV5 = match v5_strategy { Some(p) => serde_json::from_str(&std::fs::read_to_string(p)?)?, None => Default::default() };
            let rp = radar_feed::load_params(&radar_params)?;
            let v4: backtest::Strategy = serde_json::from_str(&std::fs::read_to_string(&v4_strategy)?)?;
            v5_eval::compare(&rec, rp, &v4, &st, &out)
        }
        Cmd::V5Backtest { rec, models, radar_params, strategy, out, grid, mc_runs, mc_resamples, latency_slots, buy_fail_prob } => {
            let rp = radar_feed::load_params(&radar_params)?;
            let st: strategy_v5::StratV5 = match strategy { Some(p) => serde_json::from_str(&std::fs::read_to_string(p)?)?, None => Default::default() };
            let costs = strategy_v5::CostsV5 { latency_slots, buy_fail_prob, ..Default::default() };
            let o = v5_eval::EvalOpts { models, out, grid, mc_runs, mc_resamples, costs, strategy: st };
            v5_eval::run(&rec, rp, &o)
        }
        Cmd::Follow { rec, hours, radar_params, rps, coin_concurrency, tx_batch, max_range_pages, max_history_pages, limit, min_state } => {
            let settings = Settings::load(&cli.env, Cluster::Mainnet)?;
            logging::init(Redactor::new(settings.secrets()), Some(&rec.join("follow.log")))?;
            let rp = radar_feed::load_params(&radar_params)?;
            let min_state = match min_state.as_str() { "hot" => radar::RadarState::Hot, "watch" => radar::RadarState::Watch, s => bail!("--min-state {s}: hot|watch") };
            let o = follow::Opts { hours, rps, coin_concurrency, tx_batch, max_range_pages, max_history_pages, limit, min_state };
            rt.block_on(follow::run(settings.rpc_url.clone(), &rec, rp, o))
        }
        Cmd::Census { rec, out } => census::run(&rec, &out),
        Cmd::Backtest { dev, test, radar_params, out, latency_slots, tx_cost_lamports, buy_fail_prob } => {
            let rp = radar_feed::load_params(&radar_params)?;
            let costs = backtest::Costs { latency_slots, tx_cost_lamports, buy_fail_prob, ..Default::default() };
            let load = |r: &PathBuf| -> Result<(String, backtest::Events, f64)> {
                let e = backtest::load(r)?;
                let usd = units::sol_usd();
                Ok((r.display().to_string(), e, usd))
            };
            let devs: Vec<_> = dev.iter().map(load).collect::<Result<_>>()?;
            let tests: Vec<_> = test.iter().map(load).collect::<Result<_>>()?;
            let dev_refs: Vec<(&backtest::Events, f64)> = devs.iter().map(|(_, e, u)| (e, *u)).collect();
            let (best, grid) = backtest::tune(&dev_refs, &costs, rp);
            let mut windows = Vec::new();
            for (role, set) in [("dev", &devs), ("test", &tests)] {
                for (name, e, usd) in set.iter() {
                    let r = backtest::simulate(e, &best, &costs, rp, units::start_sol(), *usd);
                    let base = backtest::simulate(e, &backtest::Strategy { trailing_only: Some(0.3), ..best }, &costs, rp, units::start_sol(), *usd);
                    windows.push(serde_json::json!({ "role": role, "recording": name, "mee": r,
                        "baseline_trailing30": { "final_usd": base.final_usd, "return_pct": base.return_pct, "trades": base.trades,
                            "win_rate": base.win_rate, "max_drawdown_pct": base.max_drawdown_pct } }));
                }
            }
            let rep = serde_json::json!({
                "label": "EXPLORATORY RULE-BASED BACKTEST (pre-gate, user-requested); pump.fun curve + PumpSwap SOL pools only",
                "strategy_frozen_on_dev": best, "costs": costs, "radar_params": rp, "grid_size": grid.len(), "windows": windows,
            });
            if let Some(d) = out.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&out, serde_json::to_string_pretty(&rep)?)?;
            std::fs::write("config/strategy_params.json", serde_json::to_string_pretty(&best)?)?;
            println!("written {}", out.display());
            Ok(())
        }
        Cmd::Diagnose { rec, radar_params } => diagnose::run(&rec, &radar_params),
        Cmd::Snapshots { rec, radar_params } => {
            use rayon::prelude::*;
            let rp = radar_feed::load_params(&radar_params)?;
            rec.par_iter().map(|r| -> Result<()> {
                let ev = backtest::load(r)?;
                let ctx = ml_sim::load_context(r, &ev)?;
                let (n_w, n_sol) = (ctx.wallets.iter().filter(|w| w.is_some()).count(), ctx.sol_usd.len());
                let st = ml_sim::write_snapshots(&ev, rp, ctx, &r.join("snapshots_v5.csv.zst"))?;
                let v = serde_json::json!({ "stats": st, "wallets_with_scores": n_w, "sol_usd_minutes": n_sol,
                    "window": ev.window, "data_end": ev.data_end(), "horizon_s": ml_sim::label_h() });
                std::fs::write(r.join("snapshots_v5_stats.json"), serde_json::to_string_pretty(&v)?)?;
                println!("{}: {}", r.display(), serde_json::to_string(&v)?);
                Ok(())
            }).collect::<Result<Vec<()>>>()?;
            Ok(())
        }
        Cmd::BacktestMl { dev, test, radar_params, out, scores_suffix } => {
            let rp = radar_feed::load_params(&radar_params)?;
            let costs = backtest::Costs::default();
            let load = |r: &PathBuf, scores: &str| -> Result<(String, backtest::Events, ml_sim::Scores, f64)> {
                let e = backtest::load(r)?;
                let usd = units::sol_usd();
                Ok((r.display().to_string(), e, ml_sim::read_scores(&r.join(scores))?, usd))
            };
            let (cross, fin) = (format!("scores_cross{scores_suffix}.csv"), format!("scores_final{scores_suffix}.csv"));
            let devs: Vec<_> = dev.iter().map(|r| load(r, &cross)).collect::<Result<_>>()?;
            let tests: Vec<_> = test.iter().map(|r| load(r, &fin)).collect::<Result<_>>()?;
            let refs: Vec<(&backtest::Events, &ml_sim::Scores, f64)> = devs.iter().map(|(_, e, s, u)| (e, s, *u)).collect();
            let (best, grid) = ml_sim::tune(&refs, rp, &costs);
            let mut windows = Vec::new();
            for (role, set) in [("dev (cross-model OOS scores)", &devs), ("test (final model, never seen)", &tests)] {
                for (name, e, s, usd) in set.iter() {
                    let cov = ml_sim::warn_score_coverage(name, e, rp, s);
                    windows.push(serde_json::json!({ "role": role, "recording": name, "score_coverage": cov, "report": ml_sim::simulate(e, rp, s, &best, &costs, *usd) }));
                }
            }
            let rep = serde_json::json!({ "label": "EXPLORATORY ML BACKTEST (pre-gate, user-requested); pump.fun curve + PumpSwap SOL pools",
                "strategy_frozen_on_dev": best, "costs": costs, "grid_size": grid.len(), "windows": windows,
                "grid_dev_returns": grid.iter().map(|(s, r)| serde_json::json!({"s": s, "dev_returns": r})).collect::<Vec<_>>() });
            std::fs::write(&out, serde_json::to_string_pretty(&rep)?)?;
            std::fs::write(format!("config/ml_strategy_params{scores_suffix}.json"), serde_json::to_string_pretty(&best)?)?;
            println!("written {}", out.display());
            Ok(())
        }
        Cmd::MlDiagnose { rec, scores, params, radar_params } => {
            let rp = radar_feed::load_params(&radar_params)?;
            let st: ml_sim::MlStrategy = serde_json::from_str(&std::fs::read_to_string(&params)?)?;
            let ev = backtest::load(&rec)?;
            let usd = units::sol_usd();
            let sc = ml_sim::read_scores(&rec.join(&scores))?;
            let cov = ml_sim::warn_score_coverage(&rec.display().to_string(), &ev, rp, &sc);
            let r = ml_sim::simulate(&ev, rp, &sc, &st, &backtest::Costs::default(), usd);
            // Runners: coins whose snapshots show >= 5x reachable (exp(y_logmax), V5 labels) at some point.
            let snaps = String::from_utf8(zstd::decode_all(std::fs::File::open(rec.join("snapshots_v5.csv.zst"))?)?)?;
            let mut hdr = snaps.lines().next().unwrap_or("").split(',');
            let iy = hdr.position(|h| h == "y_logmax").ok_or_else(|| anyhow!("no y_logmax"))?;
            let mut best: std::collections::BTreeMap<u32, f64> = Default::default();
            for l in snaps.lines().skip(1) {
                let v: Vec<&str> = l.split(',').collect();
                let (c, y): (u32, f64) = (v[0].parse()?, v[iy].parse::<f64>()?.exp());
                let e = best.entry(c).or_insert(0.0);
                *e = e.max(y);
            }
            let mut out = Vec::new();
            for tier in [2.0, 5.0, 10.0] {
                let coins: Vec<u32> = best.iter().filter(|(_, y)| **y >= tier).map(|(c, _)| *c).collect();
                let mut agg: std::collections::BTreeMap<&str, u32> = Default::default();
                let (mut never_over, mut entered) = (0, 0);
                let mut maxp: Vec<f32> = Vec::new();
                for c in &coins {
                    let d = &r.diag[*c as usize];
                    maxp.push(d.max_p2);
                    if d.snaps_over_entry == 0 { never_over += 1; }
                    if d.entered > 0 { entered += 1; }
                    for (k, v) in &d.blocked { *agg.entry(k).or_default() += v; }
                }
                maxp.sort_by(|a, b| a.total_cmp(b));
                let q = |f: f64| maxp.get(((maxp.len() as f64 - 1.0) * f) as usize).copied();
                let examples: Vec<serde_json::Value> = if tier >= 10.0 {
                    coins.iter().map(|c| { let d = &r.diag[*c as usize]; serde_json::json!({ "mint": &ev.mints[*c as usize][..8], "reachable": best[c],
                        "snapshots": d.snapshots, "max_p2": d.max_p2, "snaps_over_threshold": d.snaps_over_entry, "blocked": d.blocked, "entered": d.entered }) }).collect()
                } else { Vec::new() };
                out.push(serde_json::json!({ "tier_reachable": tier, "runner_coins": coins.len(), "score_never_reached_entry_threshold": never_over,
                    "entered": entered, "max_p2_median": q(0.5), "max_p2_p90": q(0.9), "block_counts_when_over_threshold": agg, "coins_10x": examples }));
            }
            let all_over: u32 = r.diag.iter().map(|d| d.snaps_over_entry).sum();
            let all_coins_over = r.diag.iter().filter(|d| d.snaps_over_entry > 0).count();
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "score_coverage": cov, "entry_threshold": st.entry_p2, "coins_with_any_score_over_threshold": all_coins_over,
                "snapshots_over_threshold": all_over, "runners": out,
                "trades": r.trades_list.iter().map(|t| serde_json::json!({"mint": &t.mint[..8], "entry_t": t.entry_t, "exit_t": t.exit_t, "ret": t.ret, "reason": t.reason, "max_mult_held": t.max_mult_held, "p2": t.p2_at_entry})).collect::<Vec<_>>() }))?);
            Ok(())
        }
        Cmd::MlSim { rec, scores, params, radar_params } => {
            let rp = radar_feed::load_params(&radar_params)?;
            let st: ml_sim::MlStrategy = serde_json::from_str(&std::fs::read_to_string(&params)?)?;
            for r in &rec {
                let ev = backtest::load(r)?;
                let usd = units::sol_usd();
                let sc = ml_sim::read_scores(&r.join(&scores))?;
                ml_sim::warn_score_coverage(&r.display().to_string(), &ev, rp, &sc);
                let rep = ml_sim::simulate(&ev, rp, &sc, &st, &backtest::Costs::default(), usd);
                let wins: Vec<&ml_sim::MlTrade> = rep.trades_list.iter().filter(|t| t.pnl_sol > 0.0).collect();
                let loss: Vec<&ml_sim::MlTrade> = rep.trades_list.iter().filter(|t| t.pnl_sol <= 0.0).collect();
                let avg = |v: &[&ml_sim::MlTrade]| if v.is_empty() { 0.0 } else { v.iter().map(|t| t.ret).sum::<f64>() / v.len() as f64 * 100.0 };
                let mut reason_pnl: std::collections::BTreeMap<&str, (u32, f64)> = Default::default();
                for t in &rep.trades_list {
                    let e = reason_pnl.entry(t.reason).or_default();
                    e.0 += 1;
                    e.1 += t.pnl_sol;
                }
                println!("{:6} {:.4} -> {:.4} SOL | PnL {:+.4} SOL | profit factor {:.2} | trades {} win {:.0}% | avg win {:+.1}% avg loss {:+.1}% | held>=2x {} >=5x {} | costs {:.4} SOL | by exit (n, SOL): {:?}",
                    r.file_name().and_then(|x| x.to_str()).unwrap_or(""), rep.start_sol, rep.final_sol, rep.pnl_sol, rep.profit_factor, rep.trades, rep.win_rate * 100.0, avg(&wins), avg(&loss),
                    rep.trades_list.iter().filter(|t| t.max_mult_held >= 2.0).count(), rep.held_to_5x, rep.costs_sol,
                    reason_pnl.iter().map(|(k, v)| format!("{k}:{}/{:+.1}", v.0, v.1)).collect::<Vec<_>>());
            }
            Ok(())
        }
        Cmd::BacktestLowo { rec, radar_params, out, only, tag } => {
            let sfx = if tag.is_empty() { String::new() } else { format!("_{tag}") };
            let rp = radar_feed::load_params(&radar_params)?;
            let costs = backtest::Costs::default();
            let name = |r: &PathBuf| r.file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
            let evs: Vec<(String, backtest::Events, f64)> = rec.iter().map(|r| { let e = backtest::load(r)?; let u = units::sol_usd(); Ok((name(r), e, u)) }).collect::<Result<_>>()?;
            let mut results = Vec::new();
            for (wi, (wname, wev, wusd)) in evs.iter().enumerate() {
                if !only.is_empty() && !only.contains(wname) {
                    continue;
                }
                let inner: Vec<(usize, ml_sim::Scores)> = evs.iter().enumerate().filter(|(i, _)| *i != wi)
                    .map(|(i, (vname, _, _))| Ok((i, ml_sim::read_scores(&rec[i].join(format!("scores_inner_excl_{wname}{sfx}.csv")))?)).map(|x: (usize, ml_sim::Scores)| { let _ = vname; x }))
                    .collect::<Result<_>>()?;
                let refs: Vec<(&backtest::Events, &ml_sim::Scores, f64)> = inner.iter().map(|(i, s)| (&evs[*i].1, s, evs[*i].2)).collect();
                let (best, _) = ml_sim::tune(&refs, rp, &costs);
                let outer = ml_sim::read_scores(&rec[wi].join(format!("scores_outer{sfx}.csv")))?;
                let cov = ml_sim::warn_score_coverage(wname, wev, rp, &outer);
                let r = ml_sim::simulate(wev, rp, &outer, &best, &costs, *wusd);
                println!("{wname}: {:.4} -> {:.4} SOL (PnL {:+.4} SOL, {:+.1}%) trades {} win rate {:.0}% profit factor {:.2} maxDD {:.0}% held>=5x {} best {:.0}% exits {:?}",
                    r.start_sol, r.final_sol, r.pnl_sol, r.return_pct, r.trades, r.win_rate * 100.0, r.profit_factor, r.max_drawdown_pct, r.held_to_5x, r.best_ret * 100.0, r.exits);
                results.push(serde_json::json!({ "test_window": wname, "score_coverage": cov, "strategy_tuned_on_other_windows": best, "report": r }));
            }
            std::fs::write(&out, serde_json::to_string_pretty(&serde_json::json!({
                "label": "EXPLORATORY nested leave-one-window-out (--start-sol per window); pump.fun curve + PumpSwap", "results": results }))?)?;
            Ok(())
        }
        Cmd::ExportEvents { rec, out } => {
            let ev = backtest::load(&rec)?;
            let mut f = std::io::BufWriter::new(std::fs::File::create(&out)?);
            writeln!(f, "slot,block_time,order,mint,venue,side,sol_lamports,token_raw,trader,creator,quote_reserve_after,token_reserve_after,real_quote,real_token,fee_bps,curve_complete,pool")?;
            for e in &ev.evs {
                writeln!(f, "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}", e.slot, e.time, e.order, ev.mints[e.coin as usize], match e.venue { 0 => "pumpfun_curve", 1 => "pumpswap", v => decoder::venue_pool::code::name(v) },
                    if e.buy { "buy" } else { "sell" }, e.sol, e.tokens, ev.users[e.user as usize], ev.users[e.creator as usize], e.quote_res, e.token_res,
                    e.real_quote, e.real_token, e.fee_bps, e.complete as u8, e.pool)?;
            }
            f.flush()?;
            println!("{}: {} trades, {} coins, {} wallets", out.display(), ev.evs.len(), ev.mints.len(), ev.users.len());
            Ok(())
        }
        Cmd::PumpCheck { rec, dump, max_blocks } => pump_check::run(&rec, dump.as_deref(), max_blocks),
        Cmd::RadarTune { rec, params_out, base_params, source, legacy, max_hot_per_min, report } => {
            let base = radar_feed::load_params(&base_params)?;
            let dss: Vec<radar_eval::Dataset> = rec.iter().map(|r| radar_eval::load_auto(r, &base, &source)).collect::<Result<_>>()?;
            let preps: Vec<radar_eval::Prepared> = dss.iter().map(radar_eval::prepare).collect();
            let sets: Vec<(&radar_eval::Dataset, &radar_eval::Prepared)> = dss.iter().zip(preps.iter()).collect();
            if legacy {
                let (best, all) = radar_eval::tune(&sets, base);
                let chosen: Vec<_> = sets.iter().map(|(d, p)| radar_eval::evaluate(d, p, best)).collect();
                std::fs::write(&params_out, serde_json::to_string_pretty(&serde_json::json!({
                    "frozen_from": rec, "params": best, "grid_size": all.len(),
                    "objective": "legacy V4 grid: maximize summed mean recall(2x,5x) on pre-existing coins s.t. precision_2x >= naive on every dev recording",
                }))?)?;
                println!("{}", serde_json::to_string_pretty(&chosen)?);
                return Ok(());
            }
            let (best, all, ablation) = radar_eval::tune_v6(&sets, base, max_hot_per_min);
            if let Some(d) = report.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&report, serde_json::to_string_pretty(&serde_json::json!({
                "dev_windows": rec, "max_hot_per_min": max_hot_per_min, "chosen": best,
                "grid": all.iter().map(|(p, rs)| serde_json::json!({"params": p, "windows": rs})).collect::<Vec<_>>(),
                "ablation": ablation.iter().map(|(n, rs)| serde_json::json!({"variant": n, "windows": rs})).collect::<Vec<_>>(),
            }))?)?;
            let Some(best) = best else {
                bail!("no grid point satisfies hot_per_min <= {max_hot_per_min} and precision_2x >= naive on every dev window; params not written (report: {})", report.display());
            };
            std::fs::write(&params_out, serde_json::to_string_pretty(&serde_json::json!({
                "frozen_from": rec, "params": best, "grid_size": all.len(), "max_hot_per_min": max_hot_per_min,
                "objective": "v6: maximize the median over dev windows of mean(recall HOT before +10%, recall HOT at +30%) at tier 5x averaged over new/old coins, s.t. hot_per_min <= max and precision_2x >= naive on every dev window; ties -> fewer promoted coins",
            }))?)?;
            println!("chosen params written to {}; report {}", params_out.display(), report.display());
            Ok(())
        }
        Cmd::ScanCreations { rec } => {
            let f = backtest::scan_creations(&rec)?;
            println!("{}: {} creations ({} migrations), window {:?}", rec.display(), f.items.len(), f.items.iter().filter(|x| x.migration).count(), f.window);
            Ok(())
        }
        Cmd::RadarEval { rec, params, out, source } => {
            let p = radar_feed::load_params(&params)?;
            let ds = radar_eval::load_auto(&rec, &p, &source)?;
            let prep = radar_eval::prepare(&ds);
            let r = radar_eval::evaluate(&ds, &prep, p);
            let text = serde_json::to_string_pretty(&r)?;
            if let Some(o) = out {
                std::fs::write(o, &text)?;
            }
            println!("{text}");
            Ok(())
        }
    }
}

struct RecordOpts {
    cluster: String,
    feeds: Vec<String>,
    programs: Option<Vec<String>>,
    out: PathBuf,
    duration_secs: u64,
    gap_grace_slots: u64,
    chaos_kills: u64,
    chaos_interval_secs: u64,
    chaos_feed: String,
    rotate_secs: u64,
}

fn git_commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "uncommitted".into())
}

fn host_of(url: &str) -> Result<String> {
    let rest = url.split("://").nth(1).ok_or_else(|| anyhow!("bad url"))?;
    Ok(rest.split(['/', ':', '?']).next().unwrap_or("").to_string())
}

/// Kill every TCP socket to `host` (G0.2 forced disconnect). Needs root (ss -K).
async fn kill_connections(host: &str) -> Result<usize> {
    let addrs: Vec<std::net::IpAddr> = tokio::net::lookup_host((host, 443)).await?.map(|a| a.ip()).collect();
    let mut n = 0;
    for ip in addrs {
        let dst = if ip.is_ipv6() { format!("[{ip}]") } else { ip.to_string() };
        let out = tokio::process::Command::new("ss").args(["-K", "-t", "dst", &dst]).output().await?;
        n += String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.contains("ESTAB")).count();
    }
    Ok(n)
}

async fn record(settings: Settings, o: RecordOpts) -> Result<()> {
    std::fs::create_dir_all(&o.out)?;
    let (tx, mut rx) = mpsc::channel::<RawMsg>(1 << 16);
    let _ = logging::SDK_EVENTS.set(tx.clone());
    let start = feed::control(
        "start",
        serde_json::json!({
            "cluster": o.cluster, "feeds": o.feeds, "programs": o.programs,
            "gap_grace_slots": o.gap_grace_slots, "commit": git_commit(),
            "chaos_kills": o.chaos_kills, "chaos_interval_secs": o.chaos_interval_secs,
        }),
    );
    let filter = match &o.programs {
        Some(p) => ProgramFilter::Programs(p.iter().cloned().collect()),
        None => ProgramFilter::AllNonVote,
    };

    // Recorder + pipeline thread: the single consumer. Records first, then processes, in arrival order.
    let (act_tx, mut act_rx) = mpsc::unbounded_channel::<u64>();
    let out_dir = o.out.clone();
    let grace = o.gap_grace_slots;
    let rotate = o.rotate_secs;
    let consumer = std::thread::Builder::new().name("pipeline".into()).spawn(move || -> Result<serde_json::Value> {
        let mut rec = recorder::Recorder::create(&out_dir, latency::mono_ns(), rotate, 500)?;
        let derived = std::fs::File::create(out_dir.join("derived.live.txt"))?;
        let mut w = replay::HashingWriter::new(std::io::BufWriter::with_capacity(1 << 20, derived));
        let mut pipe = Pipeline::new(filter, grace);
        let mut actions = Vec::new();
        let mut last_status = latency::mono_ns();
        let t0 = latency::mono_ns();
        while let Some(m) = rx.blocking_recv() {
            let is_stop = m.source == events::Source::Control && m.payload.starts_with(br#"{"event":"stop""#);
            rec.write(&m)?;
            pipe.process(&m, &mut w, &mut actions)?;
            for a in actions.drain(..) {
                let Action::BackfillBlock(s) = a;
                let _ = act_tx.send(s);
            }
            let now = latency::mono_ns();
            if now - last_status > 30_000_000_000 {
                last_status = now;
                let s = pipe.stats();
                tracing::info!(
                    elapsed_s = (now - t0) / 1_000_000_000,
                    msgs = ?s.msgs, dups = ?s.dups, gaps = ?s.gaps,
                    raw_mb = rec.bytes_raw / 1_000_000, disk_mb = rec.bytes_written / 1_000_000,
                    "status"
                );
            }
            if is_stop {
                break;
            }
        }
        let stats = pipe.finish(&mut w)?;
        let sha = w.finish()?;
        let (raw, disk) = (rec.bytes_raw, rec.bytes_written);
        rec.close(latency::mono_ns())?;
        Ok(serde_json::json!({ "derived_live_sha256": sha, "stats": stats, "raw_bytes": raw, "disk_bytes": disk,
                               "duration_s": (latency::mono_ns() - t0) as f64 / 1e9 }))
    })?;

    tx.send(start).await.map_err(|_| anyhow!("consumer died"))?;
    let (stop_tx, stop_rx) = watch::channel(false);
    let mut tasks = Vec::new();
    let rpc = Rpc::new(settings.rpc_url.clone());
    if o.feeds.iter().any(|f| f == "grpc") {
        tasks.push(tokio::spawn(feed::grpc::run(
            settings.grpc_url.clone(),
            settings.grpc_token.clone(),
            o.programs.clone(),
            tx.clone(),
            stop_rx.clone(),
        )));
    }
    if o.feeds.iter().any(|f| f == "ws") {
        let programs = o.programs.clone().ok_or_else(|| anyhow!("ws needs programs"))?;
        tasks.push(tokio::spawn(feed::ws::run(settings.ws_url.clone(), rpc.clone(), programs, tx.clone(), stop_rx.clone())));
    }
    // Gap backfill worker (finalized getBlock).
    {
        let rpc = rpc.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            while let Some(slot) = act_rx.recv().await {
                let m = rpc.get_block_msg(slot, "finalized").await;
                if tx.send(m).await.is_err() {
                    return;
                }
            }
        });
    }
    // Chaos: forced disconnects.
    if o.chaos_kills > 0 {
        let host = host_of(if o.chaos_feed == "ws" { &settings.ws_url } else { &settings.grpc_url })?;
        let feed_tag = if o.chaos_feed == "ws" { "ws" } else { "grpc" };
        let tx = tx.clone();
        let (n, every) = (o.chaos_kills, o.chaos_interval_secs);
        tokio::spawn(async move {
            // First kill after 2 intervals so the stream is warm; deterministic jitter in [0, 9] s.
            tokio::time::sleep(Duration::from_secs(every)).await;
            for i in 0..n {
                tokio::time::sleep(Duration::from_secs(every + (i * 7) % 10)).await;
                let killed = kill_connections(&host).await.unwrap_or(0);
                tracing::info!(i, killed, "chaos kill");
                let _ = tx.send(feed::control("chaos_kill", serde_json::json!({"i": i, "feed": feed_tag, "sockets": killed}))).await;
            }
        });
    }

    let ctrl_c = tokio::signal::ctrl_c();
    if o.duration_secs > 0 {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(o.duration_secs)) => {}
            _ = ctrl_c => {}
        }
    } else {
        ctrl_c.await?;
    }
    tracing::info!("stopping");
    let _ = stop_tx.send(true);
    for t in tasks {
        let _ = tokio::time::timeout(Duration::from_secs(5), t).await;
    }
    tx.send(feed::control("stop", serde_json::json!({}))).await.map_err(|_| anyhow!("consumer died"))?;
    let summary = tokio::task::spawn_blocking(move || consumer.join()).await?.map_err(|_| anyhow!("pipeline thread panicked"))??;
    std::fs::write(o.out.join("summary.json"), serde_json::to_string_pretty(&summary)?)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

/// G0.1 independent check: for a deterministic sample of finalized slots that have a gRPC BlockMeta,
/// fetch the block with getBlock and compare the program-filtered signature set with the derived output.
async fn verify_chain(env: &Path, rec: &Path, sample: usize, outages: bool) -> Result<()> {
    let mut reader = recorder::Reader::open(rec)?;
    let meta = replay::run_meta_from(&reader.next_msg()?.ok_or_else(|| anyhow!("empty"))?)?;
    let settings = Settings::load(env, meta.cluster.parse()?)?;
    logging::init(Redactor::new(settings.secrets()), None)?;
    let derived = std::fs::read_to_string(rec.join("derived.live.txt")).context("derived.live.txt")?;
    let mut finalized = std::collections::BTreeSet::new();
    let mut with_meta = std::collections::BTreeSet::new();
    let mut ours: std::collections::HashMap<u64, std::collections::BTreeSet<String>> = Default::default();
    // WS-only recordings have no BlockMeta: slots announced by slotNotification and rooted are the candidates.
    let mut ws_slots = std::collections::BTreeSet::new();
    let mut max_root = 0u64;
    let mut windows: Vec<(u64, u64)> = Vec::new();
    let mut last_slot_seen = 0u64;
    let mut open_from: Option<u64> = None;
    for line in derived.lines() {
        let p: Vec<&str> = line.split(' ').collect();
        let line_slot = match p[0] {
            "T" | "M" if p.len() > 2 => p[2].parse::<u64>().ok(),
            "L" | "W" | "S" if p.len() > 1 => p[1].parse::<u64>().ok(),
            _ => None,
        };
        if let Some(ls) = line_slot {
            if let Some(from) = open_from.take() {
                windows.push((from, ls + 10));
            }
            last_slot_seen = last_slot_seen.max(ls);
        }
        if p[0] == "C" && p.len() > 1 && (p[1] == "ws_disconnected" || p[1] == "grpc_stream_error" || p[1] == "chaos_kill") {
            open_from.get_or_insert(last_slot_seen);
        }
        match p[0] {
            "S" if p.len() > 2 && p[2] == "2" => {
                finalized.insert(p[1].parse::<u64>()?);
            }
            "M" if p.len() > 2 => {
                with_meta.insert(p[2].parse::<u64>()?);
            }
            "T" if p.len() > 4 => {
                ours.entry(p[2].parse()?).or_default().insert(p[4].to_string());
            }
            "W" if p.len() > 3 => {
                ws_slots.insert(p[1].parse::<u64>()?);
                max_root = max_root.max(p[3].parse::<u64>()?);
            }
            "L" if p.len() > 2 => {
                ours.entry(p[1].parse()?).or_default().insert(p[2].to_string());
            }
            _ => {}
        }
    }
    let cand: Vec<u64> = if with_meta.is_empty() {
        ws_slots.range(..=max_root).copied().collect()
    } else {
        finalized.intersection(&with_meta).copied().collect()
    };
    if cand.len() < 120 {
        bail!("too few finalized slots with meta ({})", cand.len());
    }
    let edge = (cand.len() / 10).min(200);
    let inner = &cand[edge..cand.len() - edge];
    let step = (inner.len() / sample.max(1)).max(1);
    let picks: Vec<u64> = if outages {
        // Every slot number in each window, whether or not we saw it: getBlock says which ones have blocks.
        let mut v: Vec<u64> = windows.iter().flat_map(|(a, b)| *a..=*b).collect();
        v.sort();
        v.dedup();
        v
    } else {
        inner.iter().step_by(step).take(sample).copied().collect()
    };
    let rpc = Rpc::new(settings.rpc_url.clone());
    let (mut checked, mut missing_total, mut extra_total, mut bad_slots, mut rpc_txs) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut skipped_no_block, mut rpc_errors) = (0u64, 0u64);
    let mut examples = Vec::new();
    for s in &picks {
        let m = rpc.get_block_msg(*s, "finalized").await;
        let norms = events::normalize(&m)?;
        let b = match norms.into_iter().next() {
            Some(events::Norm::RpcBlock { block: Some(b), .. }) => b,
            Some(events::Norm::RpcBlock { block: None, error: None, .. }) => {
                // Leader skipped the slot: nothing to compare, but we must not hold data for it either.
                if ours.get(s).is_some_and(|x| !x.is_empty()) {
                    bad_slots += 1;
                    examples.push(format!("slot {s}: skipped per RPC but we have txs"));
                }
                skipped_no_block += 1;
                continue;
            }
            _ => {
                rpc_errors += 1;
                examples.push(format!("slot {s}: getBlock error"));
                continue;
            }
        };
        let theirs: std::collections::BTreeSet<String> =
            b.txs.iter().filter(|t| meta.filter.matches(t)).map(|t| bs58_sig(&t.sig)).collect();
        rpc_txs += theirs.len() as u64;
        let empty = Default::default();
        let mine = ours.get(s).unwrap_or(&empty);
        let missing = theirs.difference(mine).count() as u64;
        let extra = mine.difference(&theirs).count() as u64;
        if missing + extra > 0 {
            bad_slots += 1;
            if examples.len() < 10 {
                examples.push(format!("slot {s}: missing {missing}, extra {extra}"));
            }
        }
        missing_total += missing;
        extra_total += extra;
        checked += 1;
    }
    let v = serde_json::json!({
        "finalized_slots_with_meta": cand.len(), "mode": if outages { "outage_windows" } else { "sample" },
        "outage_windows": windows.len(), "sampled": picks.len(), "checked": checked,
        "rpc_filtered_txs": rpc_txs, "skipped_no_block": skipped_no_block, "rpc_errors": rpc_errors, "missing_in_ours": missing_total, "extra_in_ours": extra_total,
        "slots_with_mismatch": bad_slots, "examples": examples,
    });
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}

fn bs58_sig(sig: &[u8; 64]) -> String {
    bs58::encode(sig).into_string()
}

async fn verify_programs(env: &Path, venues: &Path) -> Result<()> {
    let settings = Settings::load(env, Cluster::Mainnet)?;
    logging::init(Redactor::new(settings.secrets()), None)?;
    let reg = VenueRegistry::load(venues)?;
    let rpc = Rpc::new(settings.rpc_url.clone());
    let ids = reg.program_ids();
    let v = rpc
        .call("getMultipleAccounts", serde_json::json!([ids, {"encoding": "base64", "dataSlice": {"offset": 0, "length": 0}}]))
        .await?;
    let arr = v["result"]["value"].as_array().ok_or_else(|| anyhow!("bad response"))?;
    let mut ok = true;
    for (venue, acc) in reg.venue.iter().zip(arr) {
        let exec = acc["executable"].as_bool().unwrap_or(false);
        let owner = acc["owner"].as_str().unwrap_or("-");
        ok &= exec;
        println!("{:<20} {:<46} executable={exec} owner={owner}", venue.name, venue.program_id);
    }
    let _ = std::io::stdout().flush();
    if !ok {
        bail!("some program IDs are not executable accounts");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn backfill(
    settings: Settings,
    cluster: String,
    programs: Vec<String>,
    out: PathBuf,
    end_slot: Option<u64>,
    slots: u64,
    concurrency: usize,
    spot_checks: usize,
) -> Result<()> {
    use futures::StreamExt;
    use std::sync::atomic::Ordering::Relaxed;
    let rpc = Rpc::new(settings.rpc_url.clone());
    let c = std::sync::Arc::new(feed::backfill::FetchCounters::default());
    let end = match end_slot {
        Some(e) => e,
        None => {
            let v = rpc.call("getSlot", serde_json::json!([{"commitment": "finalized"}])).await?;
            v["result"].as_u64().ok_or_else(|| anyhow!("getSlot failed"))? - 200
        }
    };
    let start = end + 1 - slots;
    let t0 = latency::mono_ns();
    let blocks = feed::backfill::confirmed_blocks(&rpc, start, end, &c).await?;
    let listed: std::collections::HashSet<u64> = blocks.iter().copied().collect();
    let skipped: Vec<u64> = (start..=end).filter(|s| !listed.contains(s)).collect();
    tracing::info!(start, end, blocks = blocks.len(), skipped = skipped.len(), "backfill range");

    let mut rec = recorder::Recorder::create(&out, latency::mono_ns(), 3600, 60_000)?.with_compression(9, 8 << 20);
    rec.write(&feed::control("start", serde_json::json!({
        "mode": "backfill", "cluster": cluster, "programs": programs, "gap_grace_slots": 150,
        "commit": git_commit(), "start_slot": start, "end_slot": end,
    })))?;
    rec.write(&feed::control("getblocks", serde_json::json!({ "start": start, "end": end, "slots": blocks })))?;
    // Spot-check: a deterministic sample of slots absent from getBlocks must be reported skipped by getBlock too.
    let step = (skipped.len() / spot_checks.max(1)).max(1);
    let checks: Vec<u64> = skipped.iter().step_by(step).take(spot_checks).copied().collect();
    let pset: std::sync::Arc<std::collections::HashSet<String>> = std::sync::Arc::new(programs.iter().cloned().collect());
    let mut work = blocks.clone();
    work.extend(&checks);
    let mut stream = Box::pin(feed::backfill::fetch_ordered(rpc.clone(), work, pset, concurrency, c.clone()));
    let mut done = 0u64;
    let mut last = latency::mono_ns();
    while let Some(m) = stream.next().await {
        rec.write(&m)?;
        done += 1;
        let now = latency::mono_ns();
        if now - last > 30_000_000_000 {
            last = now;
            let el = (now - t0) as f64 / 1e9;
            tracing::info!(done, of = blocks.len() + checks.len(), slots_per_s = format!("{:.1}", done as f64 / el),
                retries = c.get_block_retries.load(Relaxed), decoded_mb = c.bytes_decoded.load(Relaxed) / 1_000_000,
                disk_mb = rec.bytes_written / 1_000_000, "backfill progress");
        }
    }
    rec.write(&feed::control("stop", serde_json::json!({})))?;
    let disk = rec.bytes_written;
    rec.close(latency::mono_ns())?;
    let el = (latency::mono_ns() - t0) as f64 / 1e9;
    let summary = serde_json::json!({
        "cluster": settings_cluster_tag(&settings), "start_slot": start, "end_slot": end, "slots_in_range": slots,
        "confirmed_blocks": blocks.len(), "skipped_per_getblocks": skipped.len(), "skipped_spot_checked": checks.len(),
        "calls": { "getBlocks": c.get_blocks_calls.load(Relaxed), "getBlock": c.get_block_calls.load(Relaxed),
                   "getBlock_retries": c.get_block_retries.load(Relaxed), "getSlot": if end_slot.is_none() { 1 } else { 0 } },
        "bytes_decoded": c.bytes_decoded.load(Relaxed), "txs_total": c.txs_total.load(Relaxed), "txs_kept_venues": c.txs_kept.load(Relaxed),
        "disk_bytes": disk, "elapsed_s": el,
    });
    std::fs::write(out.join("summary.json"), serde_json::to_string_pretty(&summary)?)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

fn settings_cluster_tag(s: &Settings) -> &'static str {
    match s.cluster {
        Cluster::Mainnet => "mainnet",
        Cluster::Devnet => "devnet",
    }
}

/// G0.H1: reads only the recording. Every slot in the range must be either a fetched block or listed as
/// skipped by the cluster; the spot-checked skipped slots must come back from getBlock as skipped.
fn verify_backfill(rec: &Path) -> Result<()> {
    let mut range = None;
    let mut listed: std::collections::BTreeSet<u64> = Default::default();
    let mut fetched_ok = std::collections::BTreeSet::new();
    let mut fetched_none = std::collections::BTreeSet::new();
    let mut errors = Vec::new();
    for m in recorder::Reader::open(rec)? {
        let m = m?;
        for n in events::normalize(&m)? {
            match n {
                events::Norm::Control(v) if v["event"] == "start" => {
                    range = Some((v["start_slot"].as_u64().unwrap_or(0), v["end_slot"].as_u64().unwrap_or(0)));
                }
                events::Norm::Control(v) if v["event"] == "getblocks" => {
                    listed = v["slots"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).collect()).unwrap_or_default();
                }
                events::Norm::RpcBlock { slot, block: Some(_), .. } => {
                    fetched_ok.insert(slot);
                }
                events::Norm::RpcBlock { slot, block: None, error: None } => {
                    fetched_none.insert(slot);
                }
                events::Norm::RpcBlock { slot, error: Some(e), .. } => errors.push((slot, format!("{slot}: {e}"))),
                _ => {}
            }
        }
    }
    let (start, end) = range.ok_or_else(|| anyhow!("no start marker"))?;
    // a fetch error is repaired when the same slot was fetched later (backfill-repair segment)
    let repaired = errors.iter().filter(|(s, _)| fetched_ok.contains(s)).count();
    let errors: Vec<String> = errors.into_iter().filter(|(s, _)| !fetched_ok.contains(s)).map(|(_, e)| e).collect();
    let missing: Vec<u64> = listed.iter().filter(|s| !fetched_ok.contains(s)).copied().collect();
    let unaccounted: Vec<u64> = (start..=end).filter(|s| !listed.contains(s) && !fetched_ok.contains(s) && !fetched_none.contains(s)).collect();
    let skipped_listed_but_block: Vec<u64> = fetched_ok.iter().filter(|s| !listed.contains(s)).copied().collect();
    let spot_contradictions: Vec<u64> = fetched_none.iter().filter(|s| listed.contains(s)).copied().collect();
    let v = serde_json::json!({
        "range": [start, end], "slots": end - start + 1, "listed_blocks": listed.len(), "fetched_blocks": fetched_ok.len(),
        "confirmed_skipped_by_getblocks": (end - start + 1) as usize - listed.len(),
        "spot_checked_skipped_confirmed": fetched_none.len(),
        "missing_blocks": missing.len(), "missing_examples": missing.iter().take(10).collect::<Vec<_>>(),
        "fetch_errors": errors.len(), "error_examples": errors.iter().take(5).collect::<Vec<_>>(), "fetch_errors_repaired": repaired,
        "spot_check_contradictions": skipped_listed_but_block.len() + spot_contradictions.len(),
        "unaccounted_slots_note": unaccounted.len(),
        "G0_H1": if missing.is_empty() && errors.is_empty() && skipped_listed_but_block.is_empty() && spot_contradictions.is_empty() { "PASS" } else { "FAIL" },
    });
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}

/// Listed (getBlocks) slots of a backfill recording without a fetched block.
fn missing_listed_blocks(rec: &Path) -> Result<Vec<u64>> {
    let mut listed: std::collections::BTreeSet<u64> = Default::default();
    let mut ok = std::collections::BTreeSet::new();
    for m in recorder::Reader::open(rec)? {
        for n in events::normalize(&m?)? {
            match n {
                events::Norm::Control(v) if v["event"] == "getblocks" => {
                    listed = v["slots"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).collect()).unwrap_or_default();
                }
                events::Norm::RpcBlock { slot, block: Some(_), .. } => {
                    ok.insert(slot);
                }
                _ => {}
            }
        }
    }
    Ok(listed.into_iter().filter(|s| !ok.contains(s)).collect())
}

async fn backfill_repair(settings: Settings, programs: Vec<String>, rec: PathBuf, concurrency: usize) -> Result<()> {
    use futures::StreamExt;
    let missing = missing_listed_blocks(&rec)?;
    if missing.is_empty() {
        println!("{}: nothing to repair", rec.display());
        return Ok(());
    }
    let rpc = Rpc::new(settings.rpc_url.clone());
    let c = std::sync::Arc::new(feed::backfill::FetchCounters::default());
    let tmp = rec.join("_repair");
    let _ = std::fs::remove_dir_all(&tmp);
    let mut w = recorder::Recorder::create(&tmp, latency::mono_ns(), 3600, 60_000)?.with_compression(9, 8 << 20);
    let pset: std::sync::Arc<std::collections::HashSet<String>> = std::sync::Arc::new(programs.iter().cloned().collect());
    let mut stream = Box::pin(feed::backfill::fetch_ordered(rpc, missing.clone(), pset, concurrency, c.clone()));
    while let Some(m) = stream.next().await {
        w.write(&m)?;
    }
    w.close(latency::mono_ns())?;
    let next = std::fs::read_dir(&rec)?.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("raw-2")).count();
    let mut moved = 0;
    for (i, e) in std::fs::read_dir(&tmp)?.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("raw-")).enumerate() {
        std::fs::rename(e.path(), rec.join(format!("raw-{:06}.zst", 200_000 + next + i)))?;
        moved += 1;
    }
    std::fs::remove_dir_all(&tmp)?;
    println!("{}: re-fetched {} blocks ({} getBlock calls) into {} segment(s)", rec.display(), missing.len(),
        c.get_block_calls.load(std::sync::atomic::Ordering::Relaxed), moved);
    Ok(())
}

fn read_summary(rec: &Path) -> Result<serde_json::Value> {
    let p = rec.join("summary.json");
    Ok(serde_json::from_str(&std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?)?)
}
