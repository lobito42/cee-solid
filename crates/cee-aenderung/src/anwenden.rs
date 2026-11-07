//! Applying a change set to a design: all of it or nothing.
//!
//! Every change is checked against the design (the value it replaces must be there, a new key must
//! be free, a predecessor must exist). The set is applied to a copy; the first failing change
//! rejects the whole set and the design the caller holds is untouched.

use serde_json::Value;
use solidcraft_doc::Document;

use crate::Fehler;
use crate::berechnen::{element_id, groessen_name, schritt_id};
use crate::kanon::{baum, fingerabdruck, fingerabdruck_baum, teil_aus_baum, zeiger_schritte, zeiger_teil};
use crate::liste::{einfuegestelle, finde};
use crate::satz::{Aenderung, Aenderungssatz, VERSION};

/// The most changes one set may hold.
pub const MAX_AENDERUNGEN: usize = 1_000_000;

/// `teil` with `satz` applied. The set must start from `teil`'s state (its fingerprint) and must
/// lead to the state it names; otherwise, or when any change does not fit, it is rejected whole.
pub fn wende_an(teil: &Document, satz: &Aenderungssatz) -> Result<Document, Fehler> {
    if satz.version != VERSION {
        return Err(Fehler::Version(satz.version));
    }
    if satz.aenderungen.len() > MAX_AENDERUNGEN {
        return Err(Fehler::Ungueltig { nummer: 0, grund: format!("mehr als {MAX_AENDERUNGEN} Änderungen") });
    }
    let mut b = baum(teil)?;
    let gefunden = fingerabdruck_baum(&b)?;
    if gefunden != satz.ausgangsstand {
        return Err(Fehler::FalscherAusgangsstand { erwartet: satz.ausgangsstand.clone(), gefunden });
    }
    for (i, a) in satz.aenderungen.iter().enumerate() {
        eine(&mut b, a).map_err(|grund| Fehler::Ungueltig { nummer: i + 1, grund })?;
    }
    let neu = teil_aus_baum(&b)?;
    let ziel = fingerabdruck(&neu)?;
    if ziel != satz.zielstand {
        return Err(Fehler::Zielstand { erwartet: satz.zielstand.clone(), gefunden: ziel });
    }
    Ok(neu)
}

fn eine(b: &mut Value, a: &Aenderung) -> Result<(), String> {
    match a {
        Aenderung::GroesseNeu { name, nach, groesse } => {
            pruefe_schluessel(groessen_name(groesse).as_ref() == Some(name), name)?;
            neu(liste(b, "/params")?, &groessen_name, name, nach.as_ref(), groesse.clone())
        }
        Aenderung::GroesseGesetzt { name, vorher, nachher } => {
            pruefe_schluessel(groessen_name(nachher).as_ref() == Some(name), name)?;
            setze_eintrag(liste(b, "/params")?, &groessen_name, name, vorher, nachher.clone())
        }
        Aenderung::GroesseGeloescht { name, vorher } => weg(liste(b, "/params")?, &groessen_name, name, vorher),
        Aenderung::GroesseVerschoben { name, nach } => verschiebe(liste(b, "/params")?, &groessen_name, name, nach.as_ref()),
        Aenderung::SchrittEingefuegt { id, nach, schritt } => {
            pruefe_schluessel(schritt_id(schritt) == Some(*id), id)?;
            neu(liste(b, "/features")?, &schritt_id, id, nach.as_ref(), schritt.clone())
        }
        Aenderung::SchrittGeloescht { id, vorher } => weg(liste(b, "/features")?, &schritt_id, id, vorher),
        Aenderung::SchrittVerschoben { id, nach } => verschiebe(liste(b, "/features")?, &schritt_id, id, nach.as_ref()),
        Aenderung::SchrittUmbenannt { id, vorher, nachher } => {
            setze(schritt(b, *id)?, "/name", Some(&Value::String(vorher.clone())), Some(Value::String(nachher.clone())))
        }
        Aenderung::SchrittUnterdrueckt { id, vorher, nachher } => {
            setze(schritt(b, *id)?, "/suppressed", Some(&Value::Bool(*vorher)), Some(Value::Bool(*nachher)))
        }
        Aenderung::SchrittGeaendert { id, pfad, vorher, nachher } => {
            if pfad == "/id" || pfad.is_empty() {
                return Err(format!("die Kennung von Schritt {id} lässt sich nicht ändern"));
            }
            setze(schritt(b, *id)?, pfad, vorher.as_ref(), nachher.clone())
        }
        Aenderung::ElementHinzu { schritt: s, liste: l, id, nach, element } => {
            pruefe_schluessel(element_id(element).as_ref() == Some(id), id)?;
            neu(elemente(b, *s, l)?, &element_id, id, nach.as_ref(), element.clone())
        }
        Aenderung::ElementWeg { schritt: s, liste: l, id, vorher } => weg(elemente(b, *s, l)?, &element_id, id, vorher),
        Aenderung::ElementGeaendert { schritt: s, liste: l, id, vorher, nachher } => {
            pruefe_schluessel(element_id(nachher).as_ref() == Some(id), id)?;
            setze_eintrag(elemente(b, *s, l)?, &element_id, id, vorher, nachher.clone())
        }
        Aenderung::ElementVerschoben { schritt: s, liste: l, id, nach } => verschiebe(elemente(b, *s, l)?, &element_id, id, nach.as_ref()),
        Aenderung::Eigenschaft { pfad, vorher, nachher } => {
            // Parameters and steps change through their own changes; as plain properties only when
            // the design's lists have no usable keys (what `berechne` does then).
            let erster = zeiger_schritte(pfad).and_then(|s| s.into_iter().next());
            if let Some(liste @ ("params" | "features")) = erster.as_deref() {
                let l = format!("/{liste}");
                if b.pointer(&l).is_some_and(|v| keyed_list(&l, v)) {
                    return Err(format!("{pfad} wird über Größen- und Schritt-Änderungen geändert"));
                }
            }
            setze(b, pfad, vorher.as_ref(), nachher.clone())
        }
    }
}

