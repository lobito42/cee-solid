//! Applying a change set to a state it was not computed on.
//!
//! Two ways, after FigCraft's server and undo rules (see ATTRIBUTION):
//!
//! - [`Art::SpaeterGewinnt`]: the server orders sets; a set built on an older state is applied to
//!   the current one, and where both changed the same thing the later one wins. What it needs and
//!   what is gone (a parameter someone deleted, a step id someone else took) refuses the set.
//!   Clients use the same rule to show their own unconfirmed sets on top of the confirmed state.
//! - [`Art::NurUnveraendert`]: undo. A change is applied only where the design still holds the
//!   value it replaces, so what someone else changed since is left alone (and reported).
//!
//! The result is always checked by reading it as a design ([`cee_aenderung::kanon::teil_aus_baum`]).

use cee_aenderung::Aenderung;
use cee_aenderung::kanon::{zeiger_schritte, zeiger_teil};
use cee_aenderung::liste::{einfuegestelle, finde};
use serde_json::Value;

/// How to apply a set to a state it was not computed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Art {
    SpaeterGewinnt,
    NurUnveraendert,
}

/// The design's id counter: it only grows, so ids are never given out twice.
const ZAEHLER: &str = "/next_id";

fn groessen_name(v: &Value) -> Option<String> {
    v.get("name").and_then(Value::as_str).map(str::to_owned)
}

fn schritt_id(v: &Value) -> Option<u64> {
    v.get("id").and_then(Value::as_u64)
}

fn element_id(v: &Value) -> Option<String> {
    v.get("id").and_then(Value::as_str).map(str::to_owned)
}

/// Apply `aenderungen` to the JSON tree `b` (keys sorted, as [`cee_aenderung::kanon::baum`] gives).
/// `Ok` lists the targets left alone ([`Art::NurUnveraendert`] only); `Err` says why the set does
/// not fit, and `b` may then be half changed (callers work on a copy).
pub fn wende_an(b: &mut Value, aenderungen: &[Aenderung], art: Art) -> Result<Vec<String>, String> {
    let mut gelassen = Vec::new();
    for a in aenderungen {
        match eine(b, a, art)? {
            Ergebnis::Fertig => {}
            Ergebnis::Gelassen => gelassen.push(ziel(a)),
        }
    }
    Ok(gelassen)
}

enum Ergebnis {
    Fertig,
    Gelassen,
}

