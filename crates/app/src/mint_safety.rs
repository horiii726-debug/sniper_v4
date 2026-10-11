//! Mint safety from chain state (honeypot / rug risk flags): `getMultipleAccounts` (jsonParsed) for every mint of the
//! given recordings, cached in `data/mint_safety.csv` (one row per mint, fetched once).
//!
//! Flags per mint: SPL Token vs Token-2022, mint authority set (supply can be inflated), freeze authority set
//! (holders can be frozen: honeypot), Token-2022 extensions that can block or tax sells (transfer hook, transfer fee,
//! permanent delegate, default-frozen accounts, non-transferable, pausable).
//! CAVEAT: this is the CURRENT account state, not the state at decision time (authorities can be revoked later);
//! backtests use it as an approximation, live trading reads it at decision time.

use anyhow::{anyhow, Result};
use feed::rpc::Rpc;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

pub const CACHE: &str = "data/mint_safety.csv";
pub const HEADER: &str = "mint,exists,program,mint_auth,freeze_auth,transfer_fee_bps,transfer_hook,permanent_delegate,default_frozen,non_transferable,pausable";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MintSafety {
    pub exists: bool,
    pub token2022: bool,
    pub mint_auth: bool,
    pub freeze_auth: bool,
    pub transfer_fee_bps: u32,
    pub transfer_hook: bool,
    pub permanent_delegate: bool,
    pub default_frozen: bool,
    pub non_transferable: bool,
    pub pausable: bool,
}

impl MintSafety {
    /// Sells can be blocked or confiscated by an authority (honeypot risk).
    pub fn honeypot_risk(&self) -> bool {
        self.freeze_auth || self.transfer_hook || self.permanent_delegate || self.default_frozen || self.non_transferable || self.pausable
            || self.transfer_fee_bps >= 1000
    }

    fn row(&self, mint: &str) -> String {
        let b = |x: bool| x as u8;
        format!("{mint},{},{},{},{},{},{},{},{},{},{}", b(self.exists), if self.token2022 { "t22" } else { "spl" }, b(self.mint_auth), b(self.freeze_auth),
            self.transfer_fee_bps, b(self.transfer_hook), b(self.permanent_delegate), b(self.default_frozen), b(self.non_transferable), b(self.pausable))
    }
}

/// Parse one `getMultipleAccounts` jsonParsed value (null = account does not exist).
pub fn parse(v: &Value) -> MintSafety {
    if v.is_null() {
        return MintSafety::default();
    }
    let info = &v["data"]["parsed"]["info"];
    let mut s = MintSafety {
        exists: true,
        token2022: v["data"]["program"].as_str() == Some("spl-token-2022"),
        mint_auth: info["mintAuthority"].is_string(),
        freeze_auth: info["freezeAuthority"].is_string(),
        ..Default::default()
    };
    for e in info["extensions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let st = &e["state"];
        match e["extension"].as_str().unwrap_or("") {
            "transferFeeConfig" => {
                let f = |k: &str| st[k]["transferFeeBasisPoints"].as_u64().unwrap_or(0) as u32;
                s.transfer_fee_bps = f("newerTransferFee").max(f("olderTransferFee"));
            }
            "transferHook" => s.transfer_hook = st["programId"].is_string(),
            "permanentDelegate" => s.permanent_delegate = st["delegate"].is_string(),
            "defaultAccountState" => s.default_frozen = st["accountState"].as_str() == Some("frozen"),
            "nonTransferable" => s.non_transferable = true,
            "pausableConfig" => s.pausable = st["authority"].is_string() || st["paused"].as_bool() == Some(true),
            _ => {}
        }
    }
    s
}

