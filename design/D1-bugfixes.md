# D1-bugfixes — design (V6, partial; NOT implemented, NOT compiled)

> Produced 2026-10-11 by a read-only design agent. Nothing here has been coded yet.

## Design

# D1 — Phase 1 bug fixes: implementation design (read-only review of branch `v6-components`, nothing compiled or run)

All line numbers are for the current branch (`v6-components` = `v5-upgrade` b70c27c plus the three spec files). Every location the forensic file cites for a V5 bug matches the current code exactly. The V4 IDs cite lines on `main`; their current positions are given below.

**D1 adds no feature columns and no label columns.** FEATURES stays at 107 entries and LABEL_COLS keeps its 9 entries. The only change to an existing feature's *values* is V5-14 (`price_vs_vwap60`, index 36, an in-place bug fix). That is safe because no V5 model has been trained yet.

## 0. Implementation order and Phase-1 protocol (for the VPS)

Use one commit per bug ID, so the user can measure before/after for each one:

1. V5-16 (sol_usd module; every command depends on it)
2. V5-13/V4-4
3. V5-14/V4-5
4. V5-8
5. V5-7 + V5-1 (one commit: `allocate` needs the hold table)
6. V4-12
7. V5-9 diag
8. V5-17
9. V5-10
10. V5-12

Commands on the VPS:
```
mkdir -p data/bf_1h data/h1d
zstd -d datasets/bf_1h/pump_events.bin.zst -o data/bf_1h/pump_events.bin   # V4 cache, loaded by backtest::load_window (WindowCacheV1)
zstd -d datasets/h1d/pump_events.bin.zst  -o data/h1d/pump_events.bin
# BEFORE (binary built from the branch head before D1):
./target/release/sniper v5-compare --rec data/bf_1h --rec data/h1d --out reports/V5_COMPARE_before_D1.json
# AFTER each commit / at the end:
cargo test --release -p app
python3 -m pytest -q python/research/tests python/ml/tests/test_legacy_v4_guard.py python/ml/tests/test_v5_train.py -k "not end_to_end"
python3 -m pytest -q python/ml/tests/test_v5_train.py -k end_to_end      # needs the 2 GPUs
./target/release/sniper v5-compare --rec data/bf_1h --rec data/h1d --out reports/V5_COMPARE_after_D1.json   # add --sol-usd if Binance is unreachable
./target/release/sniper label-diag --rec data/bf_1h --rec data/h1d --out reports/V5_9_BARRIER_v4windows.json
```
**What v5-compare can and cannot show.** v5-compare runs without a model. The NoScores scorer gives every signal the same EV and there is no hold table, so the effects of V5-1 and V5-7 *cannot be measured* by v5-compare. They need fold score files (`models/v5/fold_<W>/scores_<W>.csv.zst` with `p_hold`), which means V5 windows plus `v5_train.py`. Without a model, V5-1 still changes three things:
- a ≤ `decision_tick_s` entry delay;
- the cash reservation fix;
- the untradable re-check.

The before/after numbers on bf_1h/h1d mainly reflect V5-13 (V5 and V4 entries) and V5-8 (DevSell exits).

Reference SOL/USD values from `reports/V5_COMPARE_v4windows.json`: bf_1h **109.65**, h1d **110.53**. Use them with `--sol-usd` for reproducibility if Binance is blocked.

---

## 1. V5-1 — exposure cap checked before slot ranking (CONFIRMED)

**Where:** `crates/app/src/strategy_v5.rs`
- 537-543: per-minute cap.
- 544-549: `exposure_cap` is checked first.
- 550-564: replacement is reachable only when `pos.len()+n_busy_buys >= max_positions`.
- 565: cash check.
- 569-574: the entry itself.

Each signal row is decided immediately, so the logic is first-come.

**Data proof:** `reports/V5_COMPARE_v4windows.json` v5_no_model `blocked` shows `exposure_cap` 1381 (bf_1h) and 2013 (h1d), and `slots_full` 0. The arithmetic: sol_usd 109.65 → start 0.912 SOL → cap 0.6×0.912 = 0.547 SOL → ~10 bets of 0.05, which is fewer than `max_positions` 18.

**Extra bug found, folded into this fix:** line 565 `cash < st.bet_sol + 3.0*txc` ignores cash already reserved by pending buys. Over-committed buys then fail at fill (line 359 `!oracle && cash < size`) and still pay `txc`.

**Fix (decision tick + rank + capacity + replacement):**
- New field `StratV5.decision_tick_s: i64`, `#[serde(default = "default_decision_tick_s")]`, default **2**. A value of 0 means batch per slot (no extra delay). Old strategy JSONs still load.
- New `ReportV5` fields: `replacements: usize`, `merged_in_tick: usize`.
- In `simulate`, the entry section (lines 537-574) no longer enters. After the per-row gates (order_pending, blacklisted, untradable, no_score; plus ev/rug/ret_60s/top10/bundle when not oracle) it pushes a `Cand` into `cands`.
- New loop state: `cands: Vec<Cand>`, `cand_tick: i64`, `last_slot: u64`, `last_t: i64`. Set `last_slot`/`last_t` right after `let e = &evs[i]; i += 1;`.
  - `tick_of(slot,t) = if st.decision_tick_s > 0 { t.div_euclid(st.decision_tick_s) } else { slot as i64 }`.
  - When pushing a cand: `cand_tick = tick_of(e.slot, e.time)`.
- **Flush at the top of the loop, before "1) Fill due orders".** Condition: `!cands.is_empty() && (i >= evs.len() || tick_of(cur_slot,cur_t) != cand_tick)`.
  - If `i >= evs.len()`, block every cand as `"data_end"`.
  - Otherwise:
    1. Re-check `eng.untradable(coin, MIN_EXIT_LIQ_SOL)` per cand; block with that reason.
    2. Build `Capacity` and `Vec<Held>`.
    3. Call `allocate`.
    4. Apply the decisions. Orders go in at `last_slot + c.latency_slots`.
  - Causality holds: only events of the closed tick have been applied to `eng`. Fill semantics are unchanged: a fill happens when the first event of slot > due arrives.

