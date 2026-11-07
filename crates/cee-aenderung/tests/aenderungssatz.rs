//! Change sets on the Winkelhalter (angle bracket): computing one between two states and applying
//! it gives the second state, a set on the wrong state or a broken set changes nothing, and the
//! same input always gives the same bytes.

use cee_aenderung::{Aenderung, Aenderungssatz, Fehler, berechne, fingerabdruck, lies, satz_als_json, satz_aus_json, wende_an};
use serde_json::{Value, json};
use solidcraft_doc::Document;
use solidcraft_engine::Session;

const BEFEHLE: &str = include_str!("winkelhalter-befehle.json");

/// The Winkelhalter built by its command list with SolidCraft's own session.
fn winkelhalter() -> Session {
    let script: Value = serde_json::from_str(BEFEHLE).expect("command list is JSON");
    let mut s = Session::default();
    s.run_script(&script).expect("the Winkelhalter builds");
    s
}

fn teil(s: &Session) -> Document {
    (*s.doc).clone()
}

fn run(s: &mut Session, id: &str, params: Value) {
    if let Err(e) = s.execute(id, &params) {
        panic!("{id} {params}: {e}");
    }
}

fn kanonisch(d: &Document) -> String {
    let v: Value = serde_json::from_str(&d.to_json()).expect("design JSON");
    serde_json::to_string(&v).expect("JSON")
}

/// `wende_an(berechne(a, b), a) == b`, and the set as text and back is the same set.
fn rundlauf(a: &Document, b: &Document) -> Aenderungssatz {
    let satz = berechne(a, b).expect("change set");
    let text = satz_als_json(&satz).expect("JSON");
    let wieder = satz_aus_json(text.as_bytes()).expect("reads back");
    assert_eq!(wieder, satz);
    let neu = wende_an(a, &wieder).expect("applies");
    assert_eq!(kanonisch(&neu), kanonisch(b));
    assert_eq!(fingerabdruck(&neu).expect("fingerprint"), fingerabdruck(b).expect("fingerprint"));
    satz
}

fn feature_named(d: &Document, name: &str) -> u64 {
    d.features.iter().find(|f| f.name == name).map(|f| f.id).unwrap_or_else(|| panic!("no step {name}"))
}

#[test]
fn changing_a_parameter_round_trips_and_reads_as_old_to_new() {
    let mut s = winkelhalter();
    let a = teil(&s);
    run(&mut s, "parameters.change", json!({"name": "dicke", "expression": "10 mm"}));
    let satz = rundlauf(&a, &teil(&s));
    let text = lies(&satz, Some(&a));
    assert!(text.iter().any(|z| z == "Größe dicke: 8 mm → 10 mm"), "{text:#?}");
}

#[test]
fn inserting_a_step_round_trips() {
    let mut s = winkelhalter();
    let a = teil(&s);
    // The plate's top edge at the front, away from the web.
    run(&mut s, "solid.fillet", json!({"edges": [[0, 20, 8]], "radius": 2}));
    let b = teil(&s);
    let satz = rundlauf(&a, &b);
    assert!(satz.aenderungen.iter().any(|c| matches!(c, Aenderung::SchrittEingefuegt { .. })), "{satz:#?}");
    let text = lies(&satz, Some(&a));
    assert!(text.iter().any(|z| z.starts_with("Schritt") && z.contains("eingefügt nach")), "{text:#?}");
}

#[test]
fn deleting_a_step_round_trips() {
    let mut s = winkelhalter();
    let a = teil(&s);
    let last = a.features.last().map(|f| f.name.clone()).expect("steps");
    run(&mut s, "timeline.delete", json!({"features": [last]}));
    let satz = rundlauf(&a, &teil(&s));
    assert!(satz.aenderungen.iter().any(|c| matches!(c, Aenderung::SchrittGeloescht { .. })), "{satz:#?}");
}

#[test]
fn moving_a_step_round_trips_as_one_move() {
    let mut s = winkelhalter();
    let a = teil(&s);
    // The hole pattern's sketch lies on XY and needs nothing before it.
    run(&mut s, "timeline.reorder", json!({"feature": "Bohrbild", "position": 0}));
    let b = teil(&s);
    assert_eq!(b.features.first().map(|f| f.name.as_str()), Some("Bohrbild"));
    let satz = rundlauf(&a, &b);
    let moves = satz.aenderungen.iter().filter(|c| matches!(c, Aenderung::SchrittVerschoben { .. })).count();
    assert_eq!(moves, 1, "{satz:#?}");
}