pub fn read_cache(p: &Path) -> Result<BTreeMap<String, MintSafety>> {
    let mut m = BTreeMap::new();
    if !p.exists() {
        return Ok(m);
    }
    for l in std::fs::read_to_string(p)?.lines().skip(1) {
        let v: Vec<&str> = l.split(',').collect();
        if v.len() != 11 {
            continue;
        }
        let b = |i: usize| v[i] == "1";
        m.insert(v[0].to_string(), MintSafety {
            exists: b(1), token2022: v[2] == "t22", mint_auth: b(3), freeze_auth: b(4), transfer_fee_bps: v[5].parse().unwrap_or(0),
            transfer_hook: b(6), permanent_delegate: b(7), default_frozen: b(8), non_transferable: b(9), pausable: b(10),
        });
    }
    Ok(m)
}

pub async fn fetch(rpc_url: String, mints: Vec<String>, cache: &Path) -> Result<BTreeMap<String, MintSafety>> {
    let rpc = Rpc::new(rpc_url);
    let mut m = read_cache(cache)?;
    let todo: Vec<String> = mints.into_iter().filter(|x| !m.contains_key(x)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    tracing::info!(cached = m.len(), todo = todo.len(), calls = todo.len().div_ceil(100), "mint safety");
    for chunk in todo.chunks(100) {
        let mut res = None;
        for attempt in 0..8u32 {
            match rpc.call("getMultipleAccounts", json!([chunk, {"encoding": "jsonParsed", "commitment": "confirmed"}])).await {
                Ok(v) if v.get("error").is_none() => {
                    res = Some(v);
                    break;
                }
                _ => tokio::time::sleep(std::time::Duration::from_millis(300 * 2u64.pow(attempt.min(5)))).await,
            }
        }
        let v = res.ok_or_else(|| anyhow!("getMultipleAccounts failed after retries"))?;
        let vals = v["result"]["value"].as_array().ok_or_else(|| anyhow!("getMultipleAccounts: no value"))?;
        if vals.len() != chunk.len() {
            return Err(anyhow!("getMultipleAccounts: {} values for {} keys", vals.len(), chunk.len()));
        }
        for (k, val) in chunk.iter().zip(vals) {
            m.insert(k.clone(), parse(val));
        }
    }
    let mut out = String::from(HEADER);
    out.push('\n');
    for (k, s) in &m {
        out.push_str(&s.row(k));
        out.push('\n');
    }
    let tmp = cache.with_extension("tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, cache)?;
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spl_and_token2022_flags() {
        let spl = json!({"data": {"program": "spl-token", "parsed": {"info": {"mintAuthority": null, "freezeAuthority": null}}}});
        let s = parse(&spl);
        assert!(s.exists && !s.token2022 && !s.honeypot_risk() && !s.mint_auth);
        let t22 = json!({"data": {"program": "spl-token-2022", "parsed": {"info": {"mintAuthority": "A", "freezeAuthority": null, "extensions": [
            {"extension": "transferFeeConfig", "state": {"newerTransferFee": {"transferFeeBasisPoints": 9900}, "olderTransferFee": {"transferFeeBasisPoints": 0}}},
            {"extension": "transferHook", "state": {"programId": null}},
            {"extension": "metadataPointer", "state": {}}]}}}});
        let s = parse(&t22);
        assert!(s.token2022 && s.mint_auth && !s.transfer_hook);
        assert_eq!(s.transfer_fee_bps, 9900);
        assert!(s.honeypot_risk());
        assert!(!parse(&Value::Null).exists);
        let fr = json!({"data": {"program": "spl-token", "parsed": {"info": {"freezeAuthority": "F"}}}});
        assert!(parse(&fr).honeypot_risk());
    }

    #[test]
    fn cache_round_trip() {
        let d = std::env::temp_dir().join(format!("ms_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("c.csv");
        let s = MintSafety { exists: true, token2022: true, freeze_auth: true, transfer_fee_bps: 250, pausable: true, ..Default::default() };
        std::fs::write(&p, format!("{HEADER}\n{}\n", s.row("M1"))).unwrap();
        assert_eq!(read_cache(&p).unwrap()["M1"], s);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
