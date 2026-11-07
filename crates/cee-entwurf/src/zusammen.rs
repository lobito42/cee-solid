//! Three-way merge of two states of a design from their common ancestor.
//!
//! What each side changed since the ancestor is kept; a conflict is (after FigCraft's branch
//! merge, see ATTRIBUTION):
//!
//! - the same value changed on both sides to different values (a parameter's formula, a step's
//!   property), or added on both sides with different values;
//! - a step or a parameter deleted on one side and changed on the other;
//! - the same step or parameter moved on both sides to different places.
//!
//! A sketch's content is one value: when both sides changed the same sketch, that is one conflict
//! and one side's sketch is taken whole; sketches are never mixed point by point. Whether the
//! result can still be solved is for the checker ([`crate::pruefer`]) to say.
//!
//! Two different new steps with the same number (both sides gave out the next number) are both
//! kept: the step of the draft merged in gets a new number (and new names where its own names
//! clash), with SolidCraft's own renumbering of a pasted step
//! ([`solidcraft_doc::Feature::remap_clone`]); where that fails it is a conflict. The draft merged
//! into keeps its numbers, so its history reads "step inserted", not "step changed".
//!
//! Order of lists (timeline, parameters): what neither side moved keeps the ancestor's order; what
//! one side added or moved goes right after the entry it follows on that side (or the nearest
//! earlier one still there). Several entries after the same one: a side's run stays together, and
//! the runs are ordered by key (step number, parameter name). So the result does not depend on
//! which side is merged into which, except for the numbers (and own names) of steps both sides
//! inserted: there the side merged in is renumbered.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;

use cee_aenderung::kanon::{sortiert, zeiger_teil};
use cee_aenderung::liste::{Schritt, schluessel, vergleiche};
use serde_json::{Map, Value};
use solidcraft_doc::Feature;

/// One side of a merge: the draft merged in (`Ein`) or the one merged into (`In`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Seite {
    Ein,
    In,
}

impl Seite {
    pub fn name(self) -> &'static str {
        match self {
            Seite::Ein => "ein",
            Seite::In => "in",
        }
    }

    pub fn parse(s: &str) -> Option<Seite> {
        match s {
            "ein" => Some(Seite::Ein),
            "in" => Some(Seite::In),
            _ => None,
        }
    }
}

/// What a conflict is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Art {
    /// The same value changed (or added) on both sides, differently.
    Wert,
    /// Deleted on one side, changed on the other.
    GeloeschtBearbeitet,
    /// Moved on both sides to different places.
    Reihenfolge,
    /// Two new steps with the same number that could not be renumbered.
    Nummer,
}

/// One conflict.
#[derive(Clone, Debug, PartialEq)]
pub struct Konflikt {
    /// Its number, from 1, in a fixed order (the same whichever side is merged into which).
    pub nr: usize,
    pub art: Art,
    /// What it is about, in words (`Größe „dicke“`, `Schritt „Fillet1“ › radius`).
    pub was: String,
    /// What it is about as a path-like target (`groesse/dicke`, `schritt/7/radius`).
    pub ziel: String,
    /// The value on the side merged in, in words.
    pub ein: String,
    /// The value on the side merged into, in words.
    pub in_: String,
    /// The side taken, once decided.
    pub gewaehlt: Option<Seite>,
}

impl Konflikt {
    /// One line: number, what, both values, and the side taken if decided.
    pub fn zeile(&self, ein: &str, nach: &str) -> String {
        let art = match self.art {
            Art::Wert => "",
            Art::GeloeschtBearbeitet => " (gelöscht gegen geändert)",
            Art::Reihenfolge => " (Reihenfolge)",
            Art::Nummer => " (gleiche Schritt-Nummer)",
        };
        let gewaehlt = match self.gewaehlt {
            Some(Seite::Ein) => format!(" → genommen: „{ein}“"),
            Some(Seite::In) => format!(" → genommen: „{nach}“"),
            None => String::new(),
        };
        format!("{}. {}{art}: „{ein}“ {}, „{nach}“ {}{gewaehlt}", self.nr, self.was, self.ein, self.in_)
    }
}

/// The result of a merge: the design as a JSON tree (only usable when every conflict is
/// decided), the conflicts, and notes (renumbered steps).
pub struct Ergebnis {
    pub baum: Value,
    pub konflikte: Vec<Konflikt>,
    pub hinweise: Vec<String>,
}

