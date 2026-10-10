//! Flat execution view of a `getBlock` (encoding=json) transaction: every top-level and inner
//! instruction in execution order with its program, account keys, data and stack height.

use serde_json::Value;

pub struct Exec {
    pub program: usize,
    pub accounts: Vec<usize>,
    pub data: Vec<u8>,
    pub stack: u32,
    /// Index of the top-level instruction this belongs to.
    pub top: usize,
}

pub struct TxView {
    pub keys: Vec<String>,
    pub exec: Vec<Exec>,
    pub failed: bool,
    pub fee_payer: String,
}

impl TxView {
    pub fn parse(t: &Value) -> Option<Self> {
        let msg = &t["transaction"]["message"];
        let meta = &t["meta"];
        let mut keys: Vec<String> = msg["accountKeys"].as_array()?.iter().filter_map(|k| k.as_str().map(String::from)).collect();
        for side in ["writable", "readonly"] {
            if let Some(a) = meta["loadedAddresses"][side].as_array() {
                keys.extend(a.iter().filter_map(|k| k.as_str().map(String::from)));
            }
        }
        let ix = |v: &Value, stack: u32, top: usize| -> Option<Exec> {
            Some(Exec {
                program: v["programIdIndex"].as_u64()? as usize,
                accounts: v["accounts"].as_array()?.iter().filter_map(|a| a.as_u64().map(|x| x as usize)).collect(),
                data: bs58::decode(v["data"].as_str()?).into_vec().ok()?,
                stack: v["stackHeight"].as_u64().map(|x| x as u32).unwrap_or(stack),
                top,
            })
        };
        let mut inner_by_top: Vec<Vec<&Value>> = vec![Vec::new(); msg["instructions"].as_array().map_or(0, |a| a.len())];
        for g in meta["innerInstructions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            if let (Some(i), Some(list)) = (g["index"].as_u64(), g["instructions"].as_array()) {
                if let Some(slot) = inner_by_top.get_mut(i as usize) {
                    slot.extend(list.iter());
                }
            }
        }
        let mut exec = Vec::new();
        for (k, top) in msg["instructions"].as_array()?.iter().enumerate() {
            exec.push(ix(top, 1, k)?);
            for inner in &inner_by_top[k] {
                exec.push(ix(inner, 2, k)?);
            }
        }
        Some(Self { fee_payer: keys.first()?.clone(), keys, exec, failed: !meta["err"].is_null() })
    }

    pub fn program_id(&self, e: &Exec) -> &str {
        self.keys.get(e.program).map(String::as_str).unwrap_or("")
    }

    pub fn account(&self, e: &Exec, i: usize) -> Option<&str> {
        e.accounts.get(i).and_then(|k| self.keys.get(*k)).map(String::as_str)
    }

    /// Parent of the exec item at `i` (nearest preceding item in the same top-level group one level up).
    pub fn parent(&self, i: usize) -> Option<usize> {
        let e = &self.exec[i];
        (0..i).rev().take_while(|&j| self.exec[j].top == e.top).find(|&j| self.exec[j].stack + 1 == e.stack)
    }
}
