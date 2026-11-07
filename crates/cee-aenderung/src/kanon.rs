//! The canonical form of a design and of a change set, and the fingerprint over it.
//!
//! Canonical means: the document as SolidCraft saves it, parsed to a JSON tree, object keys sorted,
//! written compactly. Two designs with the same inputs have the same bytes whatever the order the
//! JSON library keeps keys in.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use solidcraft_doc::Document;

use crate::Fehler;

/// The JSON tree of a design, keys sorted.
pub fn baum(teil: &Document) -> Result<Value, Fehler> {
    let text = teil.to_json();
    if text.is_empty() {
        return Err(Fehler::Teil("the design could not be written as JSON".into()));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| Fehler::Teil(e.to_string()))?;
    Ok(sortiert(v))
}

/// The design a JSON tree describes (the same checks as opening a file).
pub fn teil_aus_baum(baum: &Value) -> Result<Document, Fehler> {
    let text = serde_json::to_string(baum).map_err(|e| Fehler::Teil(e.to_string()))?;
    Document::from_json(&text).map_err(|e| Fehler::Teil(e.to_string()))
}

/// `v` with every object's keys in sorted order (whatever order the map type keeps).
pub fn sortiert(v: Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut pairs: Vec<(String, Value)> = m.into_iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            let mut out = Map::new();
            for (k, v) in pairs {
                out.insert(k, sortiert(v));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.into_iter().map(sortiert).collect()),
        other => other,
    }
}

/// Compact canonical bytes of a JSON tree.
pub fn bytes(v: &Value) -> Result<Vec<u8>, Fehler> {
    serde_json::to_vec(&sortiert(v.clone())).map_err(|e| Fehler::Teil(e.to_string()))
}

/// The fingerprint of a JSON tree: `sha256:` and the hex digest of its canonical bytes.
pub fn fingerabdruck_baum(v: &Value) -> Result<String, Fehler> {
    Ok(format!("sha256:{:x}", Sha256::digest(bytes(v)?)))
}

/// The fingerprint of a design (hash over its canonical form).
pub fn fingerabdruck(teil: &Document) -> Result<String, Fehler> {
    fingerabdruck_baum(&baum(teil)?)
}

/// One step of a JSON pointer, escaped (RFC 6901).
pub fn zeiger_teil(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

/// The unescaped steps of a JSON pointer; `None` when it is not one.
pub fn zeiger_schritte(pfad: &str) -> Option<Vec<String>> {
    if pfad.is_empty() {
        return Some(Vec::new());
    }
    let rest = pfad.strip_prefix('/')?;
    Some(rest.split('/').map(|s| s.replace("~1", "/").replace("~0", "~")).collect())
}
