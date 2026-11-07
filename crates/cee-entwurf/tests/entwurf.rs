//! Drafts offline and merging, on the Winkelhalter (angle bracket): Anna and Bert each take a
//! copy, change different and the same things, swap folders and merge.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cee_entwurf::verlauf::Verlauf;
use cee_entwurf::zusammen::{Art, Seite};
use cee_entwurf::{Ausgang, Fehler, Staende, Wahl, neu, sichern, verzweigen, zusammenfuehren};
use serde_json::{Value, json};
use solidcraft_doc::Document;
use solidcraft_engine::Session;

fn winkelhalter() -> Document {
    solidcraft_io::read_design(include_bytes!("../../cee-aenderung/examples/winkelhalter.sc")).expect("the example opens")
}

/// A fresh, empty folder for one test.
fn ordner(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cee-entwurf-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).expect("temp dir");
    p
}

fn kopiere(von: &Path, nach: &Path) {
    std::fs::create_dir_all(nach).expect("dir");
    for e in std::fs::read_dir(von).expect("read dir").flatten() {
        let ziel = nach.join(e.file_name());
        if e.path().is_dir() {
            kopiere(&e.path(), &ziel);
        } else {
            std::fs::copy(e.path(), ziel).expect("copy");
        }
    }
}

fn befehl(d: &Document, id: &str, params: Value) -> Document {
    let mut s = Session::new(d.clone());
    s.execute(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"));
    if s.active_sketch.is_some() {
        s.execute("sketch.finish", &json!({})).expect("finish");
    }
    (*s.doc).clone()
}

fn befehle(d: &Document, liste: &[(&str, Value)]) -> Document {
    let mut s = Session::new(d.clone());
    for (id, params) in liste {
        s.execute(id, params).unwrap_or_else(|e| panic!("{id} {params}: {e}"));
    }
    (*s.doc).clone()
}

fn groesse(d: &Document, name: &str, formel: &str) -> Document {
    befehl(d, "parameters.change", json!({"name": name, "expression": formel}))
}

fn wert(d: &Document, name: &str) -> String {
    d.params.iter().find(|p| p.name == name).map(|p| p.expr.clone()).unwrap_or_default()
}

fn stand(v: &Verlauf, entwurf: &str) -> Document {
    let h = v.entwurf(entwurf).expect("draft");
    Staende::default().stand(v, &h).expect("state")
}

/// Change draft `entwurf` with `f` and save it as `autor`, at time `zeit`.
fn aendere(v: &Verlauf, entwurf: &str, autor: &str, zeit: u64, f: impl Fn(&Document) -> Document) {
    let neu = f(&stand(v, entwurf));
    sichern(v, entwurf, &neu, autor, Some(zeit)).expect("save").expect("something to save");
}

/// A history with drafts `anna` and `bert` at the Winkelhalter.
fn zwei(name: &str) -> (PathBuf, Verlauf) {
    let o = ordner(name);
    neu(&o, &winkelhalter(), "Anna", Some(1)).expect("new history");
    let v = Verlauf::oeffnen(&o).expect("opens");
    verzweigen(&v, "haupt", "anna").expect("branch");
    verzweigen(&v, "haupt", "bert").expect("branch");
    (o, v)
}

fn gemerged(a: Ausgang) -> (Document, cee_entwurf::pruefer::Bericht, cee_entwurf::zusammen::Ergebnis) {
    match a {
        Ausgang::Zusammengefuehrt { teil, bericht, ergebnis, .. } => (*teil, bericht, ergebnis),
        Ausgang::Konflikte(e) => panic!("conflicts: {:?}", e.konflikte),
        Ausgang::SchonEnthalten => panic!("nothing to merge"),
        Ausgang::Vorgespult(_) => panic!("fast-forwarded"),
    }
}

fn dateien(o: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    for d in ["saetze", "entwuerfe"] {
        for e in std::fs::read_dir(o.join(d)).expect("dir").flatten() {
            out.push((e.path(), std::fs::read(e.path()).expect("read")));
        }
    }
    out.sort();
    out
}

#[test]
fn a_thickness_and_a_width_merge_without_conflict_the_same_bytes_either_way() {
    let (o, v) = zwei("a");
    aendere(&v, "anna", "Anna", 10, |d| groesse(d, "dicke", "10 mm"));
    aendere(&v, "bert", "Bert", 20, |d| groesse(d, "breite", "80 mm"));
    // Nothing changed: nothing saved.
    assert!(sichern(&v, "anna", &stand(&v, "anna"), "Anna", None).expect("save").is_none());

    let o2 = ordner("a2");
    kopiere(&o, &o2);
    let v2 = Verlauf::oeffnen(&o2).expect("copy opens");
    let (eins, bericht, ergebnis) = gemerged(zusammenfuehren(&v, "anna", "bert", "Bert", Some(30), Wahl::Keine).expect("merge"));
    let (zwei, _, _) = gemerged(zusammenfuehren(&v2, "bert", "anna", "Anna", Some(30), Wahl::Keine).expect("merge"));
    assert!(ergebnis.konflikte.is_empty());
    assert_eq!(wert(&eins, "dicke"), "10 mm");
    assert_eq!(wert(&eins, "breite"), "80 mm");
    assert!(eins.to_json() == zwei.to_json(), "the result depends on which side is merged into which");
    assert!(bericht.ok, "{:?}", bericht.zeilen);
    // The draft merged into is at the result; the merge entry has two predecessors and the report.
    assert!(stand(&v, "bert").to_json() == eins.to_json());
    let e = v.eintrag(&v.entwurf("bert").expect("draft")).expect("entry");
    assert_eq!(e.vorgaenger.len(), 2);
    let z = e.zusammenfuehrung.expect("merge record");
    assert!(z.pruefer_ok && z.pruefer.last().is_some_and(|l| l.starts_with("Prüfer: ok")), "{:?}", z.pruefer);
}

#[test]
fn the_same_size_changed_both_ways_is_a_conflict_decided_per_side_or_by_the_later() {
    let (o, v) = zwei("b");
    aendere(&v, "anna", "Anna", 10, |d| groesse(d, "dicke", "10 mm"));
    aendere(&v, "bert", "Bert", 20, |d| groesse(d, "dicke", "12 mm"));
    let vorher = dateien(&o);
    let Ausgang::Konflikte(e) = zusammenfuehren(&v, "anna", "bert", "Bert", None, Wahl::Keine).expect("merge") else {
        panic!("expected a conflict");
    };
    assert_eq!(e.konflikte.len(), 1);
    let k = &e.konflikte[0];
    assert_eq!((k.nr, k.art, k.ein.as_str(), k.in_.as_str()), (1, Art::Wert, "10 mm", "12 mm"));
    assert!(k.was.contains("Größe „dicke“"), "{}", k.was);
    assert_eq!(dateien(&o), vorher, "nothing was written");

    // Decided for the side merged in, in a copy.
    let o2 = ordner("b2");
    kopiere(&o, &o2);
    let v2 = Verlauf::oeffnen(&o2).expect("opens");
    let (t, _, erg) =
        gemerged(zusammenfuehren(&v2, "anna", "bert", "Bert", None, Wahl::Je { je: BTreeMap::from([(1, Seite::Ein)]), sonst: None }).expect("merge"));
    assert_eq!(wert(&t, "dicke"), "10 mm");
    assert_eq!(erg.konflikte[0].gewaehlt, Some(Seite::Ein));
    let z = v2.eintrag(&v2.entwurf("bert").expect("draft")).expect("entry").zusammenfuehrung.expect("record");
    assert!(z.entscheidungen.iter().any(|d| d.contains("genommen: „anna“")), "{:?}", z.entscheidungen);

    // The later one wins: Bert saved later.
    let (t, _, _) = gemerged(zusammenfuehren(&v, "anna", "bert", "Bert", None, Wahl::SpaetererGewinnt).expect("merge"));
    assert_eq!(wert(&t, "dicke"), "12 mm");
    // And the other way round: Anna changes it again, later.
    aendere(&v, "anna", "Anna", 40, |d| groesse(d, "dicke", "9 mm"));
    aendere(&v, "bert", "Bert", 30, |d| groesse(d, "dicke", "14 mm"));
    let (t, _, _) = gemerged(zusammenfuehren(&v, "anna", "bert", "Bert", None, Wahl::SpaetererGewinnt).expect("merge"));
    assert_eq!(wert(&t, "dicke"), "9 mm");
}

#[test]
fn a_step_deleted_on_one_side_and_changed_on_the_other_is_a_conflict() {
    let (_o, v) = zwei("c");
    aendere(&v, "anna", "Anna", 10, |d| befehl(d, "timeline.delete", json!({"features": ["Hole1"]})));
    aendere(&v, "bert", "Bert", 20, |d| befehl(d, "timeline.rename", json!({"feature": "Hole1", "name": "Bohrung"})));
    let Ausgang::Konflikte(e) = zusammenfuehren(&v, "anna", "bert", "Bert", None, Wahl::Keine).expect("merge") else {
        panic!("expected a conflict");
    };
    assert_eq!(e.konflikte.len(), 1, "{:?}", e.konflikte);
    assert_eq!(e.konflikte[0].art, Art::GeloeschtBearbeitet);
    assert_eq!((e.konflikte[0].ein.as_str(), e.konflikte[0].in_.as_str()), ("gelöscht", "geändert"));
    // Taking the deletion: the step is gone.
    let (t, bericht, _) =
        gemerged(zusammenfuehren(&v, "anna", "bert", "Bert", None, Wahl::Je { je: BTreeMap::new(), sonst: Some(Seite::Ein) }).expect("merge"));
    assert!(!t.features.iter().any(|f| f.name == "Hole1" || f.name == "Bohrung"));
    assert!(bericht.ok, "{:?}", bericht.zeilen);
}

#[test]
fn a_step_inserted_on_each_side_both_are_kept_and_the_checker_is_content() {
    let (o, v) = zwei("d");
    aendere(&v, "anna", "Anna", 10, |d| befehl(d, "solid.fillet", json!({"edges": [[0, 20, 8]], "radius": 2})));
    aendere(&v, "bert", "Bert", 20, |d| befehl(d, "solid.chamfer", json!({"edges": [[0, 20, 0]], "distance": 1})));
    verzweigen(&v, "bert", "bert@vorher").expect("branch");
    let o2 = ordner("d2");
    kopiere(&o, &o2);
    let v2 = Verlauf::oeffnen(&o2).expect("opens");
    let basis = winkelhalter().features.len();
    let (t, bericht, erg) = gemerged(zusammenfuehren(&v, "anna", "bert", "Bert", None, Wahl::Keine).expect("merge"));
    let (t2, _, _) = gemerged(zusammenfuehren(&v2, "bert", "anna", "Anna", None, Wahl::Keine).expect("merge"));
    assert_eq!(t.features.len(), basis + 2, "{:?}", t.features.iter().map(|f| (&f.name, f.id)).collect::<Vec<_>>());
    let typen: Vec<String> = t
        .features
        .iter()
        .skip(basis)
        .map(|f| serde_json::to_value(&f.kind).ok().and_then(|v| v["type"].as_str().map(str::to_owned)).unwrap_or_default())
        .collect();
    assert!(typen.contains(&"fillet".to_owned()) && typen.contains(&"chamfer".to_owned()), "{typen:?}");
    let ids: std::collections::BTreeSet<u64> = t.features.iter().map(|f| f.id).collect();
    assert_eq!(ids.len(), t.features.len(), "step numbers are unique");
    assert!(t.next_id > *ids.iter().max().unwrap_or(&0));
    assert!(erg.hinweise.iter().any(|h| h.contains("bekommt die Nummer")), "{:?}", erg.hinweise);
    assert!(bericht.ok, "{:?}", bericht.zeilen);
    // The draft merged into keeps its step numbers; the step merged in gets the next free one.
    let id = |d: &Document, typ: &str| d.features.iter().find(|f| serde_json::to_value(&f.kind).is_ok_and(|v| v["type"] == typ)).map(|f| f.id);
    assert_eq!(id(&t, "chamfer"), id(&stand(&v, "bert@vorher"), "chamfer"));
    assert_eq!(id(&t2, "fillet"), Some(basis as u64 + 1));
    assert_eq!(id(&t2, "chamfer"), Some(basis as u64 + 2));
    // Either way, the same steps and parameters: only the numbers of the two new steps differ.
    let ohne_nummern = |d: &Document| {
        let mut namen: Vec<(String, String)> =
            d.features.iter().map(|f| (f.name.clone(), serde_json::to_value(&f.kind).map(|v| v["type"].to_string()).unwrap_or_default())).collect();
        namen.sort();
        (namen, d.params.iter().map(|p| (p.name.clone(), p.expr.clone())).collect::<Vec<_>>())
    };
    assert_eq!(ohne_nummern(&t), ohne_nummern(&t2));
}

#[test]
fn a_step_on_a_sketch_the_other_side_deleted_merges_and_the_checker_names_it() {
    let (_o, v) = zwei("e");
    aendere(&v, "anna", "Anna", 10, |d| befehl(d, "timeline.delete", json!({"features": ["Bohrbild"]})));
    aendere(&v, "bert", "Bert", 20, |d| befehl(d, "solid.hole", json!({"sketch": "Bohrbild", "clearance": "M4", "name": "Bohrung klein"})));
    let (t, bericht, erg) = gemerged(zusammenfuehren(&v, "anna", "bert", "Bert", None, Wahl::Keine).expect("merge"));
    assert!(erg.konflikte.is_empty(), "{:?}", erg.konflikte);
    assert!(!t.features.iter().any(|f| f.name == "Bohrbild"));
    let neu = t.features.last().map(|f| f.name.clone()).unwrap_or_default();
    assert!(!bericht.ok);
    assert!(bericht.zeilen.iter().any(|z| z.starts_with(&format!("Schritt „{neu}“")) && z.contains("zerbrochener Bezug")), "{:?}", bericht.zeilen);
    let z = v.eintrag(&v.entwurf("bert").expect("draft")).expect("entry").zusammenfuehrung.expect("record");
    assert!(!z.pruefer_ok);
    assert!(z.pruefer.iter().any(|l| l.contains("zerbrochener Bezug")));
}

#[test]
fn a_changed_file_in_the_folder_is_noticed() {
    let (o, v) = zwei("f");
    aendere(&v, "anna", "Anna", 10, |d| groesse(d, "dicke", "10 mm"));
    let h = v.entwurf("anna").expect("draft");
    let pfad = o.join("saetze").join(format!("{h}.json"));
    let text = std::fs::read_to_string(&pfad).expect("read");
    std::fs::write(&pfad, text.replace("10 mm", "11 mm")).expect("write");
    assert!(matches!(v.eintrag(&h), Err(Fehler::Veraendert(_))));
    assert!(Staende::default().stand(&v, &h).is_err());
    let befunde = v.pruefe_dateien().expect("check");
    assert!(befunde.iter().any(|b| b.contains("wurde verändert")), "{befunde:?}");
    // A pointer to an entry that is not there.
    std::fs::write(o.join("entwuerfe").join("kaputt.json"), format!("{{\"satz\": \"{}\"}}\n", "0".repeat(64))).expect("write");
    assert!(v.pruefe_dateien().expect("check").iter().any(|b| b.contains("„kaputt“")));
}

#[test]
fn two_folders_swap_with_holen_and_merge() {
    let anna = ordner("g-anna");
    neu(&anna, &winkelhalter(), "Anna", Some(1)).expect("new");
    // Bert takes a copy of the folder.
    let bert = ordner("g-bert");
    kopiere(&anna, &bert);
    let (va, vb) = (Verlauf::oeffnen(&anna).expect("opens"), Verlauf::oeffnen(&bert).expect("opens"));
    aendere(&va, "haupt", "Anna", 10, |d| groesse(d, "dicke", "10 mm"));
    aendere(&vb, "haupt", "Bert", 20, |d| groesse(d, "breite", "80 mm"));
    aendere(&vb, "haupt", "Bert", 21, |d| groesse(d, "tiefe", "45 mm"));
    let (n, gesetzt, ausgelassen) = va.hole(&vb, "bert").expect("fetch");
    assert_eq!(n, 2);
    assert_eq!(gesetzt, vec!["haupt@bert".to_owned()]);
    assert!(ausgelassen.is_empty());
    // Fetching again copies nothing new.
    assert_eq!(va.hole(&vb, "bert").expect("fetch").0, 0);
    let (t, bericht, _) = gemerged(zusammenfuehren(&va, "haupt@bert", "haupt", "Anna", None, Wahl::Keine).expect("merge"));
    assert_eq!((wert(&t, "dicke").as_str(), wert(&t, "breite").as_str(), wert(&t, "tiefe").as_str()), ("10 mm", "80 mm", "45 mm"));
    assert!(bericht.ok);
    // Bert fetches back and catches up without a merge of his own.
    vb.hole(&va, "anna").expect("fetch");
    assert!(matches!(zusammenfuehren(&vb, "haupt@anna", "haupt", "Bert", None, Wahl::Keine).expect("merge"), Ausgang::Vorgespult(_)));
    assert!(stand(&vb, "haupt").to_json() == t.to_json());
}

#[test]
fn merging_again_and_again_finds_the_latest_common_state() {
    let (_o, v) = zwei("h");
    aendere(&v, "anna", "Anna", 10, |d| groesse(d, "dicke", "10 mm"));
    aendere(&v, "anna", "Anna", 11, |d| groesse(d, "tiefe", "45 mm"));
    aendere(&v, "bert", "Bert", 12, |d| groesse(d, "breite", "80 mm"));
    gemerged(zusammenfuehren(&v, "anna", "bert", "Bert", None, Wahl::Keine).expect("merge 1"));
    // Anna catches up (her draft is behind the merge: fast-forward).
    assert!(matches!(zusammenfuehren(&v, "bert", "anna", "Anna", None, Wahl::Keine).expect("ff"), Ausgang::Vorgespult(_)));
    assert!(matches!(zusammenfuehren(&v, "anna", "bert", "Bert", None, Wahl::Keine).expect("nothing"), Ausgang::SchonEnthalten));
    // Both go on from the merge entry; Anna changes the thickness again: no conflict with her own earlier change.
    aendere(&v, "anna", "Anna", 13, |d| groesse(d, "dicke", "11 mm"));
    aendere(&v, "bert", "Bert", 14, |d| groesse(d, "steg_hoehe", "55 mm"));
    aendere(&v, "bert", "Bert", 15, |d| groesse(d, "rand", "12 mm"));
    let (t, _, erg) = gemerged(zusammenfuehren(&v, "anna", "bert", "Bert", None, Wahl::Keine).expect("merge 2"));
    assert!(erg.konflikte.is_empty(), "{:?}", erg.konflikte);
    assert_eq!(
        ["dicke", "tiefe", "breite", "steg_hoehe", "rand"].map(|n| wert(&t, n)),
        ["11 mm", "45 mm", "80 mm", "55 mm", "12 mm"].map(str::to_owned)
    );
    // Now the other direction: Bert's merge entry into Anna's newer draft.
    aendere(&v, "anna", "Anna", 16, |d| groesse(d, "breite", "70 mm"));
    let (t, _, erg) = gemerged(zusammenfuehren(&v, "bert", "anna", "Anna", None, Wahl::Keine).expect("merge 3"));
    assert!(erg.konflikte.is_empty(), "{:?}", erg.konflikte);
    assert_eq!(wert(&t, "breite"), "70 mm");
    assert_eq!(wert(&t, "rand"), "12 mm");
    assert!(matches!(zusammenfuehren(&v, "anna", "bert", "Bert", None, Wahl::Keine).expect("ff"), Ausgang::Vorgespult(_)));
    assert!(stand(&v, "bert").to_json() == t.to_json());
    // The history reads oldest first and holds every entry once.
    let reihe = v.reihenfolge(&v.entwurf("bert").expect("draft")).expect("history");
    let mut gesehen = std::collections::BTreeSet::new();
    for (h, e) in &reihe {
        assert!(e.vorgaenger.iter().all(|p| gesehen.contains(p)), "an entry before what it builds on");
        gesehen.insert(h.clone());
    }
}

#[test]
fn the_command_line_lists_conflicts_and_writes_nothing() {
    let (o, v) = zwei("cli");
    aendere(&v, "anna", "Anna", 10, |d| groesse(d, "dicke", "10 mm"));
    aendere(&v, "bert", "Bert", 20, |d| groesse(d, "dicke", "12 mm"));
    let lauf = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_cee-entwurf")).args(args).output().expect("runs");
        (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let ordner = o.to_string_lossy().into_owned();
    let (code, text) = lauf(&["zusammenfuehren", &ordner, "anna", "bert", "--autor", "Bert"]);
    assert_eq!(code, Some(2), "{text}");
    assert!(text.contains("1 Konflikt(e), nichts geschrieben") && text.contains("„anna“ 10 mm, „bert“ 12 mm"), "{text}");
    let (code, text) = lauf(&["zusammenfuehren", &ordner, "anna", "bert", "--nimm", "1=in", "--autor", "Bert"]);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("genommen: „bert“") && text.contains("Prüfer: ok"), "{text}");
    let (code, text) = lauf(&["verlauf", &ordner, "bert"]);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("„anna“ in „bert“ zusammengeführt") && text.contains("Bert") && text.contains("Anna"), "{text}");
}

