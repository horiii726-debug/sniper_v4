//! Deterministic replay of recordings through the same pipeline code as live (V4 §26),
//! plus offline reports for Gate 0 (latency, reconnect recovery, duplicate audit).

use anyhow::{anyhow, Result};
use events::{normalize, Norm, Pipeline, PipelineStats, ProgramFilter, RawMsg, Source};
use latency::LatencyHist;
use recorder::Reader;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{BufRead, Write};
use std::path::Path;

/// Run metadata, written by the live recorder as the first Control message.
#[derive(Debug, Clone)]
pub struct RunMeta {
    pub filter: ProgramFilter,
    pub gap_grace_slots: u64,
    pub cluster: String,
}

pub fn run_meta_from(msg: &RawMsg) -> Result<RunMeta> {
    if msg.source != Source::Control {
        return Err(anyhow!("first message is not the start marker"));
    }
    let v: serde_json::Value = serde_json::from_slice(&msg.payload)?;
    if v["event"] != "start" {
        return Err(anyhow!("first control is not start"));
    }
    let filter = match v["programs"].as_array() {
        Some(a) => ProgramFilter::Programs(a.iter().filter_map(|p| p.as_str().map(String::from)).collect()),
        None => ProgramFilter::AllNonVote,
    };
    Ok(RunMeta {
        filter,
        gap_grace_slots: v["gap_grace_slots"].as_u64().ok_or_else(|| anyhow!("no gap_grace_slots"))?,
        cluster: v["cluster"].as_str().unwrap_or("").to_string(),
    })
}

/// Writer that hashes everything it forwards.
pub struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
    pub bytes: u64,
}