#[test]
fn moving_a_sketch_point_round_trips_as_an_item_change() {
    let mut s = winkelhalter();
    // The Winkelhalter's sketches are fully constrained; a free point to drag.
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Markierung"}));
    run(&mut s, "sketch.point", json!({"point": [5, 5]}));
    run(&mut s, "sketch.finish", json!({}));
    let a = teil(&s);
    let sketch = feature_named(&a, "Markierung");
    let point = a
        .feature(sketch)
        .and_then(|f| serde_json::to_value(f).ok())
        .and_then(|f| f.pointer("/sketch/points").and_then(Value::as_array).cloned())
        .and_then(|pts| pts.iter().find(|p| p.get("id") != Some(&json!("origin"))).cloned())
        .and_then(|p| p.get("id").and_then(Value::as_str).map(str::to_owned))
        .expect("the new point");
    run(&mut s, "sketch.edit", json!({"sketch": "Markierung"}));
    run(&mut s, "sketch.move_point", json!({"point": point, "to": [3, 4]}));
    run(&mut s, "sketch.finish", json!({}));
    let satz = rundlauf(&a, &teil(&s));
    assert!(satz.aenderungen.iter().any(|c| matches!(c, Aenderung::ElementGeaendert { id, .. } if *id == point)), "{satz:#?}");
    let text = lies(&satz, Some(&a));
    assert!(text.iter().any(|z| z.contains("verschoben (5, 5) → (3, 4)")), "{text:#?}");
}

#[test]
fn a_set_for_another_state_is_rejected_and_changes_nothing() {
    let mut s = winkelhalter();
    let a = teil(&s);
    run(&mut s, "parameters.change", json!({"name": "dicke", "expression": "10 mm"}));
    let satz = berechne(&a, &teil(&s)).expect("change set");
    // Someone else changed the design in the meantime.
    let mut other = Session::new(a.clone());
    run(&mut other, "parameters.change", json!({"name": "breite", "expression": "70 mm"}));
    let elsewhere = teil(&other);
    let before = kanonisch(&elsewhere);
    match wende_an(&elsewhere, &satz) {
        Err(Fehler::FalscherAusgangsstand { .. }) => {}
        other => panic!("expected a rejection, got {other:?}"),
    }
    assert_eq!(kanonisch(&elsewhere), before);
}

#[test]
fn an_invalid_set_is_rejected_whole_and_changes_nothing() {
    let mut s = winkelhalter();
    let a = teil(&s);
    run(&mut s, "parameters.change", json!({"name": "dicke", "expression": "10 mm"}));
    run(&mut s, "timeline.rename", json!({"feature": "Steg", "name": "Rippe"}));
    let mut satz = berechne(&a, &teil(&s)).expect("change set");
    assert!(satz.aenderungen.len() >= 2);
    let before = kanonisch(&a);
    // The last change claims a value the design does not have: the earlier ones must not stick.
    if let Some(Aenderung::SchrittUmbenannt { vorher, .. }) =
        satz.aenderungen.iter_mut().rev().find(|c| matches!(c, Aenderung::SchrittUmbenannt { .. }))
    {
        *vorher = "Gibt es nicht".into();
    } else {
        panic!("no rename in {satz:#?}");
    }
    match wende_an(&a, &satz) {
        Err(Fehler::Ungueltig { .. }) => {}
        other => panic!("expected a rejection, got {other:?}"),
    }
    assert_eq!(kanonisch(&a), before);
    // A set that leads elsewhere than it says is rejected too.
    let mut falsch = berechne(&a, &teil(&s)).expect("change set");
    falsch.zielstand = "sha256:0".into();
    assert!(matches!(wende_an(&a, &falsch), Err(Fehler::Zielstand { .. })));
    // Unknown fields and other versions are not read.
    let text = satz_als_json(&berechne(&a, &teil(&s)).expect("change set")).expect("JSON");
    let extra = text.replacen("\"version\"", "\"unbekannt\": 1,\n  \"version\"", 1);
    assert!(satz_aus_json(extra.as_bytes()).is_err());
    let mut v2 = berechne(&a, &teil(&s)).expect("change set");
    v2.version = 2;
    assert!(matches!(wende_an(&a, &v2), Err(Fehler::Version(2))));
}