impl Ergebnis {
    /// Whether every conflict is decided.
    pub fn entschieden(&self) -> bool {
        self.konflikte.iter().all(|k| k.gewaehlt.is_some())
    }
}

struct Ctx<'a> {
    waehle: &'a mut dyn FnMut(&Konflikt) -> Option<Seite>,
    konflikte: Vec<Konflikt>,
    hinweise: Vec<String>,
}

impl Ctx<'_> {
    fn konflikt(&mut self, art: Art, was: String, ziel: String, ein: String, in_: String) -> Option<Seite> {
        let mut k = Konflikt { nr: self.konflikte.len() + 1, art, was, ziel, ein, in_, gewaehlt: None };
        k.gewaehlt = (self.waehle)(&k);
        let g = k.gewaehlt;
        self.konflikte.push(k);
        g
    }
}

/// A saved property name in words, where there is a plain one.
fn wort(k: &str) -> &str {
    match k {
        "expr" => "Formel",
        "comment" => "Kommentar",
        "unit" => "Einheit",
        "name" => "Name",
        "suppressed" => "unterdrückt",
        "radius" => "Radius",
        "distance" => "Abstand",
        "diameter" => "Durchmesser",
        "edges" => "Kanten",
        "extent" => "Ausdehnung",
        "operation" => "Verknüpfung",
        other => other,
    }
}