impl<W: Write> HashingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner, hasher: Sha256::new(), bytes: 0 }
    }
    pub fn finish(mut self) -> std::io::Result<String> {
        self.inner.flush()?;
        Ok(hex(&self.hasher.finalize()))
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[derive(Debug, Serialize)]
pub struct ReplayResult {
    pub sha256: String,
    pub derived_bytes: u64,
    pub messages: u64,
    pub truncated_tails: u64,
    pub stats: PipelineStats,
}

/// Replay a recording into `out` (derived output). Backfill actions are ignored: their results are in the recording.
pub fn replay(rec: &Path, out: &mut dyn Write) -> Result<ReplayResult> {
    let mut reader = Reader::open(rec)?;
    let first = reader.next_msg()?.ok_or_else(|| anyhow!("empty recording"))?;
    let meta = run_meta_from(&first)?;
    let mut pipe = Pipeline::new(meta.filter, meta.gap_grace_slots);
    let mut w = HashingWriter::new(std::io::BufWriter::with_capacity(1 << 20, out));
    let mut actions = Vec::new();
    let mut n = 1u64;
    pipe.process(&first, &mut w, &mut actions)?;
    while let Some(m) = reader.next_msg()? {
        n += 1;
        pipe.process(&m, &mut w, &mut actions)?;
        actions.clear();
    }
    let stats = pipe.finish(&mut w)?;
    let derived_bytes = w.bytes;
    let sha256 = w.finish()?;
    Ok(ReplayResult { sha256, derived_bytes, messages: n, truncated_tails: reader.truncated_tails, stats })
}

#[derive(Debug, Serialize, Default)]
pub struct LatencyReport {
    /// Per feed: receive wall time minus the block producer timestamp (BlockFooter) of the event's slot.
    pub vs_block_producer_time: std::collections::BTreeMap<String, latency::Summary>,
    /// gRPC only: receive wall time minus the server's `created_at` on the update.
    pub grpc_vs_server_created_at: Option<latency::Summary>,
    /// Per feed: receive wall time minus `blockTime` (1 s resolution, coarse).
    pub vs_block_time_coarse: std::collections::BTreeMap<String, latency::Summary>,
    /// Same signature seen by both feeds: ws_recv - grpc_recv (positive = gRPC first).
    pub ws_minus_grpc_same_sig: Option<latency::Summary>,
    pub grpc_first_count: u64,
    pub ws_first_count: u64,
    pub slots_with_producer_time: u64,
    /// Diagnostics for the meaning of `block_producer_time_nanos`.
    pub diag_first_tx_recv_minus_producer: Option<latency::Summary>,
    pub diag_last_tx_recv_minus_producer: Option<latency::Summary>,
    pub diag_footer_recv_minus_producer: Option<latency::Summary>,
    pub diag_producer_minus_parent_producer: Option<latency::Summary>,
    pub diag_first_shred_recv_minus_parent_producer: Option<latency::Summary>,
    /// Primary G0.5 metric: receive time minus slot start, where slot start = parent block's producer time.
    pub vs_slot_start: std::collections::BTreeMap<String, latency::Summary>,
}

/// G0.5: two passes. Pass 1 collects per-slot producer time and block time; pass 2 measures events.
pub fn latency_report(rec: &Path) -> Result<LatencyReport> {
    let mut producer: FxHashMap<u64, u64> = FxHashMap::default();
    let mut block_time: FxHashMap<u64, i64> = FxHashMap::default();
    let mut parent: FxHashMap<u64, u64> = FxHashMap::default();
    let mut footer_recv: FxHashMap<u64, u64> = FxHashMap::default();
    let mut first_shred_recv: FxHashMap<u64, u64> = FxHashMap::default();
    for m in Reader::open(rec)? {
        let m = m?;
        if m.source != Source::Grpc && m.source != Source::Rpc {
            continue;
        }
        for n in normalize(&m).unwrap_or_default() {
            match n {
                Norm::BlockFooter { slot, producer_time_ns, .. } if producer_time_ns > 0 => {
                    producer.entry(slot).or_insert(producer_time_ns);
                    footer_recv.entry(slot).or_insert(m.recv_wall_ns);
                }
                Norm::BlockMeta { slot, block_time: bt, parent_slot, .. } => {
                    if let Some(t) = bt {
                        block_time.entry(slot).or_insert(t);
                    }
                    parent.entry(slot).or_insert(parent_slot);
                }
                Norm::Slot { slot, status: 3, .. } => {
                    first_shred_recv.entry(slot).or_insert(m.recv_wall_ns);
                }
                _ => {}
            }
        }
    }
    let mut vs_prod: std::collections::BTreeMap<String, LatencyHist> = Default::default();
    let mut vs_bt: std::collections::BTreeMap<String, LatencyHist> = Default::default();
    let mut created = LatencyHist::new();
    let mut created_n = 0u64;
    let mut first_seen: FxHashMap<[u8; 64], (Source, u64)> = FxHashMap::default();
    let mut cross = LatencyHist::new();
    let mut cross_n = 0u64;
    let (mut grpc_first, mut ws_first) = (0u64, 0u64);
    let mut both: FxHashSet<[u8; 64]> = FxHashSet::default();
    let mut slot_tx_range: FxHashMap<u64, (u64, u64)> = FxHashMap::default();
    let mut vs_start: std::collections::BTreeMap<String, LatencyHist> = Default::default();
    let slot_start = |s: u64| parent.get(&s).and_then(|p| producer.get(p)).copied();
    for m in Reader::open(rec)? {
        let m = m?;
        let (slot, sig, created_at) = match (m.source, normalize(&m).ok().and_then(|v| v.into_iter().next())) {
            (Source::Grpc, Some(Norm::Tx { slot, tx, created_at_ns })) => (slot, tx.sig, created_at_ns),
            (Source::Ws, Some(Norm::WsLog { slot, sig, .. })) => (slot, sig, None),
            _ => continue,
        };
        let feed = m.source.tag().to_string();
        if m.source == Source::Grpc {
            let e = slot_tx_range.entry(slot).or_insert((u64::MAX, 0));
            e.0 = e.0.min(m.recv_wall_ns);
            e.1 = e.1.max(m.recv_wall_ns);
        }
        if let Some(st) = slot_start(slot) {
            vs_start.entry(feed.clone()).or_default().record_diff(m.recv_wall_ns, st);
        }
        if let Some(p) = producer.get(&slot) {
            vs_prod.entry(feed.clone()).or_default().record_diff(m.recv_wall_ns, *p);
        }
        if let Some(t) = block_time.get(&slot) {
            vs_bt.entry(feed).or_default().record_diff(m.recv_wall_ns, (*t as u64) * 1_000_000_000);
        }
        if let Some(c) = created_at {
            created.record_diff(m.recv_wall_ns, c);
            created_n += 1;
        }
        match first_seen.get(&sig) {
            None => {
                first_seen.insert(sig, (m.source, m.recv_wall_ns));
            }
            Some(&(src, t)) if src != m.source && both.insert(sig) => {
                let (g, w) = if src == Source::Grpc { (t, m.recv_wall_ns) } else { (m.recv_wall_ns, t) };
                if g <= w {
                    grpc_first += 1;
                    cross.record_diff(w, g);
                } else {
                    ws_first += 1;
                }
                cross_n += 1;
            }
            _ => {}
        }
    }
    let (mut d_first, mut d_last, mut d_footer, mut d_prod, mut d_shred) =
        (LatencyHist::new(), LatencyHist::new(), LatencyHist::new(), LatencyHist::new(), LatencyHist::new());
    for (slot, p) in &producer {
        if let Some((a, b)) = slot_tx_range.get(slot) {
            d_first.record_diff(*a, *p);
            d_last.record_diff(*b, *p);
        }
        if let Some(r) = footer_recv.get(slot) {
            d_footer.record_diff(*r, *p);
        }
        if let Some(pp) = parent.get(slot).and_then(|ps| producer.get(ps)) {
            d_prod.record_diff(*p, *pp);
            if let Some(fs) = first_shred_recv.get(slot) {
                d_shred.record_diff(*fs, *pp);
            }
        }
    }
    let opt = |h: LatencyHist| { let s = h.summary(); (s.count + s.negative > 0).then_some(s) };
    Ok(LatencyReport {
        diag_first_tx_recv_minus_producer: opt(d_first),
        diag_last_tx_recv_minus_producer: opt(d_last),
        diag_footer_recv_minus_producer: opt(d_footer),
        diag_producer_minus_parent_producer: opt(d_prod),
        diag_first_shred_recv_minus_parent_producer: opt(d_shred),
        vs_slot_start: vs_start.into_iter().map(|(k, h)| (k, h.summary())).collect(),
        vs_block_producer_time: vs_prod.into_iter().map(|(k, h)| (k, h.summary())).collect(),
        grpc_vs_server_created_at: (created_n > 0).then(|| created.summary()),
        vs_block_time_coarse: vs_bt.into_iter().map(|(k, h)| (k, h.summary())).collect(),
        ws_minus_grpc_same_sig: (cross_n > 0).then(|| cross.summary()),
        grpc_first_count: grpc_first,
        ws_first_count: ws_first,
        slots_with_producer_time: producer.len() as u64,
    })
}

#[derive(Debug, Serialize)]
pub struct KillRecovery {
    pub kill_index: u64,
    pub kill_wall_ns: u64,
    /// Time from the forced kill to the next message from the killed feed.
    pub recover_ms: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct RecoveryReport {
    pub kills: Vec<KillRecovery>,
    pub recovered: u64,
    pub not_recovered: u64,
    pub max_recover_ms: f64,
}

/// G0.2: for each `chaos_kill` marker, find the first message of the killed feed that arrives after it
/// *and* after the stream errored (the SDK reports the error through a `grpc_stream_error` control).
pub fn recovery_report(rec: &Path, recover_within_ms: f64) -> Result<RecoveryReport> {
    let mut kills: Vec<KillRecovery> = Vec::new();
    let mut pending: Option<(usize, String, u64, bool)> = None; // (kill idx, feed, kill mono, saw_error)
    for m in Reader::open(rec)? {
        let m = m?;
        if m.source == Source::Control {
            let v: serde_json::Value = serde_json::from_slice(&m.payload)?;
            match v["event"].as_str() {
                Some("chaos_kill") => {
                    kills.push(KillRecovery { kill_index: kills.len() as u64, kill_wall_ns: m.recv_wall_ns, recover_ms: None });
                    pending = Some((kills.len() - 1, v["feed"].as_str().unwrap_or("grpc").to_string(), m.recv_mono_ns, false));
                }
                Some("grpc_stream_error") | Some("ws_disconnected") => {
                    if let Some(p) = pending.as_mut() {
                        p.3 = true;
                    }
                }
                _ => {}
            }
            continue;
        }
        if let Some((idx, feed, t0, saw_err)) = pending.clone() {
            if saw_err && m.source.tag() == feed {
                kills[idx].recover_ms = Some((m.recv_mono_ns - t0) as f64 / 1e6);
                pending = None;
            }
        }
    }
    let recovered = kills.iter().filter(|k| k.recover_ms.is_some_and(|r| r <= recover_within_ms)).count() as u64;
    let max = kills.iter().filter_map(|k| k.recover_ms).fold(0.0, f64::max);
    Ok(RecoveryReport { not_recovered: kills.len() as u64 - recovered, recovered, kills, max_recover_ms: max })
}

#[derive(Debug, Serialize)]
pub struct DupAudit {
    pub lines: u64,
    pub event_lines: u64,
    pub duplicate_events: u64,
    pub examples: Vec<String>,
}

/// Re-check the derived output: no canonical event key may appear twice.
pub fn audit_duplicates(derived: &Path) -> Result<DupAudit> {
    let f = std::io::BufReader::new(std::fs::File::open(derived)?);
    let mut seen: FxHashSet<String> = FxHashSet::default();
    let (mut lines, mut ev, mut dup) = (0u64, 0u64, 0u64);
    let mut examples = Vec::new();
    for line in f.lines() {
        let line = line?;
        lines += 1;
        let p: Vec<&str> = line.split(' ').collect();
        let key = match p.first().copied() {
            Some("T") if p.len() >= 5 => format!("tx {} {}", p[2], p[4]),
            Some("L") if p.len() >= 3 => format!("tx {} {}", p[1], p[2]),
            Some("M") if p.len() >= 5 => format!("m {} {}", p[2], p[4]),
            Some("F") if p.len() >= 3 => format!("f {} {}", p[1], p[2]),
            Some("S") if p.len() >= 3 => format!("s {} {}", p[1], p[2]),
            Some("W") if p.len() >= 2 => format!("w {}", p[1]),
            _ => continue,
        };
        ev += 1;
        if !seen.insert(key) {
            dup += 1;
            if examples.len() < 5 {
                examples.push(line.clone());
            }
        }
    }
    Ok(DupAudit { lines, event_lines: ev, duplicate_events: dup, examples })
}
