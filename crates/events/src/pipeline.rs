//! Stage 0 canonical pipeline: normalize -> dedup -> gap tracking -> derived output lines.
//! The derived output is the artifact compared byte-for-byte in the replay-determinism gate (G0.3).

use crate::gap::{GapEvent, GapStats, GapTracker};
use crate::normalize::{normalize, Norm, Sig, TxRef};
use crate::raw::{RawMsg, Source};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;

const SLOT_FINALIZED: i32 = 2;
/// Dedup keys older than this many slots behind the newest slot are evicted.
const DEDUP_WINDOW_SLOTS: u64 = 20_000;

/// Which transactions belong to the subscribed universe (applied to RPC backfill; gRPC filters server-side).
#[derive(Debug, Clone)]
pub enum ProgramFilter {
    AllNonVote,
    Programs(FxHashSet<String>),
}

impl ProgramFilter {
    pub fn matches(&self, tx: &TxRef) -> bool {
        if tx.is_vote {
            return false;
        }
        match self {
            ProgramFilter::AllNonVote => true,
            ProgramFilter::Programs(p) => tx.accounts.iter().any(|a| p.contains(a)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    BackfillBlock(u64),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Tx(u64, Sig),
    Meta(u64, String),
    Footer(u64, u64),
    Slot(u64, i32),
    WsSlot(u64),
}

impl Key {
    fn slot(&self) -> u64 {
        match self {
            Key::Tx(s, _) | Key::Meta(s, _) | Key::Footer(s, _) | Key::Slot(s, _) | Key::WsSlot(s) => *s,
        }
    }
}

#[derive(Default)]
struct Dedup {
    seen: FxHashSet<Key>,
    order: VecDeque<Key>,
    max_slot: u64,
}

impl Dedup {
    /// true if new.
    fn insert(&mut self, k: Key) -> bool {
        let s = k.slot();
        if self.seen.contains(&k) {
            return false;
        }
        self.seen.insert(k.clone());
        self.order.push_back(k);
        if s > self.max_slot {
            self.max_slot = s;
            let floor = self.max_slot.saturating_sub(DEDUP_WINDOW_SLOTS);
            while let Some(front) = self.order.front() {
                if front.slot() >= floor || self.order.len() < 1_000 {
                    break;
                }
                let k = self.order.pop_front().expect("front exists");
                self.seen.remove(&k);
            }
        }
        true
    }

    /// Forget transactions of a slot whose bank was cleared (Alpenglow UpdateParent): re-delivery is a new event.
    fn purge_tx_slot(&mut self, slot: u64) -> usize {
        let before = self.seen.len();
        self.seen.retain(|k| !matches!(k, Key::Tx(s, _) if *s == slot));
        before - self.seen.len()
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct PipelineStats {
    pub msgs: BTreeMap<&'static str, u64>,
    pub events: BTreeMap<&'static str, u64>,
    pub dups: BTreeMap<&'static str, u64>,
    pub decode_errors: u64,
    pub filtered_out_backfill_txs: u64,
    pub gaps: GapStats,
    pub gap_unchecked_tail: u64,
    pub controls: BTreeMap<String, u64>,
    pub ws_backfill_pages: u64,
    pub ws_backfill_incomplete: u64,
    pub cleared_bank_purged_txs: u64,
}

pub struct Pipeline {
    filter: ProgramFilter,
    dedup: Dedup,
    gap: GapTracker,
    ws_subs: FxHashMap<u64, String>,
    stats: PipelineStats,
    gap_events: Vec<GapEvent>,
    backfills: Vec<u64>,
}

fn b58(sig: &Sig) -> String {
    bs58::encode(sig).into_string()
}

impl Pipeline {
    pub fn new(filter: ProgramFilter, gap_grace_slots: u64) -> Self {
        Self {
            filter,
            dedup: Dedup::default(),
            gap: GapTracker::new(gap_grace_slots),
            ws_subs: FxHashMap::default(),
            stats: PipelineStats::default(),
            gap_events: Vec::new(),
            backfills: Vec::new(),
        }
    }

    pub fn stats(&self) -> PipelineStats {
        let mut s = self.stats.clone();
        s.gaps = self.gap.stats.clone();
        s.gap_unchecked_tail = self.gap.unchecked_tail() as u64;
        s
    }

    /// Slot currently awaiting backfill (live driver uses this to avoid duplicate requests).
    pub fn waiting_backfill(&self) -> Option<u64> {
        self.gap.waiting()
    }

    fn dup(&mut self, src: Source) {
        *self.stats.dups.entry(src.tag()).or_default() += 1;
    }

    fn ev(&mut self, kind: &'static str) {
        *self.stats.events.entry(kind).or_default() += 1;
    }

    pub fn process<W: Write>(&mut self, msg: &RawMsg, out: &mut W, actions: &mut Vec<Action>) -> std::io::Result<()> {
        let src = msg.source;
        *self.stats.msgs.entry(src.tag()).or_default() += 1;
        let norms = match normalize(msg) {
            Ok(n) => n,
            Err(_) => {
                self.stats.decode_errors += 1;
                writeln!(out, "E {}", src.tag())?;
                return Ok(());
            }
        };
        for n in norms {
            self.apply(src, n, out)?;
        }
        self.gap.poll(&mut self.gap_events, &mut self.backfills);
        self.flush_gap_events(out, actions)
    }

    fn flush_gap_events<W: Write>(&mut self, out: &mut W, actions: &mut Vec<Action>) -> std::io::Result<()> {
        for e in self.gap_events.drain(..) {
            match e {
                GapEvent::Detected { slot } => writeln!(out, "G {slot}")?,
                GapEvent::Recovered { slot } => writeln!(out, "R {slot}")?,
                GapEvent::Unrecovered { slot, reason } => writeln!(out, "X {slot} {reason}")?,
            }
        }
        actions.extend(self.backfills.drain(..).map(Action::BackfillBlock));
        Ok(())
    }

    fn emit_tx<W: Write>(&mut self, src: Source, slot: u64, tx: &TxRef, out: &mut W) -> std::io::Result<()> {
        if self.dedup.insert(Key::Tx(slot, tx.sig)) {
            self.ev("tx");
            writeln!(out, "T {} {slot} {} {} {}", src.tag(), tx.index, b58(&tx.sig), if tx.failed { "fail" } else { "ok" })?;
        } else {
            self.dup(src);
        }
        Ok(())
    }

    fn apply<W: Write>(&mut self, src: Source, n: Norm, out: &mut W) -> std::io::Result<()> {
        match n {
            Norm::Slot { slot, parent, status, dead } => {
                if self.dedup.insert(Key::Slot(slot, status)) {
                    self.ev("slot");
                    writeln!(out, "S {slot} {status} {} {}", parent.map_or("-".into(), |p| p.to_string()), dead as u8)?;
                    if status == SLOT_FINALIZED {
                        self.gap.on_finalized(slot);
                    }
                } else {
                    self.dup(src);
                }
            }
            Norm::BlockMeta { slot, parent_slot, blockhash, block_time, executed_tx } => {
                if self.dedup.insert(Key::Meta(slot, blockhash.clone())) {
                    self.ev("block_meta");
                    writeln!(
                        out,
                        "M {} {slot} {parent_slot} {blockhash} {} {executed_tx}",
                        src.tag(),
                        block_time.map_or("-".into(), |t| t.to_string())
                    )?;
                    self.gap.on_meta(slot, parent_slot);
                } else {
                    self.dup(src);
                }
            }
            Norm::BlockFooter { slot, bank_id, producer_time_ns } => {
                if self.dedup.insert(Key::Footer(slot, bank_id)) {
                    self.ev("block_footer");
                    writeln!(out, "F {slot} {bank_id} {producer_time_ns}")?;
                } else {
                    self.dup(src);
                }
            }
            Norm::Tx { slot, tx, .. } => self.emit_tx(src, slot, &tx, out)?,
            Norm::UpdateParent { slot, cleared_bank_id, parent_slot } => {
                self.ev("update_parent");
                let purged = self.dedup.purge_tx_slot(slot);
                self.stats.cleared_bank_purged_txs += purged as u64;
                writeln!(out, "U {slot} {cleared_bank_id} {parent_slot} {purged}")?;
            }
            Norm::WsSlot { slot, parent, root } => {
                if self.dedup.insert(Key::WsSlot(slot)) {
                    self.ev("ws_slot");
                    writeln!(out, "W {slot} {parent} {root}")?;
                } else {
                    self.dup(src);
                }
            }
            Norm::WsLog { sub_id, slot, sig, failed } => {
                let program = self.ws_subs.get(&sub_id).cloned().unwrap_or_else(|| "?".into());
                if self.dedup.insert(Key::Tx(slot, sig)) {
                    self.ev("ws_log");
                    writeln!(out, "L {slot} {} {} {program}", b58(&sig), if failed { "fail" } else { "ok" })?;
                } else {
                    self.dup(src);
                }
            }
            Norm::RpcBlock { slot, block, error } => {
                self.ev("rpc_block");
                match block {
                    Some(b) => {
                        writeln!(out, "B {slot} {} {} n={}", b.parent_slot, b.blockhash, b.txs.len())?;
                        if self.dedup.insert(Key::Meta(slot, b.blockhash.clone())) {
                            self.ev("block_meta");
                            writeln!(
                                out,
                                "M rpc {slot} {} {} {} {}",
                                b.parent_slot,
                                b.blockhash,
                                b.block_time.map_or("-".into(), |t| t.to_string()),
                                b.txs.len()
                            )?;
                        }
                        for tx in &b.txs {
                            if self.filter.matches(tx) {
                                self.emit_tx(Source::Rpc, slot, tx, out)?;
                            } else {
                                self.stats.filtered_out_backfill_txs += 1;
                            }
                        }
                        self.gap.on_backfill(slot, Some(b.parent_slot), &mut self.gap_events);
                    }
                    None => {
                        writeln!(out, "B {slot} none {}", if error.is_some() { "error" } else { "skipped" })?;
                        self.gap.on_backfill(slot, None, &mut self.gap_events);
                    }
                }
            }
            Norm::RpcSigs { program, sigs, complete } => {
                self.stats.ws_backfill_pages += 1;
                if !complete {
                    self.stats.ws_backfill_incomplete += 1;
                }
                writeln!(out, "P {program} n={} complete={}", sigs.len(), complete as u8)?;
                for (sig, slot, failed) in sigs {
                    if self.dedup.insert(Key::Tx(slot, sig)) {
                        self.ev("rpc_sig");
                        writeln!(out, "L {slot} {} {} {program} rpc", b58(&sig), if failed { "fail" } else { "ok" })?;
                    } else {
                        self.dup(Source::Rpc);
                    }
                }
            }
            Norm::Control(v) => {
                let event = v["event"].as_str().unwrap_or("?").to_string();
                if event == "ws_sub" {
                    if let (Some(id), Some(p)) = (v["sub_id"].as_u64(), v["program"].as_str()) {
                        self.ws_subs.insert(id, p.to_string());
                    }
                }
                *self.stats.controls.entry(event.clone()).or_default() += 1;
                writeln!(out, "C {event}")?;
            }
            Norm::Ignored => {}
        }
        Ok(())
    }

    /// Final deterministic summary line.
    pub fn finish<W: Write>(&mut self, out: &mut W) -> std::io::Result<PipelineStats> {
        let s = self.stats();
        writeln!(out, "Z {}", serde_json::to_string(&s).expect("stats serialize"))?;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helius_laserstream::grpc::{
        subscribe_update::UpdateOneof, SubscribeUpdate, SubscribeUpdateBlockMeta, SubscribeUpdateSlot,
        SubscribeUpdateTransaction, SubscribeUpdateTransactionInfo,
    };
    use prost::Message;

    fn grpc(u: UpdateOneof, t: u64) -> RawMsg {
        let up = SubscribeUpdate { filters: vec!["x".into()], update_oneof: Some(u), created_at: None };
        RawMsg { source: Source::Grpc, recv_wall_ns: t, recv_mono_ns: t, payload: up.encode_to_vec() }
    }

    fn tx(slot: u64, b: u8) -> UpdateOneof {
        UpdateOneof::Transaction(SubscribeUpdateTransaction {
            slot,
            bank_id: 0,
            transaction: Some(SubscribeUpdateTransactionInfo { signature: vec![b; 64], index: b as u64, ..Default::default() }),
        })
    }

    fn meta(slot: u64, parent: u64) -> UpdateOneof {
        UpdateOneof::BlockMeta(SubscribeUpdateBlockMeta { slot, parent_slot: parent, blockhash: format!("h{slot}"), ..Default::default() })
    }

    fn fin(slot: u64) -> UpdateOneof {
        UpdateOneof::Slot(SubscribeUpdateSlot { slot, status: SLOT_FINALIZED, ..Default::default() })
    }

    /// Reconnect replay re-sends the last slots: every duplicate must be dropped, output must equal the clean run.
    #[test]
    fn replayed_messages_are_deduplicated() {
        let clean: Vec<RawMsg> = vec![grpc(tx(10, 1), 1), grpc(meta(10, 9), 2), grpc(tx(11, 2), 3), grpc(meta(11, 10), 4)];
        let mut dirty = clean.clone();
        dirty.extend(clean.iter().cloned()); // replay after reconnect
        let run = |msgs: &[RawMsg]| {
            let mut p = Pipeline::new(ProgramFilter::AllNonVote, 0);
            let mut out = Vec::new();
            let mut a = Vec::new();
            for m in msgs {
                p.process(m, &mut out, &mut a).unwrap();
            }
            (String::from_utf8(out).unwrap(), p.stats())
        };
        let (a, _) = run(&clean);
        let (b, st) = run(&dirty);
        assert_eq!(a, b);
        assert_eq!(st.dups.get("grpc"), Some(&4));
    }

    #[test]
    fn gap_requests_backfill_and_rpc_block_recovers() {
        let mut p = Pipeline::new(ProgramFilter::AllNonVote, 0);
        let mut out = Vec::new();
        let mut a = Vec::new();
        for m in [grpc(meta(10, 9), 1), grpc(meta(12, 11), 2), grpc(fin(10), 3), grpc(fin(12), 4)] {
            p.process(&m, &mut out, &mut a).unwrap();
        }
        assert_eq!(a, vec![Action::BackfillBlock(11)]);
        let sig = bs58::encode([7u8; 64]).into_string();
        let block = serde_json::json!({"method":"getBlock","params":{"slot":11},"error":null,"result":{
            "parentSlot":10,"blockhash":"h11","blockTime":null,"transactions":[{
                "transaction":{"signatures":[sig],"message":{"accountKeys":["A"],"instructions":[{"programIdIndex":0}]}},
                "meta":{"err":null}}]}});
        let m = RawMsg { source: Source::Rpc, recv_wall_ns: 5, recv_mono_ns: 5, payload: block.to_string().into_bytes() };
        p.process(&m, &mut out, &mut a).unwrap();
        let s = p.stats();
        assert_eq!((s.gaps.detected, s.gaps.recovered, s.gaps.unrecovered), (1, 1, 0));
        assert!(String::from_utf8(out).unwrap().contains(&format!("T rpc 11 0 {sig} ok")));
    }
}