fn eine(b: &mut Value, a: &Aenderung, art: Art) -> Result<Ergebnis, String> {
    let undo = art == Art::NurUnveraendert;
    match a {
        Aenderung::GroesseNeu { name, nach, groesse } => neu(liste(b, "/params")?, &groessen_name, name, nach.as_ref(), groesse, undo),
        Aenderung::GroesseGesetzt { name, vorher, nachher } => {
            setze_eintrag(liste(b, "/params")?, &groessen_name, name, vorher, nachher, undo, &format!("die Größe „{name}“"))
        }
        Aenderung::GroesseGeloescht { name, vorher } => weg(liste(b, "/params")?, &groessen_name, name, vorher, undo),
        Aenderung::GroesseVerschoben { name, nach } => verschiebe(liste(b, "/params")?, &groessen_name, name, nach.as_ref(), undo),
        Aenderung::SchrittEingefuegt { id, nach, schritt } => {
            let l = liste(b, "/features")?;
            if !undo && finde(l, &schritt_id, id).is_some_and(|i| l.get(i) != Some(schritt)) {
                return Err(format!("die Schritt-Nummer {id} hat inzwischen jemand anderes vergeben"));
            }
            neu(l, &schritt_id, id, nach.as_ref(), schritt, undo)
        }
        Aenderung::SchrittGeloescht { id, vorher } => weg(liste(b, "/features")?, &schritt_id, id, vorher, undo),
        Aenderung::SchrittVerschoben { id, nach } => verschiebe(liste(b, "/features")?, &schritt_id, id, nach.as_ref(), undo),
        Aenderung::SchrittUmbenannt { id, vorher, nachher } => {
            let s = schritt(b, *id, undo)?;
            s.map_or(Ok(Ergebnis::Gelassen), |s| setze(s, "/name", Some(&Value::String(vorher.clone())), Some(Value::String(nachher.clone())), undo))
        }
        Aenderung::SchrittUnterdrueckt { id, vorher, nachher } => {
            let s = schritt(b, *id, undo)?;
            s.map_or(Ok(Ergebnis::Gelassen), |s| setze(s, "/suppressed", Some(&Value::Bool(*vorher)), Some(Value::Bool(*nachher)), undo))
        }
        Aenderung::SchrittGeaendert { id, pfad, vorher, nachher } => {
            if pfad == "/id" || pfad.is_empty() {
                return Err(format!("die Kennung von Schritt {id} lässt sich nicht ändern"));
            }
            let s = schritt(b, *id, undo)?;
            s.map_or(Ok(Ergebnis::Gelassen), |s| setze(s, pfad, vorher.as_ref(), nachher.clone(), undo))
        }
        Aenderung::ElementHinzu { schritt: s, liste: l, id, nach, element } => match elemente(b, *s, l, undo)? {
            Some(l) => neu(l, &element_id, id, nach.as_ref(), element, undo),
            None => Ok(Ergebnis::Gelassen),
        },
        Aenderung::ElementWeg { schritt: s, liste: l, id, vorher } => match elemente(b, *s, l, undo)? {
            Some(l) => weg(l, &element_id, id, vorher, undo),
            None => Ok(Ergebnis::Gelassen),
        },
        Aenderung::ElementGeaendert { schritt: s, liste: l, id, vorher, nachher } => match elemente(b, *s, l, undo)? {
            Some(l) => setze_eintrag(l, &element_id, id, vorher, nachher, undo, &format!("{id} in Schritt {s}")),
            None => Ok(Ergebnis::Gelassen),
        },
        Aenderung::ElementVerschoben { schritt: s, liste: l, id, nach } => match elemente(b, *s, l, undo)? {
            Some(l) => verschiebe(l, &element_id, id, nach.as_ref(), undo),
            None => Ok(Ergebnis::Gelassen),
        },
        Aenderung::Eigenschaft { pfad, vorher, nachher } => {
            if pfad == ZAEHLER {
                if undo {
                    return Ok(Ergebnis::Gelassen);
                }
                // The counter only grows: the larger of both wins.
                let jetzt = b.pointer(ZAEHLER).and_then(Value::as_u64);
                let neu = nachher.as_ref().and_then(Value::as_u64);
                if let (Some(j), Some(n)) = (jetzt, neu)
                    && j >= n
                {
                    return Ok(Ergebnis::Fertig);
                }
            }
            setze(b, pfad, vorher.as_ref(), nachher.clone(), undo)
        }
    }
}

/// What a change is about, as a path-like text: the same text for changes to the same thing, and
/// a prefix of it for changes to what holds it (a step and a property of that step).
pub fn ziel(a: &Aenderung) -> String {
    match a {
        Aenderung::GroesseNeu { name, .. }
        | Aenderung::GroesseGesetzt { name, .. }
        | Aenderung::GroesseGeloescht { name, .. }
        | Aenderung::GroesseVerschoben { name, .. } => format!("groesse/{}", zeiger_teil(name)),
        Aenderung::SchrittEingefuegt { id, .. } | Aenderung::SchrittGeloescht { id, .. } | Aenderung::SchrittVerschoben { id, .. } => {
            format!("schritt/{id}")
        }
        Aenderung::SchrittUmbenannt { id, .. } => format!("schritt/{id}/name"),
        Aenderung::SchrittUnterdrueckt { id, .. } => format!("schritt/{id}/suppressed"),
        Aenderung::SchrittGeaendert { id, pfad, .. } => format!("schritt/{id}{pfad}"),
        Aenderung::ElementHinzu { schritt, liste, id, .. }
        | Aenderung::ElementWeg { schritt, liste, id, .. }
        | Aenderung::ElementGeaendert { schritt, liste, id, .. }
        | Aenderung::ElementVerschoben { schritt, liste, id, .. } => format!("schritt/{schritt}{liste}/{}", zeiger_teil(id)),
        Aenderung::Eigenschaft { pfad, .. } => format!("teil{pfad}"),
    }
}

