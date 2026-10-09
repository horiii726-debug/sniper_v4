//! Env + config loading and secret redaction (V4 §30).

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

mod redact;
pub use redact::Redactor;

/// Which Solana cluster a feed talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cluster {
    Mainnet,
    Devnet,
}

impl std::str::FromStr for Cluster {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "mainnet" | "mainnet-beta" => Ok(Cluster::Mainnet),
            "devnet" => Ok(Cluster::Devnet),
            _ => bail!("unknown cluster {s}"),
        }
    }
}

/// Endpoints and credentials. `Debug` is implemented manually so secrets never leak.
#[derive(Clone)]
pub struct Settings {
    pub cluster: Cluster,
    pub api_key: String,
    pub grpc_url: String,
    pub grpc_token: String,
    pub ws_url: String,
    pub rpc_url: String,
    pub wallet_keypair_path: Option<String>,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings")
            .field("cluster", &self.cluster)
            .field("grpc_url", &self.grpc_url)
            .field("api_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Load `.env` (with `${VAR}` expansion) into a map without touching the process environment.
pub fn load_env_file(path: &Path) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    let iter = dotenvy::from_path_iter(path).with_context(|| format!("open {}", path.display()))?;
    for item in iter {
        let (k, v) = item?;
        out.insert(k, v);
    }
    // dotenvy expands ${VAR} only from the process env; expand from the file itself too.
    for _ in 0..4 {
        let snapshot = out.clone();
        for v in out.values_mut() {
            for (k, val) in &snapshot {
                let pat = format!("${{{k}}}");
                if v.contains(&pat) {
                    *v = v.replace(&pat, val);
                }
            }
        }
    }
    Ok(out)
}

impl Settings {
    pub fn load(env_path: &Path, cluster: Cluster) -> Result<Self> {
        let mut env = load_env_file(env_path)?;
        // Process env overrides the file (secret manager in production, §30).
        for (k, v) in std::env::vars() {
            if k.starts_with("HELIUS_") || k == "WALLET_KEYPAIR_PATH" || k == "SOLANA_CLUSTER" {
                env.insert(k, v);
            }
        }
        let get = |k: &str| -> Result<String> {
            match env.get(k) {
                Some(v) if !v.is_empty() => Ok(v.clone()),
                _ => bail!("missing env {k}"),
            }
        };
        let api_key = get("HELIUS_API_KEY")?;
        let (grpc_url, ws_url, rpc_url) = match cluster {
            Cluster::Mainnet => (get("HELIUS_GRPC_URL")?, get("HELIUS_WS_URL")?, get("HELIUS_RPC_URL")?),
            Cluster::Devnet => (
                get("HELIUS_GRPC_URL_DEVNET")?,
                get("HELIUS_WS_URL_DEVNET")?,
                get("HELIUS_RPC_URL_DEVNET")?,
            ),
        };
        let grpc_token = env.get("HELIUS_GRPC_TOKEN").filter(|v| !v.is_empty()).cloned().unwrap_or_else(|| api_key.clone());
        Ok(Settings {
            cluster,
            api_key,
            grpc_url,
            grpc_token,
            ws_url,
            rpc_url,
            wallet_keypair_path: env.get("WALLET_KEYPAIR_PATH").filter(|v| !v.is_empty()).cloned(),
        })
    }

    /// Every secret string that must never appear in logs.
    pub fn secrets(&self) -> Vec<String> {
        let mut s = vec![self.api_key.clone(), self.grpc_token.clone()];
        s.retain(|x| x.len() >= 8);
        s.sort();
        s.dedup();
        s
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Venue {
    pub name: String,
    pub program_id: String,
    #[serde(default)]
    pub official_source: String,
    #[serde(default)]
    pub official_checked: String,
    #[serde(default)]
    pub onchain_checked: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VenueRegistry {
    pub venue: Vec<Venue>,
}

impl VenueRegistry {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let reg: VenueRegistry = toml::from_str(&text)?;
        if reg.venue.is_empty() {
            bail!("no venues in {}", path.display());
        }
        Ok(reg)
    }

    pub fn program_ids(&self) -> Vec<String> {
        self.venue.iter().map(|v| v.program_id.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_expansion_and_debug_redaction() {
        let dir = std::env::temp_dir().join(format!("cfgtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(".env");
        std::fs::write(
            &p,
            "HELIUS_API_KEY=abcd1234-secret\nHELIUS_RPC_URL=https://x/?api-key=${HELIUS_API_KEY}\n\
             HELIUS_WS_URL=wss://x/?api-key=${HELIUS_API_KEY}\nHELIUS_GRPC_URL=https://g\n",
        )
        .unwrap();
        let env = load_env_file(&p).unwrap();
        assert_eq!(env["HELIUS_RPC_URL"], "https://x/?api-key=abcd1234-secret");
        let s = Settings::load(&p, Cluster::Mainnet).unwrap();
        assert_eq!(s.grpc_token, "abcd1234-secret");
        assert!(!format!("{s:?}").contains("abcd1234-secret"));
    }
}