#[test]
fn a_sketch_changed_on_both_sides_is_one_conflict_and_is_taken_whole() {
    let (_o, v) = zwei("skizze");
    let punkt = |id: &'static str, x: f64| {
        move |d: &Document| {
            befehle(
                d,
                &[("sketch.edit", json!({"sketch": "Bohrbild"})), ("sketch.point", json!({"point": [x, 4], "id": id})), ("sketch.finish", json!({}))],
            )
        }
    };
    aendere(&v, "anna", "Anna", 10, punkt("anna", 0.0));
    aendere(&v, "bert", "Bert", 20, punkt("bert", 10.0));
    let Ausgang::Konflikte(e) = zusammenfuehren(&v, "anna", "bert", "Bert", None, Wahl::Keine).expect("merge") else {
        panic!("expected a conflict");
    };
    assert_eq!(e.konflikte.len(), 1, "{:?}", e.konflikte);
    assert!(e.konflikte[0].was.contains("Schritt „Bohrbild“ (Skizze als Ganzes)"), "{}", e.konflikte[0].was);
    let (t, bericht, _) =
        gemerged(zusammenfuehren(&v, "anna", "bert", "Bert", None, Wahl::Je { je: BTreeMap::new(), sonst: Some(Seite::Ein) }).expect("merge"));
    let json = t.to_json();
    assert!(json.contains("\"anna\"") && !json.contains("\"bert\""), "Anna's sketch whole, nothing of Bert's");
    assert!(bericht.ok, "{:?}", bericht.zeilen);
}

#[test]
fn the_checker_names_a_sketch_that_cannot_be_solved() {
    let mut t: Value = serde_json::from_str(&winkelhalter().to_json()).expect("json");
    t["features"][0]["sketch"]["constraints"].as_array_mut().expect("constraints").push(json!({"id": "k9", "l": 0, "type": "vertical"}));
    let d = Document::from_json(&t.to_string()).expect("opens");
    let b = cee_entwurf::pruefer::pruefe(&d);
    assert!(!b.ok);
    assert!(b.zeilen.iter().any(|z| z.contains("Skizze „Grundplatte“ ist nicht lösbar") && z.contains("k9")), "{:?}", b.zeilen);
    assert!(cee_entwurf::pruefer::pruefe(&winkelhalter()).ok);
}