Pure, unit-testable allocator (new, `pub(crate)` in strategy_v5.rs):
```rust
#[derive(Debug, Clone, Copy)] pub(crate) struct Cand { pub coin: u32, pub ev: f64, pub order: u64, pub label: Option<Labels> }
#[derive(Debug, Clone, Copy)] pub(crate) struct Held { pub coin: u32, pub hold_sol: f64, pub cost_share: f64, pub value: f64 }
#[derive(Debug, Clone, Copy)] pub(crate) struct Capacity { pub open_cost: f64, pub equity: f64, pub cash_free: f64, pub slots_used: usize, pub entries_60s: usize }
#[derive(Debug, Clone, Copy, PartialEq)] pub(crate) enum Decision { Enter(usize), Replace(usize, u32), Block(usize, &'static str), Merged(usize) }

pub(crate) fn allocate(cands: &[Cand], held: &[Held], mut cap: Capacity, st: &StratV5, txc: f64, can_replace: bool, oracle: bool) -> Vec<Decision> {
    // 1. one row per coin: keep the LATEST row (max order) of the tick; the others -> Merged
    // 2. sort the kept rows by ev desc, tie order asc (total_cmp; deterministic)
    // 3. oracle: Enter all, return
    // 4. victims = held sorted by hold_sol asc, tie coin asc; vi = 0
    // for each cand in rank order:
    //   if cap.entries_60s >= st.max_entries_per_min -> Block "max_entries_per_min"
    //   exp_full = cap.open_cost + b > st.max_total_exposure * cap.equity; slots_full = cap.slots_used >= st.max_positions;
    //   cash_short = cap.cash_free < b + 3*txc            (b = st.bet_sol)
    //   if none -> Enter; open_cost += b; slots_used += 1; cash_free -= b + txc; entries_60s += 1
    //   else if can_replace && vi < victims.len():
    //       w = victims[vi]
    //       better = b*c.ev - txc > w.hold_sol + st.replace_margin*b              // V5-7: expected net SOL, same unit
    //       exp_ok = cap.open_cost - w.cost_share + b <= st.max_total_exposure*cap.equity
    //       cash_ok = cap.cash_free + w.value - txc >= b + 3*txc
    //       if better && exp_ok && cash_ok -> Replace(i, w.coin); vi += 1; open_cost += b - w.cost_share;
    //            cash_free += w.value - 2*txc - b; entries_60s += 1
    //   otherwise Block(first binding of: "exposure_cap" | "slots_full" | "no_cash")   (same order/names as before)
}
```
Capacity at flush:
- `entries`: pop entries with `<= last_t-60`.
- `open_cost = Σ p.cost·tokens/tokens0 + pending_buy_cost`.
- `equity = cash + Σ pool_value`.
- `n_pending_buys = busy.iter().filter(|b| !pos.contains_key(b)).count()`.
- `cash_free = cash - pending_buy_cost - n_pending_buys*txc`.
- `slots_used = pos.len()+n_pending_buys`.
- `held`: positions not in `busy`, with `value = pool_value`, `cost_share = cost·tokens/tokens0`, `hold_sol = value * hold_table.value(p.last_hold)`.
- `can_replace = hold.is_some()`.

Applying the decisions:
- **Merged:** `outcomes.push((coin,"merged_in_tick"))`, `merged_in_tick += 1`.
- **Block:** `blocked[r] += 1` and push to `outcomes`.
- **Replace:** `busy.insert(w)` and push `Order::Sell{coin:w, tokens: pos[&w].tokens, reason:"Replaced", close:true}`. Then do the same as Enter, and `replacements += 1`.
- **Enter:** push `"entered"` to `outcomes`, `busy.insert`, `pending_buy_cost += bet`, `entries.push_back(last_t)` (non-oracle), and push `Order::Buy{..., ev: cand.ev, label: cand.label}`.

Also add a constant `const MIN_EXIT_LIQ_SOL: f64 = 0.5;`, replacing the literal at line 510.

**Tests** (strategy_v5.rs `mod tests`):
- `allocate_ranks_by_ev_not_arrival`: cands A(ev 0.2, order 1) and B(ev 0.9, order 2); capacity for exactly one bet; no hold table → `[Enter(B), Block(A,"exposure_cap")]`.
- `allocate_replaces_when_exposure_is_binding` (the V5-1 scenario): `slots_used` = 10 < 18, exposure full, held hold_sol `[0.004, -0.02]`, cand ev 0.6 → `Replace(cand, coin with -0.02)`.
- `allocate_without_hold_table_never_replaces`: same as the previous test with `can_replace = false` → `Block "exposure_cap"`.
- `allocate_dedupes_coin_keeps_latest_row`
- `allocate_respects_entries_per_minute_in_rank_order`: cap 1 and three cands → the highest EV enters, the others are blocked `max_entries_per_min`.
- `allocate_cash_reserved_by_pending_buys`
- `two_coins_same_tick_higher_ev_wins` (simulate-level):
  - Two AMM coins with interleaved buy streams (3 distinct buyers/s at 0.3 SOL, the same pattern as `ladder_on_a_runner`), both promoted inside one tick. Use `decision_tick_s = 10` for robustness.
  - Scorer keyed on `x[quote_res_sol]`: pool 80 SOL → bin 5, 50 SOL → bin 1. rk = `[0,0.2,0,0,0,3.0,0]`.
  - `start_usd`/`sol_usd` chosen so that exactly one bet fits under the cap.
  - Assert that the 80-SOL coin is entered and the other is blocked `exposure_cap`.
  - With `decision_tick_s = 0`, the coin promoted in the earlier slot enters (first-come), which documents the difference.
- The existing `ladder_on_a_runner` and `ev_gate_blocks` must stay green after the new `simulate` signature (`hold = None`).

**Overlap: D7 (strategy WP) should own the final version.** D7 will add WATCH/ARM/price guard and Momentum Fade. `allocate` is the slot/ranking core it should reuse.

## 2. V5-7 — EV units mismatch (CONFIRMED)

**Where:** strategy_v5.rs.
- 305: `ev_of` = Σ P(bin_k)·R_k. R_k is the mean *net* return per bet of this exit policy (oracle runs on training windows, costs included).
- 307: `hold_ev` = p_hold·1.0 − (1−p_hold)·0.5. This is a gross two-outcome bet with no costs and a different policy. It is used at 553.
- Extra issue: under `NoScores`, hold = 0 → hold_ev = −0.5 for every position. After the V5-1 fix this would make v5-compare replace positions constantly.

**Fix:** one unit, *expected net SOL*.
- Candidate: `b·EV_c − txc`. The `txc` is the replacement sell transaction.
- Held position: `HV_w = V_w · H(p_hold_w)`.
  - `V_w` = current liquidation value (`pool_value`, fees included).
  - `H` = expected net forward return of *continuing to hold under the same exit policy*, per SOL of current value. It is measured on the training windows' oracle runs, exactly like R_k.
- Replace iff `b·EV_c − txc > HV_w + replace_margin·b`. The default margin 0.15 now means 0.15·bet SOL, which is equivalent to the old per-bet unit.
- Choosing the victim by minimum HV protects large runners: a 5x position with H > 0 has a large HV.

New code (strategy_v5.rs):
```rust
pub const HOLD_PRIOR_W: f64 = 5.0;
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct HoldTable { pub r: [f64; 11], pub w: [f64; 11] }   // 10 P(hold) deciles + index 10 = no prediction yet
impl HoldTable {
    pub const NO_PRED: usize = 10;
    pub fn bucket(p: Option<f32>) -> usize { match p { Some(x) if x.is_finite() => ((x.clamp(0.0,1.0)*10.0) as usize).min(9), _ => Self::NO_PRED } }
    /// obs = (bucket, forward net return, weight); r clipped to [-1, 1000]; shrunk to the pooled mean with HOLD_PRIOR_W.
    /// None if there is no observation (=> replacement disabled).
    pub fn fit(obs: &[(u8, f64, f64)]) -> Option<Self>;
    pub fn value(&self, p: Option<f32>) -> f64 { self.r[Self::bucket(p)] }
}
/// Forward net return from each hold mark (bucket, value at mark, legs already sold at mark):
/// (Σ_{legs[k..]} (proceeds_sol − txc)) / value − 1, weight 1/marks (each trade weighs 1 in total).
pub fn forward_returns(marks: &[(u8, f64, u32)], legs: &[Leg], txc: f64) -> Vec<(u8, f64, f64)>;
```
Changes to `Pos`:
- Add `last_hold: Option<f32>`. Set it at held snapshots where `pred` is Some, next to `p.last = pr` at line 430. It replaces `p.last` for the hold value.
- Add `marks: Vec<(u8,f64,u32)>`, pushed only `if oracle` at held snapshot events (bucket from `pred`, `NO_PRED` if the scorer returned None; `val` from line 422; `p.legs.len()`). Marks are recorded before the sell decision, and only while not busy. Using `legs.len()` instead of time avoids same-second ambiguity.