fn keyed_list(pfad: &str, l: &Value) -> bool {
    let Some(l) = l.as_array() else { return false };
    match pfad {
        "/params" => crate::liste::schluessel(l, groessen_name).is_some(),
        _ => crate::liste::schluessel(l, schritt_id).is_some(),
    }
}

fn pruefe_schluessel(ok: bool, k: &impl std::fmt::Display) -> Result<(), String> {
    if ok { Ok(()) } else { Err(format!("der Eintrag trägt nicht den Schlüssel {k}")) }
}

fn liste<'a>(b: &'a mut Value, pfad: &str) -> Result<&'a mut Vec<Value>, String> {
    b.pointer_mut(pfad).and_then(Value::as_array_mut).ok_or_else(|| format!("das Teil hat keine Liste {pfad}"))
}

fn schritt(b: &mut Value, id: u64) -> Result<&mut Value, String> {
    let l = liste(b, "/features")?;
    let i = finde(l, &schritt_id, &id).ok_or_else(|| format!("Schritt {id} gibt es nicht"))?;
    l.get_mut(i).ok_or_else(|| format!("Schritt {id} gibt es nicht"))
}

fn elemente<'a>(b: &'a mut Value, s: u64, pfad: &str) -> Result<&'a mut Vec<Value>, String> {
    if zeiger_schritte(pfad).is_none_or(|p| p.is_empty()) {
        return Err(format!("{pfad} ist kein Pfad in einem Schritt"));
    }
    let f = schritt(b, s)?;
    f.pointer_mut(pfad).and_then(Value::as_array_mut).ok_or_else(|| format!("Schritt {s} hat keine Liste {pfad}"))
}

fn neu<K: PartialEq + std::fmt::Display>(
    l: &mut Vec<Value>,
    key: &impl Fn(&Value) -> Option<K>,
    k: &K,
    nach: Option<&K>,
    v: Value,
) -> Result<(), String> {
    if finde(l, key, k).is_some() {
        return Err(format!("{k} gibt es schon"));
    }
    let i = einfuegestelle(l, key, nach).ok_or_else(|| format!("der Vorgänger von {k} fehlt"))?;
    l.insert(i, v);
    Ok(())
}

fn weg<K: PartialEq + std::fmt::Display>(l: &mut Vec<Value>, key: &impl Fn(&Value) -> Option<K>, k: &K, vorher: &Value) -> Result<(), String> {
    let i = finde(l, key, k).ok_or_else(|| format!("{k} gibt es nicht"))?;
    if l.get(i) != Some(vorher) {
        return Err(format!("{k} ist inzwischen anders"));
    }
    l.remove(i);
    Ok(())
}

fn setze_eintrag<K: PartialEq + std::fmt::Display>(
    l: &mut [Value],
    key: &impl Fn(&Value) -> Option<K>,
    k: &K,
    vorher: &Value,
    nachher: Value,
) -> Result<(), String> {
    let i = finde(l, key, k).ok_or_else(|| format!("{k} gibt es nicht"))?;
    let slot = l.get_mut(i).ok_or_else(|| format!("{k} gibt es nicht"))?;
    if slot != vorher {
        return Err(format!("{k} ist inzwischen anders"));
    }
    *slot = nachher;
    Ok(())
}

fn verschiebe<K: PartialEq + std::fmt::Display>(
    l: &mut Vec<Value>,
    key: &impl Fn(&Value) -> Option<K>,
    k: &K,
    nach: Option<&K>,
) -> Result<(), String> {
    if nach == Some(k) {
        return Err(format!("{k} kann nicht hinter sich selbst stehen"));
    }
    let i = finde(l, key, k).ok_or_else(|| format!("{k} gibt es nicht"))?;
    let v = l.remove(i);
    // The set is applied to a copy, so a failure after the removal leaves the caller's design alone.
    let j = einfuegestelle(l, key, nach).ok_or_else(|| format!("der Vorgänger von {k} fehlt"))?;
    l.insert(j, v);
    Ok(())
}

/// Set (or remove, for `None`) the value at `pfad` below `b`, if what is there is `vorher`.
fn setze(b: &mut Value, pfad: &str, vorher: Option<&Value>, nachher: Option<Value>) -> Result<(), String> {
    let mut schritte = zeiger_schritte(pfad).ok_or_else(|| format!("{pfad} ist kein Pfad"))?;
    let Some(letzter) = schritte.pop() else {
        return Err("das Ganze lässt sich nicht als Eigenschaft ersetzen".into());
    };
    let eltern: String = schritte.iter().map(|s| format!("/{}", zeiger_teil(s))).collect();
    let obj = b.pointer_mut(&eltern).and_then(Value::as_object_mut).ok_or_else(|| format!("{pfad}: das Objekt darüber fehlt"))?;
    if obj.get(&letzter) != vorher {
        return Err(format!("{pfad} hat nicht mehr den Wert, von dem die Änderung ausgeht"));
    }
    match nachher {
        Some(v) => {
            obj.insert(letzter, v);
        }
        None => {
            obj.remove(&letzter);
        }
    }
    Ok(())
}
