//! The checker: build a design again with SolidCraft's own session and say, step by step, what
//! no longer works: a step that does not compute, a reference to a step that is gone, a sketch
//! that cannot be solved, a parameter whose formula fails. Under-determined sketches and
//! constraints that repeat others are noted, not counted as broken.

use std::collections::{BTreeMap, BTreeSet};

use solidcraft_doc::Document;
use solidcraft_engine::Session;

/// What the checker found.
#[derive(Clone, Debug, PartialEq)]
pub struct Bericht {
    /// Nothing broken.
    pub ok: bool,
    /// The findings, one per line; the last line sums up.
    pub zeilen: Vec<String>,
}

/// Check `teil`.
pub fn pruefe(teil: &Document) -> Bericht {
    let session = Session::new(teil.clone());
    let doc = &session.doc;
    let mut zeilen = Vec::new();
    let mut probleme = 0usize;
    let name = |id: u64| doc.feature(id).map_or_else(|| format!("Nr. {id}"), |f| format!("„{}“", f.name));
    // References to sketches that are gone or come later, by step.
    let mut bezuege: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for (i, f) in doc.features.iter().enumerate() {
        for s in f.kind.input_sketches() {
            let text = match doc.feature_index(s) {
                None => format!("die Skizze Nr. {s} gibt es nicht mehr"),
                Some(j) if j >= i => format!("die Skizze {} steht erst danach", name(s)),
                Some(_) => continue,
            };
            bezuege.entry(f.id).or_default().push(text);
        }
    }
    let mut gemeldet = BTreeSet::new();
    for r in &session.model.results {
        let bezug = bezuege.get(&r.id).map(|b| b.join("; "));
        match (&r.error, bezug) {
            (Some(e), Some(b)) => {
                probleme += 1;
                gemeldet.insert(r.id);
                zeilen.push(format!("Schritt „{}“ rechnet nicht: zerbrochener Bezug, {b} ({e})", r.name));
            }
            (None, Some(b)) => {
                probleme += 1;
                gemeldet.insert(r.id);
                zeilen.push(format!("Schritt „{}“: zerbrochener Bezug, {b}", r.name));
            }
            (Some(e), None) => {
                probleme += 1;
                zeilen.push(format!("Schritt „{}“ rechnet nicht: {e}", r.name));
            }
            (None, None) => {
                if let Some(w) = &r.warning {
                    let skizze = doc.feature(r.id).is_some_and(|f| matches!(f.kind, solidcraft_doc::FeatureKind::Sketch { .. }));
                    if !skizze {
                        zeilen.push(format!("Schritt „{}“, Hinweis: {w}", r.name));
                    }
                }
            }
        }
    }
    // Steps not computed (after the rollback marker) with a broken reference.
    for (id, b) in &bezuege {
        if !gemeldet.contains(id) {
            probleme += 1;
            zeilen.push(format!("Schritt {}: zerbrochener Bezug, {}", name(*id), b.join("; ")));
        }
    }
    for sk in &session.model.state().sketches {
        let rep = &sk.report;
        if !rep.ok() {
            probleme += 1;
            let welche = if rep.failing.is_empty() { String::new() } else { format!(" (Bedingungen {})", rep.failing.join(", ")) };
            zeilen.push(format!("Skizze „{}“ ist nicht lösbar: überbestimmt oder widersprüchlich{welche}", sk.name));
            continue;
        }
        if !rep.redundant.is_empty() {
            zeilen.push(format!("Skizze „{}“, Hinweis: überbestimmt, doppelte Bedingungen {}", sk.name, rep.redundant.join(", ")));
        }
        if rep.dof > 0 {
            zeilen.push(format!("Skizze „{}“, Hinweis: unterbestimmt, {} Freiheitsgrad(e) offen", sk.name, rep.dof));
        }
    }
    for (p, e) in &session.model.param_errors {
        probleme += 1;
        zeilen.push(format!("Größe „{p}“: {e}"));
    }
    let n = doc.features.len();
    if probleme == 0 {
        zeilen.push(format!("Prüfer: ok, alle {n} Schritte rechnen"));
    } else {
        zeilen.push(format!("Prüfer: {probleme} Problem(e) bei {n} Schritten"));
    }
    Bericht { ok: probleme == 0, zeilen }
}
