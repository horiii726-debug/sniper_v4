//! Minimal Anchor (0.30+ format) IDL reader and IDL-driven borsh decoder.
//! Used to decode venue events and to name instruction accounts. Decoding stops cleanly at the end of
//! the data, so events from older program versions (fewer trailing fields) decode to their prefix.

use serde_json::{Map, Value};
use std::collections::HashMap;

/// Anchor `emit_cpi!` instruction tag (first 8 bytes of the self-CPI instruction data).
pub const EVENT_IX_TAG: [u8; 8] = [0xe4, 0x45, 0xa5, 0x2e, 0x51, 0xcb, 0x9a, 0x1d];

#[derive(Debug, Clone)]
pub struct IxDef {
    pub name: String,
    pub accounts: Vec<String>,
}

pub struct Idl {
    pub address: String,
    types: HashMap<String, Value>,
    events: HashMap<[u8; 8], String>,
    instructions: HashMap<[u8; 8], IxDef>,
}

fn disc(v: &Value) -> Option<[u8; 8]> {
    let a = v.as_array()?;
    if a.len() != 8 {
        return None;
    }
    let mut d = [0u8; 8];
    for (i, x) in a.iter().enumerate() {
        d[i] = x.as_u64()? as u8;
    }
    Some(d)
}

fn flatten_accounts(v: &Value, out: &mut Vec<String>) {
    for a in v.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        if let Some(inner) = a.get("accounts") {
            flatten_accounts(inner, out);
        } else {
            out.push(a["name"].as_str().unwrap_or("?").to_string());
        }
    }
}

impl Idl {
    pub fn from_json(text: &str) -> Result<Self, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let address = v["address"].as_str().ok_or("IDL without address")?.to_string();
        let mut types = HashMap::new();
        for t in v["types"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            types.insert(t["name"].as_str().unwrap_or("").to_string(), t["type"].clone());
        }
        let mut events = HashMap::new();
        for e in v["events"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            if let Some(d) = disc(&e["discriminator"]) {
                events.insert(d, e["name"].as_str().unwrap_or("").to_string());
            }
        }
        let mut instructions = HashMap::new();
        for i in v["instructions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            if let Some(d) = disc(&i["discriminator"]) {
                let mut accounts = Vec::new();
                flatten_accounts(&i["accounts"], &mut accounts);
                instructions.insert(d, IxDef { name: i["name"].as_str().unwrap_or("").to_string(), accounts });
            }
        }
        Ok(Self { address, types, events, instructions })
    }

    pub fn instruction(&self, data: &[u8]) -> Option<&IxDef> {
        let d: [u8; 8] = data.get(..8)?.try_into().ok()?;
        self.instructions.get(&d)
    }

    pub fn is_known_instruction(&self, data: &[u8]) -> bool {
        self.instruction(data).is_some()
    }

    /// Decode an event body: `data` = 8-byte event discriminator + borsh fields.
    pub fn decode_event(&self, data: &[u8]) -> Option<(String, Value)> {
        let d: [u8; 8] = data.get(..8)?.try_into().ok()?;
        let name = self.events.get(&d)?;
        let ty = self.types.get(name)?;
        let mut r = Reader { b: &data[8..], i: 0 };
        let v = self.decode_type_def(ty, &mut r);
        Some((name.clone(), v))
    }

    fn decode_type_def(&self, def: &Value, r: &mut Reader) -> Value {
        match def["kind"].as_str() {
            Some("struct") => {
                let mut m = Map::new();
                let fields = def["fields"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
                for (i, f) in fields.iter().enumerate() {
                    if r.done() {
                        m.insert("_truncated".into(), Value::Bool(true));
                        break;
                    }
                    let (name, ty) = match f.get("name") {
                        Some(n) => (n.as_str().unwrap_or("").to_string(), &f["type"]),
                        None => (i.to_string(), f),
                    };
                    match self.decode(ty, r) {
                        Some(v) => {
                            m.insert(name, v);
                        }
                        None => {
                            m.insert("_truncated".into(), Value::Bool(true));
                            break;
                        }
                    }
                }
                Value::Object(m)
            }
            Some("enum") => {
                let Some(tag) = r.u8() else { return Value::Null };
                let variants = def["variants"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
                let Some(var) = variants.get(tag as usize) else { return Value::Null };
                let name = var["name"].as_str().unwrap_or("").to_string();
                if let Some(fields) = var.get("fields") {
                    let inner = self.decode_type_def(&serde_json::json!({"kind": "struct", "fields": fields}), r);
                    serde_json::json!({ name: inner })
                } else {
                    Value::String(name)
                }
            }
            _ => Value::Null,
        }
    }

    fn decode(&self, ty: &Value, r: &mut Reader) -> Option<Value> {
        if let Some(s) = ty.as_str() {
            return Some(match s {
                "bool" => Value::Bool(r.u8()? != 0),
                "u8" => r.u8()?.into(),
                "i8" => (r.u8()? as i8).into(),
                "u16" => u16::from_le_bytes(r.take(2)?.try_into().ok()?).into(),
                "i16" => i16::from_le_bytes(r.take(2)?.try_into().ok()?).into(),
                "u32" => u32::from_le_bytes(r.take(4)?.try_into().ok()?).into(),
                "i32" => i32::from_le_bytes(r.take(4)?.try_into().ok()?).into(),
                "u64" => u64::from_le_bytes(r.take(8)?.try_into().ok()?).into(),
                "i64" => i64::from_le_bytes(r.take(8)?.try_into().ok()?).into(),
                "u128" => u128::from_le_bytes(r.take(16)?.try_into().ok()?).to_string().into(),
                "i128" => i128::from_le_bytes(r.take(16)?.try_into().ok()?).to_string().into(),
                "pubkey" | "publicKey" => bs58::encode(r.take(32)?).into_string().into(),
                "string" => {
                    let n = u32::from_le_bytes(r.take(4)?.try_into().ok()?) as usize;
                    String::from_utf8_lossy(r.take(n)?).into_owned().into()
                }
                "bytes" => {
                    let n = u32::from_le_bytes(r.take(4)?.try_into().ok()?) as usize;
                    Value::Array(r.take(n)?.iter().map(|b| (*b).into()).collect())
                }
                _ => return None,
            });
        }
        if let Some(inner) = ty.get("vec") {
            let n = u32::from_le_bytes(r.take(4)?.try_into().ok()?) as usize;
            let mut out = Vec::with_capacity(n.min(1024));
            for _ in 0..n {
                out.push(self.decode(inner, r)?);
            }
            return Some(Value::Array(out));
        }
        if let Some(inner) = ty.get("option") {
            return if r.u8()? == 0 { Some(Value::Null) } else { self.decode(inner, r) };
        }
        if let Some(arr) = ty.get("array").and_then(Value::as_array) {
            let n = arr.get(1)?.as_u64()? as usize;
            let mut out = Vec::with_capacity(n);
            for _ in 0..n {
                out.push(self.decode(&arr[0], r)?);
            }
            return Some(Value::Array(out));
        }
        if let Some(d) = ty.get("defined") {
            let name = d.get("name").and_then(Value::as_str).or_else(|| d.as_str())?;
            let def = self.types.get(name)?;
            return Some(self.decode_type_def(def, r));
        }
        None
    }
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn done(&self) -> bool {
        self.i >= self.b.len()
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.i..self.i + n)?;
        self.i += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
}