/// Whether two targets ([`ziel`]) are about the same thing or one holds the other.
pub fn ueberschneiden(a: &str, b: &str) -> bool {
    let teil_von = |kurz: &str, lang: &str| lang == kurz || lang.strip_prefix(kurz).is_some_and(|r| r.starts_with('/'));
    teil_von(a, b) || teil_von(b, a)
}

/// The step a change works inside (its items or properties, or deleting it); renaming,
/// suppressing and moving a step are not work inside it.
pub fn schritt_innen(a: &Aenderung) -> Option<u64> {
    match a {
        Aenderung::SchrittGeaendert { id, .. } | Aenderung::SchrittGeloescht { id, .. } => Some(*id),
        Aenderung::ElementHinzu { schritt, .. }
        | Aenderung::ElementWeg { schritt, .. }
        | Aenderung::ElementGeaendert { schritt, .. }
        | Aenderung::ElementVerschoben { schritt, .. } => Some(*schritt),
        _ => None,
    }
}

fn liste<'a>(b: &'a mut Value, pfad: &str) -> Result<&'a mut Vec<Value>, String> {
    b.pointer_mut(pfad).and_then(Value::as_array_mut).ok_or_else(|| format!("das Teil hat keine Liste {pfad}"))
}

/// The step `id`; `Ok(None)` when it is gone and that is no reason to refuse (undo).
fn schritt(b: &mut Value, id: u64, undo: bool) -> Result<Option<&mut Value>, String> {
    let l = liste(b, "/features")?;
    match finde(l, &schritt_id, &id) {
        Some(i) => Ok(l.get_mut(i)),
        None if undo => Ok(None),
        None => Err(format!("Schritt {id} hat inzwischen jemand anderes gelöscht")),
    }
}

fn elemente<'a>(b: &'a mut Value, s: u64, pfad: &str, undo: bool) -> Result<Option<&'a mut Vec<Value>>, String> {
    if zeiger_schritte(pfad).is_none_or(|p| p.is_empty()) {
        return Err(format!("{pfad} ist kein Pfad in einem Schritt"));
    }
    let Some(f) = schritt(b, s, undo)? else { return Ok(None) };
    match f.pointer_mut(pfad).and_then(Value::as_array_mut) {
        Some(l) => Ok(Some(l)),
        None if undo => Ok(None),
        None => Err(format!("Schritt {s} hat keine Liste {pfad}")),
    }
}

/// Where to insert after `nach`: right after it, or at the end when it is gone.
fn stelle<K: PartialEq>(l: &[Value], key: &impl Fn(&Value) -> Option<K>, nach: Option<&K>) -> usize {
    einfuegestelle(l, key, nach).unwrap_or(l.len())
}

fn neu<K: PartialEq + std::fmt::Display>(
    l: &mut Vec<Value>,
    key: &impl Fn(&Value) -> Option<K>,
    k: &K,
    nach: Option<&K>,
    v: &Value,
    undo: bool,
) -> Result<Ergebnis, String> {
    if key(v).as_ref() != Some(k) {
        return Err(format!("der Eintrag trägt nicht den Schlüssel {k}"));
    }
    if let Some(i) = finde(l, key, k) {
        // Added by both: the later one wins. Undo leaves what is there alone.
        if undo {
            return Ok(Ergebnis::Gelassen);
        }
        if let Some(slot) = l.get_mut(i) {
            *slot = v.clone();
        }
        return Ok(Ergebnis::Fertig);
    }
    let i = stelle(l, key, nach);
    l.insert(i, v.clone());
    Ok(Ergebnis::Fertig)
}