/// A value in words: text as it is, a parameter by its formula, anything else as short JSON.
pub fn in_worten(v: Option<&Value>) -> String {
    match v {
        None => "gelöscht".into(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => {
            if let Some(e) = v.get("expr").and_then(Value::as_str) {
                return e.to_owned();
            }
            let s = v.to_string();
            if s.chars().count() > 80 { format!("{}…", s.chars().take(79).collect::<String>()) } else { s }
        }
    }
}

/// Merge `ein` and `in_` (JSON trees of designs, as [`cee_aenderung::kanon::baum`] gives) from
/// their common ancestor `basis`. `waehle` decides each conflict as it is found (`None`: leave it
/// open; the result is then not to be used).
pub fn fuehre_zusammen(basis: &Value, ein: &Value, in_: &Value, waehle: &mut dyn FnMut(&Konflikt) -> Option<Seite>) -> Ergebnis {
    let mut ctx = Ctx { waehle, konflikte: Vec::new(), hinweise: Vec::new() };
    let baum = teil(&mut ctx, basis, ein, in_);
    Ergebnis { baum, konflikte: ctx.konflikte, hinweise: ctx.hinweise }
}

fn teil(ctx: &mut Ctx, b: &Value, x: &Value, y: &Value) -> Value {
    let (Some(bo), Some(xo), Some(yo)) = (b.as_object(), x.as_object(), y.as_object()) else {
        return wert(ctx, "das Teil".into(), "teil".into(), Some(b), Some(x), Some(y), false).unwrap_or(Value::Null);
    };
    // Steps with the same new number on both sides: one side's gets a new number first.
    let (x, y) = umnummerieren(ctx, b, x, y);
    let (xo, yo) = (x.as_object().unwrap_or(xo), y.as_object().unwrap_or(yo));
    let mut keys: Vec<&String> = bo.keys().chain(xo.keys()).chain(yo.keys()).collect();
    keys.sort();
    keys.dedup();
    let mut out = Map::new();
    for k in keys {
        let (bv, xv, yv) = (bo.get(k), xo.get(k), yo.get(k));
        let v = match k.as_str() {
            "params" => liste_oder_wert(
                ctx,
                "/params",
                bv,
                xv,
                yv,
                &groessen_name,
                &|n: &String| (format!("Größe „{n}“"), format!("groesse/{}", zeiger_teil(n))),
                &|ctx, was, ziel, b, x, y| wert(ctx, was, ziel, b, x, y, false),
            ),
            "features" => liste_oder_wert(
                ctx,
                "/features",
                bv,
                xv,
                yv,
                &schritt_id,
                &|id: &u64| (schritt_name(id, xv, yv, bv), format!("schritt/{id}")),
                &schritt_wert,
            ),
            // The id counter only grows: the larger wins.
            "next_id" => [bv, xv, yv].into_iter().flatten().filter_map(Value::as_u64).max().map(Value::from).or_else(|| xv.cloned()),
            _ => wert(ctx, format!("Eigenschaft {k}"), format!("teil/{}", zeiger_teil(k)), bv, xv, yv, false),
        };
        if let Some(v) = v {
            out.insert(k.clone(), v);
        }
    }
    // Renumbered steps may need a larger counter.
    let groesste = out.get("features").and_then(Value::as_array).map(|l| l.iter().filter_map(schritt_id).max().unwrap_or(0)).unwrap_or(0);
    if let Some(n) = out.get("next_id").and_then(Value::as_u64)
        && n <= groesste
    {
        out.insert("next_id".into(), Value::from(groesste + 1));
    }
    Value::Object(out)
}

fn groessen_name(v: &Value) -> Option<String> {
    v.get("name").and_then(Value::as_str).map(str::to_owned)
}

fn schritt_id(v: &Value) -> Option<u64> {
    v.get("id").and_then(Value::as_u64)
}

fn schritt_name(id: &u64, lists: Option<&Value>, more: Option<&Value>, base: Option<&Value>) -> String {
    let name = [lists, more, base]
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .flatten()
        .find(|f| schritt_id(f) == Some(*id))
        .and_then(|f| f.get("name").and_then(Value::as_str));
    match name {
        Some(n) => format!("Schritt „{n}“"),
        None => format!("Schritt Nr. {id}"),
    }
}

/// A step: its properties one by one; a sketch's content (`sketch` holding an object) whole.
fn schritt_wert(ctx: &mut Ctx, was: String, ziel: String, b: Option<&Value>, x: Option<&Value>, y: Option<&Value>) -> Option<Value> {
    if x == y {
        return x.cloned();
    }
    if x == b {
        return y.cloned();
    }
    if y == b {
        return x.cloned();
    }
    let (Some(xo), Some(yo)) = (x.and_then(Value::as_object), y.and_then(Value::as_object)) else {
        return wert(ctx, was, ziel, b, x, y, true);
    };
    let leer = Map::new();
    let bo = b.and_then(Value::as_object).unwrap_or(&leer);
    let mut keys: Vec<&String> = bo.keys().chain(xo.keys()).chain(yo.keys()).collect();
    keys.sort();
    keys.dedup();
    let mut out = Map::new();
    for k in keys {
        let (bv, xv, yv) = (bo.get(k), xo.get(k), yo.get(k));
        let skizze = k == "sketch" && [bv, xv, yv].into_iter().flatten().any(Value::is_object);
        let (w, z) = if skizze {
            (format!("{was} (Skizze als Ganzes)"), format!("{ziel}/sketch"))
        } else {
            (format!("{was} ({})", wort(k)), format!("{ziel}/{}", zeiger_teil(k)))
        };
        if let Some(v) = wert(ctx, w, z, bv, xv, yv, skizze) {
            out.insert(k.clone(), v);
        }
    }
    Some(Value::Object(out))
}

/// A value: what changed on one side is taken; objects changed on both sides are merged key by
/// key (unless `ganz`); anything else changed on both sides differently is a conflict.
fn wert(ctx: &mut Ctx, was: String, ziel: String, b: Option<&Value>, x: Option<&Value>, y: Option<&Value>, ganz: bool) -> Option<Value> {
    if x == y {
        return x.cloned();
    }
    if x == b {
        return y.cloned();
    }
    if y == b {
        return x.cloned();
    }
    if !ganz
        && let (Some(xo), Some(yo)) = (x.and_then(Value::as_object), y.and_then(Value::as_object))
        && (b.is_none() || b.is_some_and(Value::is_object))
    {
        let leer = Map::new();
        let bo = b.and_then(Value::as_object).unwrap_or(&leer);
        let mut keys: Vec<&String> = bo.keys().chain(xo.keys()).chain(yo.keys()).collect();
        keys.sort();
        keys.dedup();
        let mut out = Map::new();
        for k in keys {
            if let Some(v) = wert(ctx, format!("{was} ({})", wort(k)), format!("{ziel}/{}", zeiger_teil(k)), bo.get(k), xo.get(k), yo.get(k), false) {
                out.insert(k.clone(), v);
            }
        }
        return Some(Value::Object(out));
    }
    match ctx.konflikt(Art::Wert, was, ziel, in_worten(x), in_worten(y)) {
        Some(Seite::Ein) => x.cloned(),
        _ => y.cloned(),
    }
}

type Element<'a> = dyn Fn(&mut Ctx, String, String, Option<&Value>, Option<&Value>, Option<&Value>) -> Option<Value> + 'a;

/// A keyed list merged entry by entry, or (when an entry lacks a key or keys repeat) as a value.
#[allow(clippy::too_many_arguments)]
fn liste_oder_wert<K: Ord + Clone + Display>(
    ctx: &mut Ctx,
    pfad: &str,
    b: Option<&Value>,
    x: Option<&Value>,
    y: Option<&Value>,
    key: &dyn Fn(&Value) -> Option<K>,
    label: &dyn Fn(&K) -> (String, String),
    element: &Element,
) -> Option<Value> {
    let leer = Vec::new();
    fn arr(v: Option<&Value>) -> Option<&Vec<Value>> {
        v.and_then(Value::as_array)
    }
    let bl = if b.is_none() { Some(&leer) } else { arr(b) };
    if let (Some(bl), Some(xl), Some(yl)) = (bl, arr(x), arr(y))
        && let (Some(bk), Some(xk), Some(yk)) = (schluessel(bl, key), schluessel(xl, key), schluessel(yl, key))
    {
        let paare = |l: &'_ [Value], k: Vec<K>| -> Vec<(K, Value)> { k.into_iter().zip(l.iter().cloned()).collect() };
        return Some(Value::Array(liste(ctx, &paare(bl, bk), &paare(xl, xk), &paare(yl, yk), label, element)));
    }
    wert(ctx, format!("Liste {pfad}"), format!("teil{pfad}"), b, x, y, true)
}

/// Where an entry added or moved on one side goes: after `nach` on that side's list.
#[derive(Clone)]
struct Anker<K> {
    nach: Option<K>,
    /// That side's keys in order (to find the nearest earlier entry still there).
    seite: Seite,
}

fn liste<K: Ord + Clone + Display>(
    ctx: &mut Ctx,
    b: &[(K, Value)],
    x: &[(K, Value)],
    y: &[(K, Value)],
    label: &dyn Fn(&K) -> (String, String),
    element: &Element,
) -> Vec<Value> {
    let karte = |l: &[(K, Value)]| -> BTreeMap<K, Value> { l.iter().cloned().collect() };
    let (bm, xm, ym) = (karte(b), karte(x), karte(y));
    let alle: BTreeSet<K> = bm.keys().chain(xm.keys()).chain(ym.keys()).cloned().collect();
    let mut bleibt: BTreeMap<K, Value> = BTreeMap::new();
    for k in &alle {
        let (bv, xv, yv) = (bm.get(k), xm.get(k), ym.get(k));
        let (was, ziel) = label(k);
        let v = match (bv, xv, yv) {
            (Some(_), None, None) => None,
            (Some(bw), None, Some(yw)) | (Some(bw), Some(yw), None) => {
                let geloescht_ein = xv.is_none();
                if yw == bw {
                    None
                } else {
                    let (ein, in_) = if geloescht_ein { ("gelöscht", "geändert") } else { ("geändert", "gelöscht") };
                    let wahl = ctx.konflikt(Art::GeloeschtBearbeitet, was, ziel, ein.into(), in_.into());
                    let loeschen = match wahl {
                        Some(Seite::Ein) => geloescht_ein,
                        Some(Seite::In) => !geloescht_ein,
                        None => false,
                    };
                    if loeschen { None } else { Some(yw.clone()) }
                }
            }
            _ => element(ctx, was, ziel, bv, xv, yv),
        };
        if let Some(v) = v {
            bleibt.insert(k.clone(), v);
        }
    }

    // Order: what was moved or added, and after what.
    fn refs<K: Clone>(l: &[(K, Value)]) -> Vec<(K, &Value)> {
        l.iter().map(|(k, v)| (k.clone(), v)).collect()
    }
    let bewegung = |seite: &[(K, Value)]| {
        let mut verschoben = BTreeMap::new();
        let mut neu = BTreeMap::new();
        for s in vergleiche(&refs(b), &refs(seite)) {
            match s {
                Schritt::Verschoben(k, nach) => {
                    verschoben.insert(k, nach);
                }
                Schritt::Neu(k, nach, _) => {
                    neu.insert(k, nach);
                }
                _ => {}
            }
        }
        (verschoben, neu)
    };
    let ((vx, nx), (vy, ny)) = (bewegung(x), bewegung(y));
    let mut anker: BTreeMap<K, Anker<K>> = BTreeMap::new();
    for k in bleibt.keys() {
        let a = |nach: &Option<K>, seite| Anker { nach: nach.clone(), seite };
        let gewaehlt = if !bm.contains_key(k) {
            match (nx.get(k), ny.get(k)) {
                (Some(p), Some(q)) => Some(if q < p { a(q, Seite::In) } else { a(p, Seite::Ein) }),
                (Some(p), None) => Some(a(p, Seite::Ein)),
                (None, Some(q)) => Some(a(q, Seite::In)),
                (None, None) => None,
            }
        } else {
            match (vx.get(k), vy.get(k)) {
                (Some(p), Some(q)) if p == q => Some(a(p, Seite::Ein)),
                (Some(p), Some(q)) => {
                    let (was, ziel) = label(k);
                    let wort = |n: &Option<K>| n.as_ref().map_or("am Anfang".to_owned(), |n| format!("nach {}", label(n).0));
                    match ctx.konflikt(Art::Reihenfolge, was, ziel, wort(p), wort(q)) {
                        Some(Seite::Ein) => Some(a(p, Seite::Ein)),
                        _ => Some(a(q, Seite::In)),
                    }
                }
                (Some(p), None) => Some(a(p, Seite::Ein)),
                (None, Some(q)) => Some(a(q, Seite::In)),
                (None, None) => None,
            }
        };
        if let Some(g) = gewaehlt {
            anker.insert(k.clone(), g);
        }
    }
    // An anchor that is gone: the nearest earlier entry on that side that is still there.
    let mut kinder: BTreeMap<Option<K>, Vec<K>> = BTreeMap::new();
    for (k, a) in &anker {
        let seite = if a.seite == Seite::Ein { x } else { y };
        let mut nach = a.nach.clone();
        while let Some(n) = nach.clone() {
            if bleibt.contains_key(&n) && &n != k {
                break;
            }
            nach = seite.iter().position(|(s, _)| *s == n).and_then(|i| i.checked_sub(1)).and_then(|i| seite.get(i)).map(|(s, _)| s.clone());
        }
        kinder.entry(nach).or_default().push(k.clone());
    }
    let mut out: Vec<K> = Vec::with_capacity(bleibt.len());
    let mut gesehen: BTreeSet<K> = BTreeSet::new();
    fn ausgeben<K: Ord + Clone>(von: Option<&K>, kinder: &BTreeMap<Option<K>, Vec<K>>, out: &mut Vec<K>, gesehen: &mut BTreeSet<K>) {
        // Iterative depth first: children (sorted by key) right after their anchor.
        let mut stapel: Vec<K> = kinder.get(&von.cloned()).map(|c| c.iter().rev().cloned().collect()).unwrap_or_default();
        while let Some(k) = stapel.pop() {
            if !gesehen.insert(k.clone()) {
                continue;
            }
            out.push(k.clone());
            if let Some(c) = kinder.get(&Some(k)) {
                stapel.extend(c.iter().rev().cloned());
            }
        }
    }
    ausgeben(None, &kinder, &mut out, &mut gesehen);
    for (k, _) in b {
        if bleibt.contains_key(k) && !anker.contains_key(k) && gesehen.insert(k.clone()) {
            out.push(k.clone());
            ausgeben(Some(k), &kinder, &mut out, &mut gesehen);
        }
    }
    // Entries whose anchors point at each other (moved around each other on both sides): at the end.
    for k in anker.keys() {
        if !gesehen.contains(k) {
            gesehen.insert(k.clone());
            out.push(k.clone());
            ausgeben(Some(k), &kinder, &mut out, &mut gesehen);
        }
    }
    out.into_iter().filter_map(|k| bleibt.remove(&k)).collect()
}

fn schritte(v: &Value) -> Vec<&Value> {
    v.get("features").and_then(Value::as_array).map(|l| l.iter().collect()).unwrap_or_default()
}

/// Names a design uses for its own parameters (the parameter list and the steps' own names).
fn parameter_namen(v: &Value) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = v.get("params").and_then(Value::as_array).into_iter().flatten().filter_map(groessen_name).collect();
    for f in schritte(v) {
        out.extend(f.get("param_names").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned));
    }
    out
}

