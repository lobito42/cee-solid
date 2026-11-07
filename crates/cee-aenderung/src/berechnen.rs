//! Computing the change set between two states of a design.
//!
//! The method follows FigCraft's collaboration diff (see ATTRIBUTION): compare the state before a
//! command with the state after it and list "property P of object O is now V", plus creations,
//! deletions and moves. Here the objects are SolidCraft's parameters (by name), timeline steps
//! (by id) and the keyed items inside a step (sketch points, curves, constraints … by id);
//! everything else in the saved design is a property at a JSON path.

use serde_json::Value;
use solidcraft_doc::Document;

use crate::Fehler;
use crate::kanon::{baum, fingerabdruck, zeiger_teil};
use crate::liste::{Schritt, schluessel, vergleiche};
use crate::satz::{Aenderung, Aenderungssatz, VERSION};

/// The key of a parameter: its name.
pub(crate) fn groessen_name(v: &Value) -> Option<String> {
    v.get("name").and_then(Value::as_str).map(str::to_owned)
}

/// The key of a timeline step: its id.
pub(crate) fn schritt_id(v: &Value) -> Option<u64> {
    v.get("id").and_then(Value::as_u64)
}

/// The key of an item in a step's list (sketch point, curve, constraint …): its id.
pub(crate) fn element_id(v: &Value) -> Option<String> {
    v.get("id").and_then(Value::as_str).map(str::to_owned)
}

/// A list's entries with their keys, when every entry has a unique one.
fn keyed<K: Ord + Clone>(l: Option<&Vec<Value>>, key: impl Fn(&Value) -> Option<K>) -> Option<Vec<(K, &Value)>> {
    let l = l?;
    Some(schluessel(l, key)?.into_iter().zip(l.iter()).collect())
}

/// The change set that turns `vorher` into `nachher`.
pub fn berechne(vorher: &Document, nachher: &Document) -> Result<Aenderungssatz, Fehler> {
    let (a, b) = (baum(vorher)?, baum(nachher)?);
    let mut aenderungen = Vec::new();
    groessen(a.get("params"), b.get("params"), &mut aenderungen);
    schritte(a.get("features"), b.get("features"), &mut aenderungen);
    if let (Some(ao), Some(bo)) = (a.as_object(), b.as_object()) {
        let mut keys: Vec<&String> = ao.keys().chain(bo.keys()).filter(|k| *k != "params" && *k != "features").collect();
        keys.sort();
        keys.dedup();
        for k in keys {
            eigenschaft(&format!("/{}", zeiger_teil(k)), ao.get(k), bo.get(k), &mut aenderungen);
        }
    } else if a != b {
        aenderungen.push(Aenderung::Eigenschaft { pfad: String::new(), vorher: Some(a.clone()), nachher: Some(b.clone()) });
    }
    Ok(Aenderungssatz { version: VERSION, ausgangsstand: fingerabdruck(vorher)?, zielstand: fingerabdruck(nachher)?, aenderungen })
}

/// Generic property changes below `pfad` of the document: objects are compared key by key,
/// anything else is replaced whole.
fn eigenschaft(pfad: &str, a: Option<&Value>, b: Option<&Value>, out: &mut Vec<Aenderung>) {
    match (a, b) {
        (Some(Value::Object(ao)), Some(Value::Object(bo))) => {
            let mut keys: Vec<&String> = ao.keys().chain(bo.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                eigenschaft(&format!("{pfad}/{}", zeiger_teil(k)), ao.get(k), bo.get(k), out);
            }
        }
        _ if a != b => out.push(Aenderung::Eigenschaft { pfad: pfad.to_owned(), vorher: a.cloned(), nachher: b.cloned() }),
        _ => {}
    }
}

fn groessen(a: Option<&Value>, b: Option<&Value>, out: &mut Vec<Aenderung>) {
    let (Some(alt), Some(neu)) = (keyed(a.and_then(Value::as_array), groessen_name), keyed(b.and_then(Value::as_array), groessen_name)) else {
        return eigenschaft("/params", a, b, out);
    };
    for s in vergleiche(&alt, &neu) {
        out.push(match s {
            Schritt::Weg(name, vorher) => Aenderung::GroesseGeloescht { name, vorher },
            Schritt::Neu(name, nach, groesse) => Aenderung::GroesseNeu { name, nach, groesse },
            Schritt::Verschoben(name, nach) => Aenderung::GroesseVerschoben { name, nach },
            Schritt::Geaendert(name, vorher, nachher) => Aenderung::GroesseGesetzt { name, vorher, nachher },
        });
    }
}