fn weg<K: PartialEq>(l: &mut Vec<Value>, key: &impl Fn(&Value) -> Option<K>, k: &K, vorher: &Value, undo: bool) -> Result<Ergebnis, String> {
    let Some(i) = finde(l, key, k) else {
        // Already gone.
        return Ok(if undo { Ergebnis::Gelassen } else { Ergebnis::Fertig });
    };
    if undo && l.get(i) != Some(vorher) {
        return Ok(Ergebnis::Gelassen);
    }
    l.remove(i);
    Ok(Ergebnis::Fertig)
}

fn setze_eintrag<K: PartialEq>(
    l: &mut [Value],
    key: &impl Fn(&Value) -> Option<K>,
    k: &K,
    vorher: &Value,
    nachher: &Value,
    undo: bool,
    was: &str,
) -> Result<Ergebnis, String> {
    if key(nachher).as_ref() != Some(k) {
        return Err(format!("{was}: der Eintrag trägt einen anderen Schlüssel"));
    }
    let Some(slot) = finde(l, key, k).and_then(|i| l.get_mut(i)) else {
        return if undo { Ok(Ergebnis::Gelassen) } else { Err(format!("{was} hat inzwischen jemand anderes gelöscht")) };
    };
    if undo && slot != vorher {
        return Ok(Ergebnis::Gelassen);
    }
    *slot = nachher.clone();
    Ok(Ergebnis::Fertig)
}

fn verschiebe<K: PartialEq + std::fmt::Display>(
    l: &mut Vec<Value>,
    key: &impl Fn(&Value) -> Option<K>,
    k: &K,
    nach: Option<&K>,
    undo: bool,
) -> Result<Ergebnis, String> {
    if nach == Some(k) {
        return Err(format!("{k} kann nicht hinter sich selbst stehen"));
    }
    let Some(i) = finde(l, key, k) else {
        return if undo { Ok(Ergebnis::Gelassen) } else { Err(format!("{k} hat inzwischen jemand anderes gelöscht")) };
    };
    let v = l.remove(i);
    let j = stelle(l, key, nach);
    l.insert(j, v);
    Ok(Ergebnis::Fertig)
}

/// Set (or remove, for `None`) the value at `pfad` below `b`. Undo only where `vorher` is there.
fn setze(b: &mut Value, pfad: &str, vorher: Option<&Value>, nachher: Option<Value>, undo: bool) -> Result<Ergebnis, String> {
    let mut schritte = zeiger_schritte(pfad).ok_or_else(|| format!("{pfad} ist kein Pfad"))?;
    let Some(letzter) = schritte.pop() else {
        return Err("das Ganze lässt sich nicht als Eigenschaft ersetzen".into());
    };
    let eltern: String = schritte.iter().map(|s| format!("/{}", zeiger_teil(s))).collect();
    let Some(obj) = b.pointer_mut(&eltern).and_then(Value::as_object_mut) else {
        return if undo { Ok(Ergebnis::Gelassen) } else { Err(format!("{pfad}: das Objekt darüber fehlt")) };
    };
    if undo && obj.get(&letzter) != vorher {
        return Ok(Ergebnis::Gelassen);
    }
    match nachher {
        Some(v) => {
            obj.insert(letzter, v);
        }
        None => {
            obj.remove(&letzter);
        }
    }
    Ok(Ergebnis::Fertig)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_overlap_on_the_same_thing_and_what_holds_it() {
        assert!(ueberschneiden("groesse/dicke", "groesse/dicke"));
        assert!(!ueberschneiden("groesse/dicke", "groesse/dicke2"));
        assert!(ueberschneiden("schritt/7", "schritt/7/name"));
        assert!(!ueberschneiden("schritt/7", "schritt/70"));
    }
}
