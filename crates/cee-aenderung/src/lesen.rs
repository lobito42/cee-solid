//! A change set as plain German sentences, one per change ("Größe dicke: 3 mm → 4 mm").

use std::collections::BTreeMap;

use serde_json::Value;
use solidcraft_doc::Document;

use crate::kanon::baum;
use crate::satz::{Aenderung, Aenderungssatz};

/// The sentences for `satz`. Step names come from the set itself and, when given, from the design
/// it applies to (`basis`); without it a step it only refers to is named by its number.
pub fn lies(satz: &Aenderungssatz, basis: Option<&Document>) -> Vec<String> {
    let namen = schritt_namen(satz, basis);
    let schritt = |id: &u64| namen.get(id).map(|n| format!("„{n}“")).unwrap_or_else(|| format!("Nr. {id}"));
    let nach_schritt = |nach: &Option<u64>| match nach {
        Some(n) => format!("nach {}", schritt(n)),
        None => "am Anfang".to_owned(),
    };
    satz.aenderungen
        .iter()
        .map(|a| match a {
            Aenderung::GroesseNeu { name, groesse, .. } => format!("Größe {name} neu: {}", ausdruck(groesse)),
            Aenderung::GroesseGesetzt { name, vorher, nachher } => {
                let (v, n) = (ausdruck(vorher), ausdruck(nachher));
                if v != n { format!("Größe {name}: {v} → {n}") } else { format!("Größe {name}: Einheit oder Kommentar geändert") }
            }
            Aenderung::GroesseGeloescht { name, vorher } => format!("Größe {name} gelöscht (war {})", ausdruck(vorher)),
            Aenderung::GroesseVerschoben { name, nach } => match nach {
                Some(n) => format!("Größe {name} steht jetzt nach {n}"),
                None => format!("Größe {name} steht jetzt am Anfang"),
            },
            Aenderung::SchrittEingefuegt { id, nach, schritt: s } => {
                format!("Schritt {} ({}) eingefügt {}", schritt(id), art(s), nach_schritt(nach))
            }
            Aenderung::SchrittGeloescht { id, vorher } => format!("Schritt {} ({}) gelöscht", schritt(id), art(vorher)),
            Aenderung::SchrittVerschoben { id, nach } => format!("Schritt {} verschoben, steht jetzt {}", schritt(id), nach_schritt(nach)),
            Aenderung::SchrittUmbenannt { vorher, nachher, .. } => format!("Schritt „{vorher}“ umbenannt in „{nachher}“"),
            Aenderung::SchrittUnterdrueckt { id, nachher, .. } => {
                format!("Schritt {} {}", schritt(id), if *nachher { "unterdrückt" } else { "wieder aktiv" })
            }
            Aenderung::SchrittGeaendert { id, pfad, vorher, nachher } => {
                format!("Schritt {}: {} {} → {}", schritt(id), feld(pfad), kurz(vorher.as_ref()), kurz(nachher.as_ref()))
            }
            Aenderung::ElementHinzu { schritt: s, liste, id, element, .. } => {
                format!("{}: {} {id} hinzugefügt{}", ort(&schritt(s), liste), element_art(liste, element), lage(element))
            }
            Aenderung::ElementWeg { schritt: s, liste, id, vorher } => {
                format!("{}: {} {id} entfernt", ort(&schritt(s), liste), element_art(liste, vorher))
            }
            Aenderung::ElementGeaendert { schritt: s, liste, id, vorher, nachher } => {
                let wo = ort(&schritt(s), liste);
                let was = element_art(liste, nachher);
                match (vorher.get("pos"), nachher.get("pos")) {
                    (Some(p), Some(q)) if p != q => format!("{wo}: {was} {id} verschoben {} → {}", kurz(Some(p)), kurz(Some(q))),
                    _ => format!("{wo}: {was} {id} geändert"),
                }
            }
            Aenderung::ElementVerschoben { schritt: s, liste, id, nach } => {
                let wohin = nach.as_ref().map(|n| format!("nach {n}")).unwrap_or_else(|| "am Anfang".into());
                format!("{}: {id} steht jetzt {wohin}", ort(&schritt(s), liste))
            }
            Aenderung::Eigenschaft { pfad, vorher, nachher } => {
                let name = match pfad.as_str() {
                    "/next_id" => "Zähler für neue Nummern".to_owned(),
                    "/name" => "Name des Teils".to_owned(),
                    "/units" => "Einheiten".to_owned(),
                    _ => format!("Eigenschaft {}", feld(pfad)),
                };
                format!("{name}: {} → {}", kurz(vorher.as_ref()), kurz(nachher.as_ref()))
            }
        })
        .collect()
}