fn schritte(a: Option<&Value>, b: Option<&Value>, out: &mut Vec<Aenderung>) {
    let (Some(alt), Some(neu)) = (keyed(a.and_then(Value::as_array), schritt_id), keyed(b.and_then(Value::as_array), schritt_id)) else {
        return eigenschaft("/features", a, b, out);
    };
    for s in vergleiche(&alt, &neu) {
        match s {
            Schritt::Weg(id, vorher) => out.push(Aenderung::SchrittGeloescht { id, vorher }),
            Schritt::Neu(id, nach, schritt) => out.push(Aenderung::SchrittEingefuegt { id, nach, schritt }),
            Schritt::Verschoben(id, nach) => out.push(Aenderung::SchrittVerschoben { id, nach }),
            Schritt::Geaendert(id, vorher, nachher) => schritt_geaendert(id, &vorher, &nachher, out),
        }
    }
}

/// The changes inside one step: name, suppression, keyed item lists, other properties.
fn schritt_geaendert(id: u64, a: &Value, b: &Value, out: &mut Vec<Aenderung>) {
    let (Some(ao), Some(bo)) = (a.as_object(), b.as_object()) else {
        out.push(Aenderung::SchrittGeaendert { id, pfad: String::new(), vorher: Some(a.clone()), nachher: Some(b.clone()) });
        return;
    };
    let mut keys: Vec<&String> = ao.keys().chain(bo.keys()).collect();
    keys.sort();
    keys.dedup();
    for k in keys {
        let (x, y) = (ao.get(k), bo.get(k));
        if x == y {
            continue;
        }
        match (k.as_str(), x, y) {
            ("name", Some(Value::String(v)), Some(Value::String(n))) => {
                out.push(Aenderung::SchrittUmbenannt { id, vorher: v.clone(), nachher: n.clone() })
            }
            ("suppressed", Some(Value::Bool(v)), Some(Value::Bool(n))) => out.push(Aenderung::SchrittUnterdrueckt { id, vorher: *v, nachher: *n }),
            _ => in_schritt(id, &format!("/{}", zeiger_teil(k)), x, y, out),
        }
    }
}

/// Changes below `pfad` of a step: arrays of items with unique string ids become item changes.
fn in_schritt(id: u64, pfad: &str, a: Option<&Value>, b: Option<&Value>, out: &mut Vec<Aenderung>) {
    match (a, b) {
        (Some(Value::Object(ao)), Some(Value::Object(bo))) => {
            let mut keys: Vec<&String> = ao.keys().chain(bo.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                in_schritt(id, &format!("{pfad}/{}", zeiger_teil(k)), ao.get(k), bo.get(k), out);
            }
        }
        (Some(Value::Array(al)), Some(Value::Array(bl))) if al != bl => {
            let (Some(alt), Some(neu)) = (keyed(Some(al), element_id), keyed(Some(bl), element_id)) else {
                out.push(Aenderung::SchrittGeaendert { id, pfad: pfad.to_owned(), vorher: a.cloned(), nachher: b.cloned() });
                return;
            };
            let liste = pfad.to_owned();
            for s in vergleiche(&alt, &neu) {
                out.push(match s {
                    Schritt::Weg(eid, vorher) => Aenderung::ElementWeg { schritt: id, liste: liste.clone(), id: eid, vorher },
                    Schritt::Neu(eid, nach, element) => Aenderung::ElementHinzu { schritt: id, liste: liste.clone(), id: eid, nach, element },
                    Schritt::Verschoben(eid, nach) => Aenderung::ElementVerschoben { schritt: id, liste: liste.clone(), id: eid, nach },
                    Schritt::Geaendert(eid, vorher, nachher) => {
                        Aenderung::ElementGeaendert { schritt: id, liste: liste.clone(), id: eid, vorher, nachher }
                    }
                });
            }
        }
        _ if a != b => out.push(Aenderung::SchrittGeaendert { id, pfad: pfad.to_owned(), vorher: a.cloned(), nachher: b.cloned() }),
        _ => {}
    }
}