fn schritt_namen(v: &Value) -> BTreeSet<String> {
    schritte(v).into_iter().filter_map(|f| f.get("name").and_then(Value::as_str).map(str::to_owned)).collect()
}

fn koerper_namen(v: &Value) -> BTreeSet<String> {
    schritte(v)
        .into_iter()
        .flat_map(|f| f.get("body_names").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned))
        .collect()
}

/// `name` with a number at the end that none of `belegt` has: `Fillet1` → `Fillet2`, `d4` → `d5`.
fn frei(name: &str, belegt: &BTreeSet<String>) -> String {
    let stamm = name.trim_end_matches(|c: char| c.is_ascii_digit());
    let start: u64 = name.get(stamm.len()..).and_then(|n| n.parse().ok()).unwrap_or(1);
    (start.saturating_add(1)..).map(|n| format!("{stamm}{n}")).find(|n| !belegt.contains(n)).unwrap_or_else(|| format!("{name}_2"))
}

/// Two different new steps with the same number: the one of the draft merged in gets a new
/// number (and, where they clash with the other side, new names for itself, its own parameters
/// and its bodies). Returns both sides, the side merged in changed.
fn umnummerieren(ctx: &mut Ctx, b: &Value, x: &Value, y: &Value) -> (Value, Value) {
    let ids = |v: &Value| -> BTreeMap<u64, Value> { schritte(v).into_iter().filter_map(|f| schritt_id(f).map(|i| (i, f.clone()))).collect() };
    let (bi, xi, yi) = (ids(b), ids(x), ids(y));
    let gleich: Vec<u64> = xi.iter().filter(|(i, f)| !bi.contains_key(i) && yi.get(i).is_some_and(|g| g != *f)).map(|(i, _)| *i).collect();
    let (mut x, mut y) = (x.clone(), y.clone());
    if gleich.is_empty() {
        return (x, y);
    }
    let zaehler = [b, &x, &y].iter().filter_map(|v| v.get("next_id").and_then(Value::as_u64)).max().unwrap_or(1);
    let groesste = bi.keys().chain(xi.keys()).chain(yi.keys()).max().copied().unwrap_or(0);
    let mut naechste = zaehler.max(groesste + 1);
    for id in gleich {
        let (Some(fx), Some(fy)) = (xi.get(&id), yi.get(&id)) else { continue };
        let neu = naechste;
        naechste += 1;
        // `nummer_neu` changes `x` only once all of it worked.
        match nummer_neu(&mut x, &y, b, id, neu) {
            Ok(text) => {
                ctx.hinweise.push(format!("{text}: beide Seiten hatten einen neuen Schritt Nr. {id}, der hereinkommende bekommt die neue Nummer"))
            }
            Err(grund) => {
                let was = format!("Schritt Nr. {id} auf beiden Seiten neu, lässt sich nicht umnummerieren ({grund})");
                let wort = |f: &Value| f.get("name").and_then(Value::as_str).unwrap_or("?").to_owned();
                let wahl = ctx.konflikt(Art::Nummer, was, format!("schritt/{id}"), wort(fx), wort(fy));
                // The step not taken is left out on its side (and with it, what builds on it breaks).
                let weg = if wahl == Some(Seite::Ein) { &mut y } else { &mut x };
                if let Some(l) = weg.get_mut("features").and_then(Value::as_array_mut) {
                    l.retain(|f| schritt_id(f) != Some(id));
                }
            }
        }
    }
    (x, y)
}