#[test]
fn hostile_sets_never_panic() {
    let s = winkelhalter();
    let a = teil(&s);
    let fp = fingerabdruck(&a).expect("fingerprint");
    let changes = [
        json!({"art": "eigenschaft", "pfad": "", "vorher": null, "nachher": 1}),
        json!({"art": "eigenschaft", "pfad": "kein/zeiger", "vorher": null, "nachher": 1}),
        json!({"art": "eigenschaft", "pfad": "/features/0/name", "vorher": null, "nachher": "x"}),
        json!({"art": "schritt_geaendert", "id": 1, "pfad": "/id", "vorher": 1, "nachher": 2}),
        json!({"art": "schritt_verschoben", "id": 999, "nach": null}),
        json!({"art": "element_hinzu", "schritt": 1, "liste": "", "id": "p9", "nach": null, "element": {"id": "p9"}}),
        json!({"art": "element_hinzu", "schritt": 1, "liste": "/sketch/points", "id": "p9", "nach": "fehlt", "element": {"id": "p9"}}),
        json!({"art": "groesse_neu", "name": "x", "nach": null, "groesse": {"name": "y"}}),
        json!({"art": "groesse_verschoben", "name": "dicke", "nach": "dicke"}),
    ];
    for c in changes {
        let satz = json!({"version": 1, "ausgangsstand": fp, "zielstand": fp, "aenderungen": [c]});
        let satz = satz_aus_json(satz.to_string().as_bytes()).expect("well-formed");
        assert!(wende_an(&a, &satz).is_err(), "{satz:?}");
    }
    for junk in [&b"{"[..], b"[]", b"null", b"\xff\xfe", b"{\"version\": 1}"] {
        assert!(satz_aus_json(junk).is_err());
    }
}

#[test]
fn the_same_input_gives_the_same_bytes() {
    let build = || {
        let mut s = winkelhalter();
        let a = teil(&s);
        run(&mut s, "parameters.change", json!({"name": "dicke", "expression": "10 mm"}));
        run(&mut s, "timeline.reorder", json!({"feature": "Bohrbild", "position": 0}));
        (fingerabdruck(&a).expect("fingerprint"), satz_als_json(&berechne(&a, &teil(&s)).expect("change set")).expect("JSON"))
    };
    let (fa, ta) = build();
    let (fb, tb) = build();
    assert_eq!(fa, fb);
    assert_eq!(ta, tb);
    // Nothing changed: an empty set from a state to itself.
    let s = winkelhalter();
    let satz = berechne(&s.doc, &s.doc).expect("change set");
    assert!(satz.aenderungen.is_empty());
    assert_eq!(satz.ausgangsstand, satz.zielstand);
}

/// Change set size against design size for commands that rebuild a lot (printed with
/// `--nocapture`; the numbers go into the pull request).
#[test]
fn change_set_sizes() {
    let leer = Document::new("Winkelhalter");
    let s = winkelhalter();
    let a = teil(&s);
    let groesse = |satz: &Aenderungssatz| satz_als_json(satz).map(|t| t.len()).unwrap_or(0);
    let design = a.to_json().len();
    let mut zeilen = vec![format!("design (document.json, pretty): {design} bytes, {} steps", a.features.len())];
    let ganz = rundlauf(&leer, &a);
    zeilen.push(format!("whole command list from an empty design: {} bytes, {} changes", groesse(&ganz), ganz.aenderungen.len()));
    let mut cases: Vec<(&str, Vec<(&str, Value)>)> = vec![
        ("parameter change", vec![("parameters.change", json!({"name": "dicke", "expression": "10 mm"}))]),
        ("fillet inserted", vec![("solid.fillet", json!({"edges": [[0, 20, 8]], "radius": 2}))]),
        ("sketch moved to the front", vec![("timeline.reorder", json!({"feature": "Bohrbild", "position": 0}))]),
    ];
    cases.push(("first sketch deleted with what depends on it", vec![("timeline.delete", json!({"features": ["Grundplatte"]}))]));
    for (name, cmds) in cases {
        let mut t = Session::new(a.clone());
        let ok = cmds.iter().all(|(id, p)| t.execute(id, p).is_ok());
        if !ok {
            zeilen.push(format!("{name}: command refused"));
            continue;
        }
        let satz = rundlauf(&a, &teil(&t));
        zeilen.push(format!("{name}: {} bytes, {} changes", groesse(&satz), satz.aenderungen.len()));
    }
    println!("{}", zeilen.join("\n"));
}

/// Writes the example designs and change set under `examples/` (run by hand:
/// `cargo test -p cee-aenderung --test aenderungssatz -- --ignored write_examples`).
#[test]
#[ignore]
fn write_examples() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut s = winkelhalter();
    let a = teil(&s);
    // Round the plate's front top edge while it is still 8 mm thick, then make it thicker.
    run(&mut s, "solid.fillet", json!({"edges": [[0, 20, 8]], "radius": "2 mm"}));
    run(&mut s, "timeline.rename", json!({"feature": "Fillet1", "name": "Kante vorn"}));
    run(&mut s, "parameters.change", json!({"name": "dicke", "expression": "10 mm"}));
    let b = teil(&s);
    let write = |name: &str, d: &Document| {
        std::fs::write(dir.join(name), solidcraft_io::write_design_container(d).expect("package")).expect("write");
    };
    write("winkelhalter.sc", &a);
    write("winkelhalter-geaendert.sc", &b);
    let satz = rundlauf(&a, &b);
    std::fs::write(dir.join("winkelhalter-satz.json"), satz_als_json(&satz).expect("JSON")).expect("write");
}