Changes to `TradeV5`: add `#[serde(skip)] pub hold_obs: Vec<(u8,f64,f64)>`, filled at close from `forward_returns`.

New signatures:
```rust
pub fn simulate(w: &WindowData, rp: RadarParams, scorer: &mut dyn Scorer, st: &StratV5, c: &CostsV5, rk: &[f64; 7], hold: Option<&HoldTable>, oracle: bool) -> ReportV5
pub fn estimate_rk(train: &[(&WindowData, Option<&FileScores>)], rp: RadarParams, st: &StratV5, c: &CostsV5) -> ([f64; 7], [usize; 7], Option<HoldTable>)
```
Call sites:
- `v5_eval.rs:77,79`: `let (rk, rk_trades, hold) = ...; simulate(..., &rk, hold.as_ref(), false)`. Add `pub hold: Option<HoldTable>` to `WindowResult` and `"hold": w.hold` to the windows JSON.
- `v5_eval.rs:390`: `simulate(..., &[1.0;7], None, false)`. No model means no replacement, which is honest.
- `estimate_rk` internals, lines 622-623: pass `None`.
- Tests at 695 and 726: pass `None`.

**Tests:**
- `hold_table_fit_and_lookup`:
  - obs `[(0,-0.5,1),(9,1.0,1)]` → prior 0.25; r[0] = 0.125, r[9] = 0.375, r[5] = 0.25.
  - `value(None)` and `value(Some(NaN))` both give r[10] = 0.25.
  - `fit(&[])` is None.
- `forward_returns_from_marks`:
  - marks `[(3,0.10,0),(7,0.20,1)]`, legs proceeds `[0.10, 0.15]`, txc 0.0005.
  - Expected → `[(3,1.49,0.5),(7,-0.2525,0.5)]`.
- `allocate_replacement_compares_expected_sol`:
  - b 0.05, txc 0.0005, margin 0.15, cand ev 0.3 → b·ev − txc = 0.0145.
  - Victim hold_sol 0.005 → 0.0145 > 0.005 + 0.0075 → replace.
  - Victim hold_sol 0.008 → blocked.
- `estimate_rk_returns_hold_table`: the ladder runner stream in oracle mode gives `Some` table with finite values.

**Overlap: D7 should own this** (it pairs with ranking). D6 must keep `p_hold` in the score files (it is already there: `p_hold`).

## 3. V5-8 — DevSell kill only on a single ≥50% creator sell (CONFIRMED)

**Where:**
- strategy_v5.rs:441 `step.dev_sell && step.dev_sell_frac >= st.dev_kill_min_frac`.
- ml_sim.rs:436-440: the fraction is per event (`tokens / balance before`), creator only.
- ml_sim.rs:499: `dev_sell = user == creator`.

Repeated 49% sells (1000 → sell 490 → sell 249 …) never reach 0.5. Sells through other wallets are invisible.

**Fix:** cumulative creator-family accounting in `Engine`, the single feature/state path, so it is causal.
- New fields in `Coin`: `fam_bal: i64`, `fam_peak: i64`, `fam_sold: u64` (tokens, observed trades only).
- In `apply`, *before* `let c = &mut self.coins[ci]` (borrow order):
  ```rust
  let fam = e.user == e.creator || self.ctx.dev_family.as_ref().is_some_and(|m| m.get(&(e.creator, e.user)).is_some_and(|since| *since <= e.time));
  ```
- After `c` is borrowed:
  - if `fam && e.tokens > 0`:
    - buy → `fam_bal += tokens; fam_peak = max(fam_peak, fam_bal)`;
    - sell → `fam_sold += tokens; fam_bal -= tokens` and `dev_family_sell = true`.
  - `fn cum_frac(c) = if fam_sold == 0 {0} else { fam_sold / max(fam_peak.max(0), fam_sold) }`. When prior holdings are unknown (an old coin, or sells exceeding observed buys), the result is 1.0, which matches the V5 rule "before ≤ 0 → frac 1.0".