/// Step names by id: from the base design, then from what the set inserts, deletes or renames.
fn schritt_namen(satz: &Aenderungssatz, basis: Option<&Document>) -> BTreeMap<u64, String> {
    let mut out = BTreeMap::new();
    if let Some(features) = basis.and_then(|b| baum(b).ok()).and_then(|b| b.get("features").cloned()).and_then(|f| f.as_array().cloned()) {
        for f in features {
            if let (Some(id), Some(n)) = (f.get("id").and_then(Value::as_u64), f.get("name").and_then(Value::as_str)) {
                out.insert(id, n.to_owned());
            }
        }
    }
    for a in &satz.aenderungen {
        match a {
            Aenderung::SchrittEingefuegt { id, schritt: v, .. } | Aenderung::SchrittGeloescht { id, vorher: v } => {
                if let Some(n) = v.get("name").and_then(Value::as_str) {
                    out.entry(*id).or_insert_with(|| n.to_owned());
                }
            }
            Aenderung::SchrittUmbenannt { id, nachher, .. } => {
                out.insert(*id, nachher.clone());
            }
            _ => {}
        }
    }
    out
}

/// A parameter's expression as saved (`8 mm`).
fn ausdruck(p: &Value) -> String {
    match p.get("expr").and_then(Value::as_str) {
        Some(e) => e.to_owned(),
        None => kurz(Some(p)),
    }
}

/// What kind of step a saved feature is, in German where we know the word.
fn art(f: &Value) -> String {
    let t = f.get("type").and_then(Value::as_str).unwrap_or("?");
    let wort = match t {
        "chamfer" => "Fase",
        "extrude" => "Hochziehen",
        "fillet" => "Verrunden",
        "hole" => "Bohrung",
        "loft" => "Ausformen",
        "mirror" => "Spiegeln",
        "pattern" => "Muster",
        "revolve" => "Drehen",
        "shell" => "Aushöhlen",
        "sketch" => "Skizze",
        "sweep" => "Zug",
        other => other,
    };
    wort.to_owned()
}

/// "Skizze „Bohrbild“" for a sketch's item lists, the step and the list otherwise.
fn ort(schritt: &str, liste: &str) -> String {
    if liste.starts_with("/sketch/") { format!("Skizze {schritt}") } else { format!("Schritt {schritt} ({})", feld(liste)) }
}

/// "Punkt", "Kurve (line)", "Bedingung (horizontal)" … after the list an item sits in.
fn element_art(liste: &str, e: &Value) -> String {
    let typ = e.get("type").and_then(Value::as_str);
    let wort = match liste.rsplit('/').next().unwrap_or("") {
        "points" => "Punkt",
        "curves" => "Kurve",
        "constraints" => "Bedingung",
        "patterns" => "Muster",
        _ => "Eintrag",
    };
    match typ {
        Some(t) => format!("{wort} ({t})"),
        None => wort.to_owned(),
    }
}

/// " bei (x, y)" for an item with a position.
fn lage(e: &Value) -> String {
    e.get("pos").map(|p| format!(" bei {}", kurz(Some(p)))).unwrap_or_default()
}

/// A JSON pointer as dotted field names (`/sketch/view` → `sketch.view`).
fn feld(pfad: &str) -> String {
    crate::kanon::zeiger_schritte(pfad).map(|s| s.join(".")).unwrap_or_else(|| pfad.to_owned())
}

/// A short readable value.
fn kurz(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "(nichts)".into(),
        Some(Value::Bool(true)) => "ja".into(),
        Some(Value::Bool(false)) => "nein".into(),
        Some(Value::Number(n)) => n.as_f64().map(zahl).unwrap_or_else(|| n.to_string()),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) if a.iter().all(Value::is_number) && a.len() <= 4 => {
            format!("({})", a.iter().map(|x| kurz(Some(x))).collect::<Vec<_>>().join(", "))
        }
        Some(other) => {
            let s = other.to_string();
            if s.chars().count() > 80 { format!("{}…", s.chars().take(79).collect::<String>()) } else { s }
        }
    }
}

/// A number with at most three decimals, trailing zeros dropped.
fn zahl(x: f64) -> String {
    let s = format!("{x:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_owned() }
}