/// Give step `alt` of `seite` the number `neu`, renaming what clashes with `andere`; what was
/// done, in words.
fn nummer_neu(seite: &mut Value, andere: &Value, basis: &Value, alt: u64, neu: u64) -> Result<String, String> {
    let liste = seite.get("features").and_then(Value::as_array).ok_or("keine Schrittliste")?;
    let roh = liste.iter().find(|f| schritt_id(f) == Some(alt)).ok_or("Schritt fehlt")?;
    let f: Feature = serde_json::from_value(roh.clone()).map_err(|e| e.to_string())?;
    let alle_param: BTreeSet<String> = [basis, andere, &*seite].iter().flat_map(|v| parameter_namen(v)).collect();
    let andere_param = parameter_namen(andere);
    let mut params = BTreeMap::new();
    let mut belegt = alle_param.clone();
    for n in &f.param_names {
        if andere_param.contains(n) {
            let frisch = frei(n, &belegt);
            belegt.insert(frisch.clone());
            params.insert(n.clone(), frisch);
        }
    }
    let mut namen = BTreeMap::new();
    let (andere_namen, basis_namen) = (schritt_namen(andere), schritt_namen(basis));
    if andere_namen.contains(&f.name) && !basis_namen.contains(&f.name) {
        let belegt: BTreeSet<String> = andere_namen.union(&schritt_namen(seite)).cloned().collect();
        namen.insert(f.name.clone(), frei(&f.name, &belegt));
    }
    let mut koerper = BTreeMap::new();
    let (andere_koerper, basis_koerper) = (koerper_namen(andere), koerper_namen(basis));
    let mut belegt_k: BTreeSet<String> = andere_koerper.union(&koerper_namen(seite)).cloned().collect();
    for n in &f.body_names {
        if andere_koerper.contains(n) && !basis_koerper.contains(n) {
            let frisch = frei(n, &belegt_k);
            belegt_k.insert(frisch.clone());
            koerper.insert(n.clone(), frisch);
        }
    }
    let ids = BTreeMap::from([(alt, neu)]);
    // Every step of this side: only this side's own steps can refer to its new step.
    let mut neue_liste = Vec::new();
    for roh in seite.get("features").and_then(Value::as_array).cloned().unwrap_or_default() {
        let mut g: Feature = serde_json::from_value(roh.clone()).map_err(|e| e.to_string())?;
        let vorher = g.clone();
        if g.id == alt {
            g.id = neu;
        }
        g.remap_clone(&ids, &namen, &koerper, &params).map_err(|e| e.to_string())?;
        if g == vorher {
            neue_liste.push(roh);
        } else {
            neue_liste.push(sortiert(serde_json::to_value(&g).map_err(|e| e.to_string())?));
        }
    }
    // The design's own parameters of that step (model parameters), renamed with it.
    if let Some(ps) = seite.get_mut("params").and_then(Value::as_array_mut) {
        for p in ps.iter_mut() {
            if let Some(n) = groessen_name(p)
                && let Some(frisch) = params.get(&n)
                && let Some(o) = p.as_object_mut()
            {
                o.insert("name".into(), Value::String(frisch.clone()));
            }
        }
    }
    if let Some(o) = seite.as_object_mut() {
        o.insert("features".into(), Value::Array(neue_liste));
    }
    let mut text = format!("„{}“ bekommt die Nummer {neu} statt {alt}", f.name);
    for (a, n) in namen.iter().chain(params.iter()).chain(koerper.iter()) {
        text.push_str(&format!(", {a} heißt jetzt {n}"));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ohne(_: &Konflikt) -> Option<Seite> {
        None
    }

    #[test]
    fn free_names_count_on() {
        let belegt: BTreeSet<String> = ["d4", "d5", "Fillet1"].iter().map(|s| s.to_string()).collect();
        assert_eq!(frei("d4", &belegt), "d6");
        assert_eq!(frei("Fillet1", &belegt), "Fillet2");
        assert_eq!(frei("Halter", &belegt), "Halter2");
    }

    #[test]
    fn list_order_is_the_same_whichever_side_is_which() {
        let l = |ks: &[&str]| Value::Array(ks.iter().map(|k| json!({"name": k, "expr": "1"})).collect());
        let b = json!({"params": l(&["a", "b", "c"])});
        let x = json!({"params": l(&["a", "x1", "x2", "b", "c"])});
        let y = json!({"params": l(&["c", "a", "y1", "b"])});
        let r1 = fuehre_zusammen(&b, &x, &y, &mut ohne);
        let r2 = fuehre_zusammen(&b, &y, &x, &mut ohne);
        assert!(r1.konflikte.is_empty());
        assert_eq!(r1.baum, r2.baum);
        let namen: Vec<&str> = r1.baum["params"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(namen, ["c", "a", "x1", "x2", "y1", "b"]);
    }

    #[test]
    fn moved_both_ways_is_a_conflict() {
        let l = |ks: &[&str]| Value::Array(ks.iter().map(|k| json!({"name": k})).collect());
        let b = json!({"params": l(&["a", "b", "c", "d"])});
        let x = json!({"params": l(&["b", "c", "d", "a"])});
        let y = json!({"params": l(&["b", "c", "a", "d"])});
        let r = fuehre_zusammen(&b, &x, &y, &mut ohne);
        assert_eq!(r.konflikte.len(), 1);
        assert_eq!(r.konflikte[0].art, Art::Reihenfolge);
    }
}