- `Step` gains `pub dev_family_sell: bool, pub dev_cum_sell_frac: f64`. Keep `dev_sell`/`dev_sell_frac` unchanged; the V4 ML sim at ml_sim.rs:1580 still uses them.
- New accessor `pub fn dev_cum_sell_frac(&self, coin: u32) -> f64` (for D4's entry veto).
- `Context` gains `pub dev_family: Option<FxHashMap<(u32 /*creator id*/, u32 /*wallet id*/), i64 /*since*/>>`, loaded by `load_context` from optional `<rec>/dev_family.csv`.
  - Header `creator,wallet[,since]` (base58 addresses; `since` = unix seconds the link was known, absent → `i64::MIN`).
  - Addresses are mapped via `ev.users`; unknown addresses are skipped.
  - Build the `ids` map outside the `wallet_scores.csv` branch.
- strategy_v5.rs:441 becomes `if step.dev_family_sell && step.dev_cum_sell_frac >= st.dev_kill_min_frac { DevSell }`. The `dev_kill_min_frac` doc becomes "cumulative family tokens sold ≥ this fraction of the family's peak observed holdings".
- Fix the struct literals: the `Context` literal in the causality test (ml_sim.rs:1871) gets `..Default::default()`, and so does `load_context` (1237).

**Data / degradation:** creator ids exist in every dataset (`Ev.creator`), so the creator-only cumulative kill works on bf_1h/h1d. The "linked wallets" need a funding graph (SOL transfers), which is in no dataset. `dev_family.csv` is the interface D4 must produce (from System-Program transfers decoded by a new decoder pass). Links must be causal (`since` ≤ event time), or be built strictly before the window start. Without the file → creator only.

**Tests** (ml_sim.rs):
- `dev_cumulative_sell_frac_creator`: buy 1,000,000, sell 490,000, sell 249,000.
  - Per-event `dev_sell_frac` is 0.49 and 0.488 (both < 0.5).
  - `dev_cum_sell_frac` is 0.49, then 0.739.
- `dev_family_from_context`: `dev_family = {(999,7): 0}`. Creator 999 and wallet 7 each buy 1000; wallet 7 sells 1000 → `dev_family_sell = true`, frac 0.5, while `dev_sell` stays false.
  - Same with since = 10_000 > t → not family.
- `dev_unknown_holdings_count_as_full`: creator sells with no observed buys → 1.0.
- `features_do_not_see_the_future` stays green.

**Test** (strategy_v5.rs):
- `devsell_kill_is_cumulative`: the ladder stream plus a creator buy of 1,000,000 tokens at t=0, sells of 490,000 at t=60 and 249,000 at t=70. Needs a local `mk` with a `tokens` argument.
- Asserts: `final_reason == "DevSell"` with the exit after t=70; with the old rule it would not exit.

**Overlap: D4 (safety WP) owns the family definition** (funding graph, dev history, entry veto). D1 provides the cumulative accounting and the file hook.

## 4. V5-9 — −65% barrier in y_bin (CONFIRMED; diagnostic only, no label change)

**Where:** ml_sim.rs:1109-1112 (`barrier = m <= 0.35` freezes `peak_b`). The test at 1805-1808 asserts that a dump to 0.3 followed by 10x gives bin 0.

**Diagnostic** (one definition of p0, horizon and censoring, shared with `labels`):
```rust
pub const BARRIERS_DIAG: [f64; 4] = [0.35, 0.25, 0.15, 0.10];
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BarrierPath { pub peak: f64 /* = exp(y_logmax) */, pub t_peak: i64, pub min_before_peak: f64, pub peak_before: [f64; 4] /* [0] == y_peak_barrier */ }
pub fn barrier_path(path: &[(i64, f64)], t: i64, cover_end: i64) -> Option<BarrierPath>
```
How `barrier_path` works:
- Same `i0`/`p0`/`end` as `labels`, looping over `path[i0+1..]`.
- `peak` starts at 1.0. On `m > peak`: `peak = m`, `t_peak = tt − t`, `min_before_peak = min_run`. Then `min_run = min(min_run, m)`.
- For each barrier k, the `labels` freeze logic runs; NaN becomes 1.0.

New file `crates/app/src/label_diag.rs`:
- `rows(ev, rp) -> Vec<Row{coin,t,first_signal,covered,b}>`: an Engine pass; rows are radar Promote with `e.time <= window.1`; `covered = label_covered(..)`.
- `summarize(&[&Row]) -> Summary`:
  - `tiers` for min_peak 10 and 5, each with `n`, `direction_only (n<30)`, `touched_before_peak[4]` (min_before_peak ≤ barrier_k), `share_touched[4]`, `keeps_tier[4]` (peak_before[k] ≥ min_peak), `min_before_peak_p10_p25_p50`;
  - `bin_mix_by_barrier: [[usize;7];4]` over all rows.
- `run(recs, rp, out)` writes per window and pooled `{covered:{first_signal_coins, rows}, all_censored:{...}}` to JSON.

CLI `sniper label-diag --rec ... [--radar-params config/radar_params.json] [--out reports/V5_9_BARRIER.json]`.

**Proposed decision rule (to be fixed with the user before looking at results):** keep −65% if fewer than 10% of covered ≥10x first-signal runners touch it before their peak. Otherwise propose the smallest tested barrier at which ≥ 90% keep bin ≥ 5. If the barrier is widened, it becomes a NEW appended label `y_bin_w<NN>` (owner D6); `y_bin` is not changed.

**Data:** on bf_1h/h1d, cover_end = the 33-min window end and H = 10 h, so every row is censored and the result is direction only. Real numbers need V5 windows with `sniper follow --hours 10`.

**Tests:**
- `barrier_path_matches_labels`: on LCG pseudo-random paths, `peak_before[0] as f32 == labels().peak_barrier` and `ln(peak) ≈ logmax`.
- `barrier_path_dump_then_run`: path [(101,1),(150,0.3),(300,12)], t=100 → peak 12, t_peak 200, min_before_peak 0.3, peak_before [0.3,12,12,12], bins 0/5/5/5.
- `label_diag_summary_counts` (in label_diag.rs).

## 5. V5-10 — entry model trains on post-window signal rows (CONFIRMED)

**Where** (python/ml/v5_train.py):
- 136-138: `_rows("signal") = is_signal == 1`.
- 639: metrics mask, same.
- 88: `signal_rows` count, used for task cost at 590.

The radar keeps promoting on follow-up events (strategy_v5.rs:487 only enters when `e.time <= window.1`), and regime features are −1 after the window (ml_sim.rs:1014-1016).

**Fix:**
- `def entry_rows(cols): return (cols["is_signal"] == 1) & (cols["in_window"] == 1)`. Use it in `_rows("signal")` (entry_bin, entry_tp1, baseline_lr) and in the metrics mask at 639.
- Rug, hold and hazard keep all rows: they serve open positions after the window, so the distribution matches.
- `load_window` info: `signal_rows` = in-window entry rows, new `signal_rows_all`, and `CACHE_VERSION = 2` stored and checked (`info.get("cache_version") == CACHE_VERSION`). This fixes an extra bug: stale `info.json` caches have no version.
- Line 73: relax the label assert to a prefix, `header[i_y:i_y+len(LABELS)] == LABELS`, so other WPs can append label columns.

**Tests** (python/ml/tests/test_v5_train.py, no GPU):
- `test_entry_rows_in_window_only` (`_rows` on a small cols dict → `[T,F,F,T]`).
- `test_load_window_accepts_appended_labels_and_versions_cache`: `make_window`, add column `y_extra`, `load_window` → `cache_version == 2` and `signal_rows ==` the in-window signal count.

**Overlap: D6 (ML WP) owns the final version** (pre-pump/flat-state rows extend `entry_rows`).

## 6. V5-12 — bot flag without minimum trips (CONFIRMED)

**Where:** python/research/wallet_scores.py:312 (`fast_share` over closed trips) and 317-319. One fast trip makes a wallet a bot.

**Fix:**
- `BOT_MIN_TRIPS = 5`. Rule: `fast_rule = (fast_share > BOT_FAST_SHARE) & (n_closed >= BOT_MIN_TRIPS)`.
- The mints-per-day rule is unchanged.
- New meta keys: `n_bots_fast_rule`, `n_bots_mints_per_day_rule`, `n_fast_share_below_min_trips`, `params.bot_min_trips`.
- Update the docstring.

**Test:** `test_bot_flag_needs_min_trips` uses its own scenario builder, so existing counts (119 trips, `n_bots` == 2) are untouched. `Wfast2` (2 trips of 5 s) → is_bot 0, is_flipper 1, fast_share 1. `Wfast5` (5 fast trips) → is_bot 1. Meta `n_fast_share_below_min_trips == 1`. The existing `test_bot_flag` (Wfast has 5 trips) stays green.

**Data:** wallet_scores.py needs V5 `trades.csv.zst` (columns `order`, `trader`). Per Appendix A, the V4 dataset CSVs have `trader_id` and no `order` (verify with `zstd -dc | head -1`), so the effect is measurable only on V5 windows (meta counts).

**Overlap: D5 (clustering WP) should own it.**

## 7. V5-13 / V4-4 — liquidity check on virtual reserves (CONFIRMED)

**Where:**
- ml_sim.rs:556 (`untradable` uses `quote_res`, which on the curve is virtual ≈ 30 SOL + real), used at strategy_v5.rs:510 and ml_sim.rs:1647.
- backtest.rs:604 (V4 rule entry; was :549 on main).
- Data: `low_liquidity` blocked only 7 and 13 signals in V5_COMPARE.

**Fix:** a new helper in both `Pool` types: `fn exit_liquidity_sol(&self) -> f64 { if self.amm { quote_res } else { real_quote } / 1e9 }`. It is used in:
- `Engine::untradable` (reason name `low_liquidity` unchanged);
- new `pub fn exit_liquidity_sol(&self, coin) -> f64`;
- backtest.rs:604.

Price-impact sizing (backtest.rs:647-648, ml_sim.rs:1668) correctly keeps virtual reserves, per spec Bagian 4. V5 threshold: `MIN_EXIT_LIQ_SOL = 0.5`. V4 `entry_min_quote_res_sol` (5.0) and `min_quote_res_sol` keep their names but now mean real SOL. This changes V4 numbers in v5-compare; report it.

**Tests:**
- `untradable_uses_real_quote_on_curve` (ml_sim): curve with q = 31e9 virtual and real_quote 0.3e9 → `Some("low_liquidity")`; with real 2e9 → None; AMM with q 0.3e9 → low; AMM with 10e9 → None.
- `exit_liquidity_uses_real_quote_on_curve` (new `#[cfg(test)]` in backtest.rs).

## 8. V5-14 / V4-5 — price_vs_vwap60 is a plain mean (CONFIRMED)

**Where:** ml_sim.rs:648-658 (`vw_q` is summed and discarded, `vw_t` is the mean of `c.px` points); feature at 707, index 36 (was :458 on main).

**Fix (in place):**
- New `Coin.pxv: VecDeque<(i64 /*t*/, f64 /*marginal price after event*/, f64 /*SOL*/)>`, pushed in the same branch as `c.px` (line 472; late curve events after migration are skipped) and trimmed to `<= e.time-60`.
- In `features`: `vw = Σ_{tt>t-60, p>0} p·sol / Σ sol`; feature = `price/vw` if `vw > 0` else 1.0. Remove `vw_q`/`vw_t`.
- Same unit as `price` (marginal lamports per raw token). Index and name unchanged.

**Test:** `vwap_is_sol_weighted`: events at t = 100 and 101 (avoid the `first_time == 0` sentinel) with 1 SOL at price 1.0 and 9 SOL at price 2.0 → `features(0,101)[36] == 2/1.9`, where the old value was 2/1.5. The causality test stays green.

## 9. V5-16 — SOL/USD via curl, failure → NaN / abort (CONFIRMED, worse than stated)

**Where:**
- ml_sim.rs:1264-1301 `sol_usd_series`: breaks on any failure → empty → `sol_ret_1h` NaN at 980-983.
- backtest.rs:714-720 `sol_usd_at` *errors*, aborting every command that loads windows: main.rs:409, 460, 486, 534, 558; v5_eval.rs:28; diagnose.rs:27.
- Two separate network paths fetch the same data. api.binance.com geo-blocks some regions (HTTP 451 + JSON object); that is parsed as "no SOL price" and fails.

**Fix:** new module `crates/app/src/sol_usd.rs`:
```rust
pub const CACHE_FILE: &str = "sol_usd_1m.csv"; pub const MAX_STALE_S: i64 = 180;
#[derive(Debug, Clone, Default)] pub struct Opts { pub offline: bool, pub fallback_usd: Option<f64>, pub import: Option<PathBuf> }
pub fn set_opts(o: Opts);                       // OnceLock, set in main from CLI
#[derive(Debug, Clone, Default)] pub struct Series { pub pts: Vec<(i64, f64)>, pub source: &'static str } // cache|import|network|partial|missing
pub fn parse_csv(text: &str) -> Vec<(i64, f64)>;
pub fn merge(a: &[(i64,f64)], b: &[(i64,f64)]) -> Vec<(i64,f64)>;   // union by close time, sorted, finite >0 only
pub fn covers(pts: &[(i64,f64)], from: i64, to: i64) -> bool;       // first <= from+120 && last >= min(to, from+60000)-120
pub fn close_at(pts: &[(i64,f64)], t: i64) -> Option<f64>;          // last close_time <= t, stale <= MAX_STALE_S (feature, causal)
pub fn accounting_at(pts: &[(i64,f64)], t: i64) -> Option<f64>;     // first close_time >= t+60 (= old sol_usd_at semantics), else close_at
pub fn load_with(rec: &Path, from: i64, to: i64, o: &Opts, fetch: &dyn Fn(i64, i64) -> anyhow::Result<Vec<(i64, f64)>>) -> anyhow::Result<Series>;
pub fn load(rec: &Path, from: i64, to: i64) -> anyhow::Result<Series>;          // load_with(.., &opts(), &fetch_binance)
pub fn accounting_usd(rec: &Path, ev: &Events) -> anyhow::Result<(f64, &'static str)>; // series(window.0-3700, data_end); accounting_at(window.0) | fallback_usd | Err(clear message)
fn fetch_binance(from: i64, to: i64) -> anyhow::Result<Vec<(i64, f64)>>;
```
Load order:
1. `<rec>/sol_usd_1m.csv`.
2. The read-only shared import file (default `data/sol_usd_1m.csv`; the user can drop any `close_time,close` CSV there).
3. Network, unless `offline`.

On a partial result, merge and write the per-rec cache atomically (tmp + rename). It is never written to the shared file, because Snapshots runs in parallel.

`fetch_binance` works like this:
- curl with `-s -m 30 -w`, checking the exit status.
- It accepts only a JSON array of klines.
- Hosts: `https://api.binance.com`, then `https://data-api.binance.vision` (Binance's public market-data host; reachability from the VPS must be verified). 3 tries each with a 2 s backoff, paging as now.
- It never fabricates a price.

Callers:
- `ml_sim::load_context` uses `sol_usd::load`, sets `ctx.sol_usd` and the new `ctx.sol_usd_source`.
- The `sol_ret_1h` feature uses `close_at` (NaN when missing or stale).
- Replace every `backtest::sol_usd_at(e.window.0)?` with `sol_usd::accounting_usd(r, &e)?.0`. Delete `backtest::sol_usd_at` and `ml_sim::sol_usd_series`.
- `snapshots_v5_stats.json` gains `sol_usd_source`.

New global CLI args in main.rs: `--sol-usd <USD>` (Option<f64>), `--offline` (bool), `--sol-usd-import <PATH>` (default `data/sol_usd_1m.csv`) → `sol_usd::set_opts`.

Live trading (on-chain oracle or SOL/USDC pool price) is out of scope (Phase 8). There is no SOL/USDC pool data; the decoder covers pump.fun only.

**Tests** (sol_usd.rs; temp dir under `std::env::temp_dir()` with the pid; injected fetchers, never network):
- `cache_hit_skips_fetch`: the fetch closure panics if called.
- `offline_without_cache_is_missing_and_accounting_needs_fallback`: Err without fallback, `(150.0,"fallback")` with it.
- `partial_cache_is_extended_and_written`
- `fetch_error_degrades_to_missing`
- `accounting_at_matches_binance_starttime_semantics`: t aligned → candle opening at t; t+1 → next candle.
- `close_at_rejects_stale`

## 10. V5-17 — V5 snapshot format breaks V4 scripts (CONFIRMED)

**Where:**
- main.rs:446 writes `snapshots_v5.csv.zst` (LABEL_COLS ml_sim.rs:1185 has no y_up2…y_devsell10m).
- The V4 scripts read `{d}/snapshots.csv`: python/ml/train_eval.py:24, nested_lowo.py:12, nested_lowo_model.py:12, ablation.py:15, compare_models.py:10.
- The Rust V4 ML commands (backtest-ml, ml-sim, ml-diagnose, backtest-lowo) look up V4 score files by `order` (ml_sim.rs:1590, 1625). The V5 snapshot cadence (ml_sim.rs:514, adaptive + forced on Promote) differs, so they silently match few rows.

**Decision: retire explicitly** (V4 labels are also right-censored, V4-2; re-adding a V4 writer would preserve a known-bad label).
- New `python/ml/legacy_v4_guard.py`: `require_v4_snapshots(d) -> str`. It raises SystemExit with instructions when `snapshots.csv` is missing, when the header contains `is_signal`/`in_window`/`y_bin`, or when it lacks `{y_up2,y_up5,y_cont,y_rug}`. Instructions: use a worktree at e75a8c5, or use `v5_train.py`.
- The 5 V4 scripts call it in place of their path string and get a "LEGACY V4" docstring header.
- Rust: `pub fn score_coverage(ev, rp, sc: &Scores) -> (usize /*snapshot rows*/, usize /*with a score*/)` (one Engine pass). The four V4 ML CLI handlers print a WARNING when coverage < 0.5 and add `score_coverage` to their JSON. It warns rather than bails, because V4 and V5 cadences legitimately differ.

**Tests:**
- python/ml/tests/test_legacy_v4_guard.py: `test_guard_rejects_v5_layout`, `test_guard_accepts_v4_layout`, `test_guard_missing_file`.
- Rust `score_coverage_counts`: scores for half the snapshot orders → (n, n/2).

## 11. V4-12 — quarter-Kelly on uncalibrated p_up2 (CONFIRMED)

**Where:** ml_sim.rs:1657-1667 (Kelly sizing) and 1735 (every tune grid point has `kelly_fraction: 0.25`; the comment at 1724 says quarter-Kelly). It was :1139 on main.

**Fix:**
- `MlStrategy` gets a container `#[serde(default)]`. This also fixes an extra bug: stale configs such as config/ml_strategy_params.json lack many fields and currently fail to deserialize.
- New field `pub calibrated: bool` (default false).
- `pub fn bet_fraction(st: &MlStrategy, p2: f32) -> Option<f64>`:
  - Kelly only if `kelly_fraction > 0 && calibrated`; `None` for negative edge;
  - otherwise `Some(size_frac)`.
  - `simulate` uses it.
- `pub fn tune_grid() -> Vec<MlStrategy>`: `kelly_fraction 0.0`, with the `kelly_loss` loop removed (192 points); `tune` uses it.
- StratV5 already uses a fixed `bet_sol`.
- The calibration-pass signal for any future Kelly (V5 plan 7.6) is D6's job (see interfaces).

**Tests:**
- `bet_fraction_fixed_until_calibrated` (0.25/false → `Some(size_frac)`; true, p = 0.9 → `Some(0.10)` cap; true, p = 0.1 → None).
- `tune_grid_is_fixed_size` (all `kelly_fraction == 0`, len 192).

## 12. Extra findings (not in file 01; folded into the fixes above)

- E1: strategy_v5.rs:565 — pending buys do not reserve cash (fixed in V5-1).
- E2: NoScores gives hold_ev = −0.5, which would cause replacement churn after V5-1 (fixed in V5-7: no table → no replacement).
- E3: v5_train.py:64-67 — the cache info has no version (fixed in V5-10).
- E4: Binance geo-block makes every window-loading command fail (fixed in V5-16).
- E5: `MlStrategy` has no `serde(default)` (fixed in V4-12).
- E6: `Coin.first_time == 0` is used as a sentinel (ml_sim.rs:363), and `blockTime` can default to 0 (backtest.rs:126). Not fixed; minor.


## Files touched

- `crates/app/src/strategy_v5.rs` — V5-1: decision-tick candidate batching, the pure allocate() (Cand/Held/Capacity/Decision), cash reservation, StratV5.decision_tick_s (serde default 2), ReportV5.replacements/merged_in_tick, MIN_EXIT_LIQ_SOL. V5-7: HoldTable, forward_returns, Pos.last_hold/marks, TradeV5.hold_obs, simulate(+hold: Option<&HoldTable>), estimate_rk returns a HoldTable. V5-8: DevSell on dev_family_sell && dev_cum_sell_frac. Tests.
- `crates/app/src/ml_sim.rs` — V5-8: Coin fam_bal/fam_peak/fam_sold, Step.dev_family_sell/dev_cum_sell_frac, Context.dev_family + dev_family.csv loader, Engine::dev_cum_sell_frac. V5-13: Pool::exit_liquidity_sol in untradable + Engine::exit_liquidity_sol. V5-14: Coin.pxv and SOL-weighted VWAP. V5-9: BARRIERS_DIAG, BarrierPath, barrier_path. V5-16: load_context uses sol_usd::load, Context.sol_usd_source, sol_ret_1h via close_at; sol_usd_series deleted. V4-12: MlStrategy #[serde(default)] + calibrated, bet_fraction, tune_grid. V5-17: score_coverage. Test literal fixes and new tests.
- `crates/app/src/backtest.rs` — V4-4: Pool::exit_liquidity_sol used at line 604; sol_usd_at removed (moved to sol_usd.rs); new #[cfg(test)] module.
- `crates/app/src/sol_usd.rs` — NEW (V5-16): cache → import → network (two Binance hosts, retries, status/shape checks) with injectable fetcher; close_at / accounting_at; accounting_usd with explicit --sol-usd fallback; tests.
- `crates/app/src/label_diag.rs` — NEW (V5-9): Engine pass over in-window signal rows, BarrierPath per row, summary per window and pooled (covered vs censored), JSON report; tests.
- `crates/app/src/v5_eval.rs` — Uses sol_usd::accounting_usd in load_windows; estimate_rk 3-tuple; simulate gets hold.as_ref(); WindowResult.hold; compare passes None.
- `crates/app/src/diagnose.rs` — sol_usd::accounting_usd in place of backtest::sol_usd_at (line 27).
- `crates/app/src/main.rs` — mod sol_usd, mod label_diag; global --sol-usd/--offline/--sol-usd-import; LabelDiag subcommand; sol_usd calls at 409/460/486/534/558; score_coverage warnings + JSON field in the V4 ML commands; snapshots stats sol_usd_source.
- `python/ml/v5_train.py` — V5-10: entry_rows() = is_signal & in_window for 'signal' rows and metrics; CACHE_VERSION=2; signal_rows (in-window) + signal_rows_all; label assert relaxed to a prefix.
- `python/ml/tests/test_v5_train.py` — New tests: test_entry_rows_in_window_only, test_load_window_accepts_appended_labels_and_versions_cache.
- `python/research/wallet_scores.py` — V5-12: BOT_MIN_TRIPS=5 gates the fast-share bot rule; new meta counts and param; docstring.
- `python/research/tests/test_wallet_scores.py` — New test_bot_flag_needs_min_trips with its own scenario builder (existing counts untouched).
- `python/ml/legacy_v4_guard.py` — NEW (V5-17): require_v4_snapshots(d) guard with instructions.
- `python/ml/train_eval.py` — V5-17: LEGACY V4 docstring + guard call (line 24).
- `python/ml/nested_lowo.py` — V5-17: LEGACY V4 docstring + guard call (line 12).
- `python/ml/nested_lowo_model.py` — V5-17: LEGACY V4 docstring + guard call (line 12).
- `python/ml/ablation.py` — V5-17: LEGACY V4 docstring + guard call (line 15).
- `python/ml/compare_models.py` — V5-17: LEGACY V4 docstring + guard call (line 10).
- `python/ml/tests/test_legacy_v4_guard.py` — NEW: guard tests (V5 layout rejected, V4 layout accepted, missing file).

## Interfaces

## Rust (crates/app)

### strategy_v5.rs
- `StratV5.decision_tick_s: i64`: `#[serde(default = "default_decision_tick_s")]`, default 2; 0 = batch per slot.
- `const MIN_EXIT_LIQ_SOL: f64 = 0.5`.
- `pub fn simulate(w: &WindowData, rp: RadarParams, scorer: &mut dyn Scorer, st: &StratV5, c: &CostsV5, rk: &[f64; 7], hold: Option<&HoldTable>, oracle: bool) -> ReportV5`
- `pub fn estimate_rk(train: &[(&WindowData, Option<&FileScores>)], rp, st, c) -> ([f64; 7], [usize; 7], Option<HoldTable>)`
- `pub struct HoldTable { pub r: [f64; 11], pub w: [f64; 11] }` (Serialize).
  - `HoldTable::NO_PRED = 10`
  - `HoldTable::bucket(Option<f32>) -> usize`
  - `HoldTable::fit(&[(u8, f64, f64)]) -> Option<HoldTable>`
  - `HoldTable::value(&self, Option<f32>) -> f64`
- `pub const HOLD_PRIOR_W: f64 = 5.0`
- `pub fn forward_returns(marks: &[(u8, f64, u32)], legs: &[Leg], txc: f64) -> Vec<(u8, f64, f64)>`
- `pub(crate)` allocator:
  - `Cand { coin: u32, ev: f64, order: u64, label: Option<Labels> }`
  - `Held { coin: u32, hold_sol: f64, cost_share: f64, value: f64 }`
  - `Capacity { open_cost: f64, equity: f64, cash_free: f64, slots_used: usize, entries_60s: usize }`
  - `enum Decision { Enter(usize), Replace(usize, u32), Block(usize, &'static str), Merged(usize) }`
  - `fn allocate(cands: &[Cand], held: &[Held], cap: Capacity, st: &StratV5, txc: f64, can_replace: bool, oracle: bool) -> Vec<Decision>`
  - Replacement rule: `bet*ev - txc > hold_sol + replace_margin*bet`.
- `ReportV5` new fields: `replacements: usize`, `merged_in_tick: usize`.
- New `blocked` keys: `data_end`.
- New `signal_outcomes` value: `merged_in_tick`.
- `TradeV5` new field: `#[serde(skip)] hold_obs: Vec<(u8, f64, f64)>`.

### v5_eval.rs
- `WindowResult.hold: Option<HoldTable>`; the JSON windows gain a `"hold"` field.

### ml_sim.rs
- `Step` new fields: `dev_family_sell: bool`, `dev_cum_sell_frac: f64`. `dev_sell`/`dev_sell_frac` are kept for the V4 sim.
- `Context` new fields:
  - `dev_family: Option<FxHashMap<(u32 /*creator id*/, u32 /*wallet id*/), i64 /*since unix s*/>>`
  - `sol_usd_source: &'static str`
  - Struct literals need `..Default::default()`.
- `Engine` new methods: `pub fn dev_cum_sell_frac(&self, coin: u32) -> f64`, `pub fn exit_liquidity_sol(&self, coin: u32) -> f64`.
- `untradable` now uses real exit liquidity on the curve (`real_quote`) and `quote_res` on the AMM.
- `pub const BARRIERS_DIAG: [f64; 4] = [0.35, 0.25, 0.15, 0.10]`
- `pub struct BarrierPath { peak: f64, t_peak: i64, min_before_peak: f64, peak_before: [f64; 4] }`
- `pub fn barrier_path(path: &[(i64, f64)], t: i64, cover_end: i64) -> Option<BarrierPath>`
- `MlStrategy`: container `#[serde(default)]`, plus `calibrated: bool`.
- `pub fn bet_fraction(st: &MlStrategy, p2: f32) -> Option<f64>`
- `pub fn tune_grid() -> Vec<MlStrategy>`
- `pub fn score_coverage(ev: &Events, rp: RadarParams, sc: &Scores) -> (usize, usize)`
- Unchanged: FEATURES (107) and LABEL_COLS (9). D1 appends nothing. `price_vs_vwap60` (index 36) is now SOL-weighted.

### sol_usd.rs (new)
- `Opts { offline: bool, fallback_usd: Option<f64>, import: Option<PathBuf> }`, `set_opts(Opts)`
- `Series { pts: Vec<(i64, f64)>, source: &'static str }`, where source is one of cache | import | network | partial | missing | fallback.
- Functions: `parse_csv`, `merge`, `covers`, `close_at`, `accounting_at`, `load_with(rec, from, to, &Opts, fetch)`, `load(rec, from, to)`, `accounting_usd(rec, &Events) -> Result<(f64, &'static str)>`.
- `backtest::sol_usd_at` is REMOVED; use `sol_usd::accounting_usd`.

### label_diag.rs (new)
- `rows(ev, rp) -> Vec<Row>`, `summarize(&[&Row]) -> Summary`, `run(recs, rp, out)`.

## Files

- `<rec>/dev_family.csv`: header `creator,wallet[,since]`, base58 addresses; `since` = unix seconds the link became known. Produced by D4 from SOL transfers. Links must be causal (`since <= event time`) or built strictly before the window start. Absent → creator-only.
- `<rec>/sol_usd_1m.csv` and the optional shared read-only `data/sol_usd_1m.csv`: lines `close_time_unix_s,close_usd`, no header.
- `reports/V5_9_BARRIER.json`: per window and pooled, `{covered | all_censored: {first_signal_coins | rows: Summary}}`.
  - Summary = `{barriers, rows, tiers: [{min_peak, n, direction_only, touched_before_peak[4], share_touched[4], keeps_tier[4], min_before_peak_p10_p25_p50[3]}], bin_mix_by_barrier[4][7]}`.
- `snapshots_v5_stats.json` gains `sol_usd_source`.
- The V4 ML command JSON outputs gain `score_coverage`.

## CLI

- Global flags: `--sol-usd <USD>`, `--offline`, `--sol-usd-import <PATH>` (default `data/sol_usd_1m.csv`).
- New subcommand: `sniper label-diag --rec <dir>... [--radar-params config/radar_params.json] [--out reports/V5_9_BARRIER.json]`.

## Python

- v5_train.py:
  - `entry_rows(cols) = (is_signal==1) & (in_window==1)`, used for `signal` rows and for metrics.
  - `CACHE_VERSION = 2`.
  - info keys `cache_version`, `signal_rows` (in-window), `signal_rows_all`.
  - The label header check is a PREFIX check, so other WPs may append label columns after `y_t2x`.
- wallet_scores.py:
  - `BOT_MIN_TRIPS = 5`.
  - meta `n_bots_fast_rule`, `n_bots_mints_per_day_rule`, `n_fast_share_below_min_trips`, `params.bot_min_trips`.
- legacy_v4_guard.py: `require_v4_snapshots(d) -> str` (path), raising SystemExit otherwise.

## Needed from other WPs

- **D4:** produce `dev_family.csv` (funding graph) and own the family / entry-veto policy.
- **D6:** keep `p_hold` in score files. Add per-model `reliability_max_abs_err` / `calibration_pass` (< 0.05 per bin) to V5_MODELS.json before any Kelly sizing. Own any widened-barrier label (`y_bin_w<NN>`, appended).
- **D7:** reuse `allocate` / `HoldTable` for ranking, WATCH/ARM and Momentum Fade.

## Depends on

- D4: dev_family.csv (creator,wallet,since) from a funding graph built from SOL transfers — until then the V5-8 kill is creator-only
- D6: fold score files with p_hold (already produced by v5_train.py) so V5-7's HoldTable/replacement and V5-1's ranking can be measured; calibration_pass per model before any Kelly sizing (V4-12/plan 7.6)
- D7: final owner of the strategy loop (allocate, HoldTable) once Momentum Fade / WATCH-ARM land
- V5 windows with 10 h follow-up (sniper follow) for non-censored V5-9 numbers and for V5-10/V5-12 effects; trades.csv.zst with order+trader columns for wallet_scores.py

## Verified bugs

| id | confirmed | location | note |
|---|---|---|---|
| V5-1 | True | crates/app/src/strategy_v5.rs:537-568 (exposure_cap 544-549 checked before the slots/replacement branch 550-564; entry 569-574) | Replacement is reachable only when pos+pending buys >= max_positions (18). Exposure binds first: 0.6×(100/109.65 SOL) ≈ 0.547 SOL ≈ 10 bets of 0.05. reports/V5_COMPARE_v4windows.json shows exposure_cap 1381 / 2013 and slots_full 0. Decisions are per event (first-come). |
| V5-7 | True | crates/app/src/strategy_v5.rs:305 (ev_of = Σ P_bin·R_k, net per bet), 307 (hold_ev = p_hold·1 − (1−p_hold)·0.5, gross two-outcome bet), used at 553 | Different units and policies. Under NoScores, hold = 0 → −0.5 for every position, which would cause replacement churn once V5-1 is fixed. |
| V5-8 | True | crates/app/src/strategy_v5.rs:441; crates/app/src/ml_sim.rs:436-440 (per-event creator fraction), 499 | Single creator sell ≥ dev_kill_min_frac of the current balance. Repeated 49% sells (490 then 249 of 1000 = 73.9% cumulative) and sells via other wallets never trigger. |
| V5-9 | True | crates/app/src/ml_sim.rs:1109-1112 (barrier m<=0.35 freezes peak_b); test 1805-1808 asserts dump-to-0.3-then-10x → bin 0 | Diagnostic designed (barrier_path + label-diag). On bf_1h/h1d every row is censored (H = 10 h, window 33 min), so the answer there is direction only. |
| V5-10 | True | python/ml/v5_train.py:136-138 (_rows 'signal' = is_signal==1), also 639 (metrics mask), 88/590 (signal_rows count used for cost) | The radar keeps promoting on follow-up events, where regime features are −1 (ml_sim.rs:1014-1016). The strategy only enters in-window (strategy_v5.rs:487). |
| V5-12 | True | python/research/wallet_scores.py:312 (fast_share), 317-319 (bot flags) | A wallet with one closed trip under 10 s gets fast_share 1.0 and is flagged a bot. There is no minimum trip count. |
| V5-13/V4-4 | True | crates/app/src/ml_sim.rs:556 (untradable uses quote_res = virtual on curve; called at strategy_v5.rs:510 and ml_sim.rs:1647); crates/app/src/backtest.rs:604 (was :549 on main) | The curve's virtual quote reserve starts around 30 SOL, so the check never blocks there. V5_COMPARE low_liquidity blocks are only 7 and 13. The impact sizing at backtest.rs:647-648 and ml_sim.rs:1668 correctly uses virtual reserves. |
| V5-14/V4-5 | True | crates/app/src/ml_sim.rs:648-658 (vw_q summed then discarded; vw_t = plain mean of 60 s marginal prices), feature at 707 (index 36); was :458 on main | Not volume-weighted. |
| V5-16 | True | crates/app/src/ml_sim.rs:1264-1301 (sol_usd_series, empty on failure → sol_ret_1h NaN at 980-983); crates/app/src/backtest.rs:714-720 (sol_usd_at errors) called at main.rs:409,460,486,534,558, v5_eval.rs:28, diagnose.rs:27 | Worse than stated: sol_usd_at returns an error, so every window-loading command aborts when Binance is unreachable or geo-blocked (HTTP 451 JSON object). There are two separate network paths for the same data. |
| V5-17 | True | crates/app/src/main.rs:446 (writes snapshots_v5.csv.zst, labels ml_sim.rs:1185); python/ml/train_eval.py:24, nested_lowo.py:12, nested_lowo_model.py:12, ablation.py:15, compare_models.py:10 read {d}/snapshots.csv with V4 labels | The Rust V4 ML commands also look up score files by event order (ml_sim.rs:1590,1625), while the V5 snapshot cadence differs (ml_sim.rs:514). They match few rows silently. |
| V4-12 | True | crates/app/src/ml_sim.rs:1657-1667 (Kelly on uncalibrated p_up2), 1735 (every tune grid point has kelly_fraction 0.25); was :1139 on main | StratV5 already uses a fixed bet_sol. The fix is fixed size unless MlStrategy.calibrated. |
| EXTRA-cash-reservation | True | crates/app/src/strategy_v5.rs:565 (and fill guard 359) | The cash check ignores pending buys. Over-committed buys fail at fill but still pay txc. Fixed inside V5-1 (cash_free = cash − pending_buy_cost − pending·txc). |
| EXTRA-v5train-cache-version | True | python/ml/v5_train.py:64-67 | The info.json cache is keyed only on the source file. After any change to the row definition, the stale info is reused. Fixed with CACHE_VERSION in V5-10. |
| EXTRA-mlstrategy-serde-default | True | crates/app/src/ml_sim.rs:1303 (MlStrategy without #[serde(default)]); config/ml_strategy_params.json lacks giveback_arm etc. | Stale strategy JSONs fail to deserialize. Fixed by a container #[serde(default)] in V4-12. |
| EXTRA-first-time-sentinel | True | crates/app/src/ml_sim.rs:363 (c.first_time == 0 sentinel); backtest.rs:126 (blockTime unwrap_or(0)) | Minor: an event with block_time 0 breaks first-seen and age. Not fixed in D1; tests use t > 0. |

## Open questions

- V5-14: is fixing price_vs_vwap60 (index 36) in place acceptable? No V5 models exist yet. The alternative is to keep the old values and append a new 'price_vs_vwap60_sol' feature.
- V5-17: retire the V4 Python scripts behind a guard (proposed), or re-add a V4-label snapshot writer? A V4 writer would keep the right-censored V4 labels (V4-2).
- V5-1: default decision_tick_s = 2 s, per the forensic file's 2–5 s, adds up to 2 s of entry delay while 87% of ≥11x runners jump within 2 slots. Should the per-slot batch (0) be the default? Proposal: report both in the after-run.
- V5-7: the replacement rule compares expected net SOL (bet·EV − txc vs value·H(p_hold) + margin·bet). Is absolute SOL (which protects big runners) the intended 'same unit', rather than per-SOL returns?
- V5-8: should the creator's co-buying cluster from wallet_scores (cluster_id) count as 'linked wallets' until D4's funding graph exists? Proposal: no, it is a weak proxy (snipers/bundlers). Should an entry veto on dev_cum_sell_frac be added now or left to D4?
- V5-9: the keep/widen decision rule must be fixed before looking at results. Proposal: keep −65% if fewer than 10% of covered ≥10x runners touch it before the peak, else take the smallest tested barrier keeping ≥90% of them in bin ≥5. A widened label would be appended as a new column, not a change to y_bin.
- V5-16: is data-api.binance.vision reachable from the VPS? If neither Binance host is reachable, the user must pass --sol-usd (bf_1h 109.65, h1d 110.53 from V5_COMPARE_v4windows.json) or drop a data/sol_usd_1m.csv.
- V4-12 / V5-17: should the V4 ML simulator commands (backtest-ml, ml-sim, ml-diagnose, backtest-lowo) be kept at all, or removed together with the V4 scripts?
- Ownership: V5-1 and V5-7 → D7, V5-8 → D4, V5-12 → D5, V5-10 → D6. D1 fully designs them; the synthesizer decides who implements, to avoid double edits of strategy_v5.rs and wallet_scores.py.
