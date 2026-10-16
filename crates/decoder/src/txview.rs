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
        // Any malformed key or index rejects the whole transaction: dropping one item would shift every later index.
        let strs = |a: &Vec<Value>| a.iter().map(|k| k.as_str().map(String::from)).collect::<Option<Vec<_>>>();
        let mut keys: Vec<String> = strs(msg["accountKeys"].as_array()?)?;
        for side in ["writable", "readonly"] {
            match &meta["loadedAddresses"][side] {
                Value::Null => {}
                v => keys.extend(strs(v.as_array()?)?),
            }
        }
        let n_keys = keys.len();
        let idx = |a: &Value| a.as_u64().map(|x| x as usize).filter(|x| *x < n_keys);
        let ix = |v: &Value, inner: bool, top: usize| -> Option<Exec> {
            Some(Exec {
                program: idx(&v["programIdIndex"])?,
                accounts: v["accounts"].as_array()?.iter().map(idx).collect::<Option<Vec<_>>>()?,
                data: bs58::decode(v["data"].as_str()?).into_vec().ok()?,
                // Inner instructions need stackHeight to find their parent (CPI depth); without it: unsupported.
                stack: match v["stackHeight"].as_u64() {
                    Some(x) => x as u32,
                    None if !inner => 1,
                    None => return None,
                },
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
            exec.push(ix(top, false, k)?);
            for inner in &inner_by_top[k] {
                exec.push(ix(inner, true, k)?);
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tx() -> Value {
        json!({"meta": {"err": null, "loadedAddresses": {"writable": ["L1"], "readonly": []},
                        "innerInstructions": [{"index": 0, "instructions": [{"programIdIndex": 2, "accounts": [0, 3], "data": "3Bxs", "stackHeight": 2}]}]},
               "transaction": {"message": {"accountKeys": ["P", "A", "PROG"], "instructions": [{"programIdIndex": 2, "accounts": [0, 1], "data": "3Bxs", "stackHeight": 1}]}}})
    }

    #[test]
    fn valid_tx_with_loaded_address() {
        let t = TxView::parse(&tx()).unwrap();
        assert_eq!(t.keys.len(), 4);
        assert_eq!(t.account(&t.exec[1], 1), Some("L1"));
        assert_eq!(t.parent(1), Some(0));
    }

    #[test]
    fn malformed_key_or_index_rejects_tx() {
        let mut v = tx();
        v["transaction"]["message"]["accountKeys"][1] = json!({"pubkey": "A"});
        assert!(TxView::parse(&v).is_none(), "a non-string key must not shift later indexes");
        let mut v = tx();
        v["meta"]["innerInstructions"][0]["instructions"][0]["accounts"] = json!([0, 9]);
        assert!(TxView::parse(&v).is_none(), "out-of-range account index");
        let mut v = tx();
        v["transaction"]["message"]["instructions"][0]["programIdIndex"] = json!(7);
        assert!(TxView::parse(&v).is_none(), "out-of-range program index");
        let mut v = tx();
        v["meta"]["innerInstructions"][0]["instructions"][0].as_object_mut().unwrap().remove("stackHeight");
        assert!(TxView::parse(&v).is_none(), "inner instruction without stackHeight is unsupported");
    }
}
