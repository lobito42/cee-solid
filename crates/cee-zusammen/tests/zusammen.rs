//! Two people on the Winkelhalter (angle bracket): the server and the clients as pure logic,
//! with the messages delivered by hand so "at the same time" is exact; and once over TCP.
//!
//! Every test ends with all clients and the server on byte-identical designs.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use cee_zusammen::netz::{Ereignis, Leitung, kanal, starte};
use cee_zusammen::protokoll::{AnClient, AnServer};
use cee_zusammen::server::Verbindung;
use cee_zusammen::{Client, Server};
use serde_json::{Value, json};
use solidcraft_doc::Document;
use solidcraft_engine::Session;

const BEFEHLE: &str = include_str!("../../cee-aenderung/tests/winkelhalter-befehle.json");
const TEIL: &str = "winkelhalter";

fn winkelhalter() -> Document {
    let script: Value = serde_json::from_str(BEFEHLE).expect("command list is JSON");
    let mut s = Session::default();
    s.run_script(&script).expect("the Winkelhalter builds");
    (*s.doc).clone()
}

/// A server and clients; messages wait in queues until delivered.
struct Sim {
    server: Server,
    clients: Vec<Client>,
    /// The connection of each client (`None`: offline).
    verbindung: Vec<Option<Verbindung>>,
    an_server: VecDeque<(Verbindung, AnServer)>,
    an_client: Vec<VecDeque<AnClient>>,
    naechste: Verbindung,
    meldungen: Vec<Vec<String>>,
}

impl Sim {
    fn neu(namen: &[&str]) -> Self {
        let server = Server::neu(vec![(TEIL.to_owned(), winkelhalter())]).expect("server");
        let mut s = Self {
            server,
            clients: namen.iter().map(|n| Client::neu(n, TEIL)).collect(),
            verbindung: vec![None; namen.len()],
            an_server: VecDeque::new(),
            an_client: vec![VecDeque::new(); namen.len()],
            naechste: 1,
            meldungen: vec![Vec::new(); namen.len()],
        };
        for i in 0..namen.len() {
            s.verbinde(i);
        }
        s.ruhe();
        s
    }

    fn verbinde(&mut self, i: usize) {
        let v = self.naechste;
        self.naechste += 1;
        self.verbindung[i] = Some(v);
        self.an_server.push_back((v, self.clients[i].hallo()));
    }

    fn trenne(&mut self, i: usize) {
        if let Some(v) = self.verbindung[i].take() {
            // What was on the way to the server is lost with the connection.
            self.an_server.retain(|(w, _)| *w != v);
            let out = self.server.getrennt(v);
            self.verteile(out);
        }
        self.an_client[i].clear();
        self.clients[i].getrennt();
    }

    fn verteile(&mut self, out: Vec<(Verbindung, AnClient)>) {
        for (v, n) in out {
            if let Some(i) = self.verbindung.iter().position(|w| *w == Some(v)) {
                self.an_client[i].push_back(n);
            }
        }
    }

    /// Client `i`'s outgoing messages into the server's queue (not yet processed).
    fn sende(&mut self, i: usize) {
        let Some(v) = self.verbindung[i] else { return };
        while let Some(n) = self.clients[i].ausgehend() {
            self.an_server.push_back((v, n));
        }
    }

    fn schicke(&mut self, i: usize, n: AnServer) {
        if let Some(v) = self.verbindung[i] {
            self.an_server.push_back((v, n));
        }
    }

    /// Deliver everything until nothing moves.
    fn ruhe(&mut self) {
        loop {
            let mut bewegt = false;
            while let Some((v, n)) = self.an_server.pop_front() {
                let out = self.server.empfange(v, n);
                self.verteile(out);
                bewegt = true;
            }
            for i in 0..self.clients.len() {
                while let Some(n) = self.an_client[i].pop_front() {
                    self.clients[i].empfange(n);
                    bewegt = true;
                }
                let m = self.clients[i].meldungen();
                self.meldungen[i].extend(m);
                let vorher = self.an_server.len();
                self.sende(i);
                bewegt |= self.an_server.len() != vorher;
            }
            if !bewegt {
                return;
            }
        }
    }

    fn befehl(&mut self, i: usize, id: &str, params: Value) -> Vec<String> {
        match self.clients[i].befehl(id, &params) {
            Ok(t) => t,
            Err(e) => panic!("{id} {params}: {e}"),
        }
    }

    fn groesse(&mut self, i: usize, name: &str, formel: &str) {
        self.befehl(i, "parameters.change", json!({"name": name, "expression": formel}));
    }

    fn wert(&self, i: usize, name: &str) -> String {
        self.clients[i].teil().params.iter().find(|p| p.name == name).map(|p| p.expr.clone()).unwrap_or_default()
    }

    fn hat_meldung(&self, i: usize, teil: &str) -> bool {
        self.meldungen[i].iter().any(|m| m.contains(teil))
    }

    /// Everyone on the server's state, byte for byte, and nothing left unconfirmed.
    fn alle_gleich(&self) {
        let (server, seq) = self.server.stand(TEIL).expect("part");
        let erwartet = server.to_json();
        for (i, c) in self.clients.iter().enumerate() {
            assert_eq!(c.offen(), 0, "client {i} still has unconfirmed sets");
            assert_eq!(c.bestaetigt().1, seq, "client {i}");
            assert!(c.teil().to_json() == erwartet, "client {i} differs from the server");
            assert!(c.bestaetigt().0.to_json() == erwartet, "client {i}'s confirmed state differs");
        }
    }
}

fn schritt_id(d: &Document, name: &str) -> u64 {
    d.features.iter().find(|f| f.name == name).map(|f| f.id).unwrap_or_else(|| panic!("no step {name}"))
}

#[test]
fn two_people_change_different_parameters_at_once_and_both_see_both() {
    let mut s = Sim::neu(&["Anna", "Bert"]);
    s.groesse(0, "dicke", "10 mm");
    s.groesse(1, "breite", "80 mm");
    // Both sets leave before either answer comes back: both build on the same state.
    s.sende(0);
    s.sende(1);
    s.ruhe();
    for i in 0..2 {
        assert_eq!(s.wert(i, "dicke"), "10 mm");
        assert_eq!(s.wert(i, "breite"), "80 mm");
    }
    assert!(s.hat_meldung(0, "Bert: Größe breite"), "{:?}", s.meldungen[0]);
    assert!(s.hat_meldung(1, "Anna: Größe dicke"), "{:?}", s.meldungen[1]);
    assert!(!s.hat_meldung(0, "Hinweis"), "{:?}", s.meldungen[0]);
    s.alle_gleich();
}

#[test]
fn the_same_parameter_at_once_the_later_wins_and_the_other_gets_a_notice() {
    let mut s = Sim::neu(&["Anna", "Bert"]);
    s.groesse(0, "dicke", "10 mm");
    s.groesse(1, "dicke", "12 mm");
    s.sende(0);
    s.sende(1);
    s.ruhe();
    for i in 0..2 {
        assert_eq!(s.wert(i, "dicke"), "12 mm", "client {i}");
    }
    assert!(s.hat_meldung(0, "Hinweis: Bert hat gleichzeitig die Größe „dicke“ (jetzt 12 mm) geändert"), "{:?}", s.meldungen[0]);
    assert!(!s.hat_meldung(1, "Hinweis"), "{:?}", s.meldungen[1]);
    s.alle_gleich();
}

#[test]
fn a_step_inserted_while_someone_changes_a_parameter_reaches_both() {
    let mut s = Sim::neu(&["Anna", "Bert"]);
    let schritte = s.clients[0].teil().features.len();
    s.befehl(0, "solid.fillet", json!({"edges": [[0, 20, 8]], "radius": 2}));
    s.groesse(1, "dicke", "10 mm");
    s.sende(0);
    s.sende(1);
    s.ruhe();
    for i in 0..2 {
        assert_eq!(s.clients[i].teil().features.len(), schritte + 1, "client {i}");
        assert_eq!(s.wert(i, "dicke"), "10 mm");
    }
    s.alle_gleich();
}

#[test]
fn a_sketch_is_locked_while_someone_works_in_it() {
    let mut s = Sim::neu(&["Anna", "Bert"]);
    let bohrbild = schritt_id(s.clients[0].teil(), "Bohrbild");
    let n = s.clients[0].in_skizze(Some(bohrbild));
    s.schicke(0, n);
    s.ruhe();
    assert_eq!(s.clients[0].skizze(), Some(bohrbild));
    assert!(s.hat_meldung(1, "Anna arbeitet in der Skizze „Bohrbild“"), "{:?}", s.meldungen[1]);

    // Bert cannot lock it too.
    let n = s.clients[1].in_skizze(Some(bohrbild));
    s.schicke(1, n);
    s.ruhe();
    assert_eq!(s.clients[1].skizze(), None);
    assert!(s.hat_meldung(1, "In der Skizze „Bohrbild“ arbeitet gerade Anna"), "{:?}", s.meldungen[1]);

    // Bert works in it anyway (without the lock): refused, and he is back on the server's state.
    s.befehl(1, "sketch.edit", json!({"sketch": "Bohrbild"}));
    s.befehl(1, "sketch.point", json!({"point": [0, 0], "id": "bert"}));
    s.befehl(1, "sketch.finish", json!({}));
    s.ruhe();
    assert!(s.hat_meldung(1, "Abgelehnt: in der Skizze „Bohrbild“ arbeitet gerade Anna"), "{:?}", s.meldungen[1]);

    // Parameters and other steps go on at the same time.
    s.groesse(1, "breite", "80 mm");
    // Anna adds a point in her sketch.
    s.befehl(0, "sketch.point", json!({"point": [0, 5], "id": "anna"}));
    let n = s.clients[0].in_skizze(None);
    s.schicke(0, n);
    s.ruhe();
    for i in 0..2 {
        assert_eq!(s.wert(i, "breite"), "80 mm");
        let json = s.clients[i].teil().to_json();
        assert!(json.contains("\"anna\""), "client {i} lacks Anna's point");
        assert!(!json.contains("\"bert\""), "client {i} has Bert's refused point");
    }
    // Free again: now Bert can.
    let n = s.clients[1].in_skizze(Some(bohrbild));
    s.schicke(1, n);
    s.ruhe();
    assert_eq!(s.clients[1].skizze(), Some(bohrbild));
    let n = s.clients[1].in_skizze(None);
    s.schicke(1, n);
    s.ruhe();
    s.alle_gleich();
}

#[test]
fn offline_changes_go_out_after_reconnecting() {
    let mut s = Sim::neu(&["Anna", "Bert"]);
    let schritte = s.clients[0].teil().features.len();
    s.trenne(1);
    assert!(!s.clients[1].ist_verbunden());
    // Bert works offline, Anna online.
    s.groesse(1, "breite", "80 mm");
    s.befehl(1, "solid.fillet", json!({"edges": [[0, 20, 8]], "radius": 2}));
    s.groesse(0, "dicke", "10 mm");
    s.groesse(0, "tiefe", "45 mm");
    s.ruhe();
    assert_eq!(s.wert(1, "dicke"), "8 mm", "Bert has not seen Anna's change yet");
    assert_eq!(s.clients[1].offen(), 2);
    s.verbinde(1);
    s.ruhe();
    assert!(s.hat_meldung(1, "verpasste Änderung"), "{:?}", s.meldungen[1]);
    for i in 0..2 {
        assert_eq!(s.wert(i, "breite"), "80 mm", "client {i}");
        assert_eq!(s.wert(i, "dicke"), "10 mm", "client {i}");
        assert_eq!(s.wert(i, "tiefe"), "45 mm", "client {i}");
        assert_eq!(s.clients[i].teil().features.len(), schritte + 1, "client {i} has Bert's offline fillet");
    }
    s.alle_gleich();
}

#[test]
fn a_set_on_the_way_when_the_connection_drops_is_sent_again() {
    let mut s = Sim::neu(&["Anna", "Bert"]);
    s.groesse(1, "breite", "80 mm");
    s.sende(1);
    // The connection drops before the server read it.
    s.trenne(1);
    s.verbinde(1);
    s.ruhe();
    assert_eq!(s.wert(0, "breite"), "80 mm");
    s.alle_gleich();
}

#[test]
fn a_set_that_does_not_fit_is_refused_and_the_sender_is_put_right() {
    let mut s = Sim::neu(&["Anna", "Bert"]);
    // Anna deletes a step while Bert renames it.
    let name = s.clients[0].teil().features.last().map(|f| f.name.clone()).expect("steps");
    s.befehl(0, "timeline.delete", json!({"features": [name.clone()]}));
    s.befehl(1, "timeline.rename", json!({"feature": name.clone(), "name": "Umbenannt"}));
    s.sende(0);
    s.sende(1);
    s.ruhe();
    assert!(s.hat_meldung(1, "Abgelehnt"), "{:?}", s.meldungen[1]);
    for i in 0..2 {
        assert!(!s.clients[i].teil().features.iter().any(|f| f.name == name || f.name == "Umbenannt"), "client {i}");
    }
    s.alle_gleich();

    // A set that starts from a state the server never had: refused whole.
    let (stand, seq) = s.clients[1].bestaetigt();
    let mut anders = stand.clone();
    anders.params.retain(|p| p.name != "rand");
    let mut satz = cee_aenderung::berechne(stand, &anders).expect("set");
    satz.ausgangsstand = "sha256:00".into();
    let vorher = s.server.stand(TEIL).map(|(d, q)| (d.to_json(), q)).expect("part");
    s.schicke(1, AnServer::Aenderungen { basis: seq, gesehen: seq, nr: 1000, satz });
    s.ruhe();
    assert!(s.hat_meldung(1, "Abgelehnt: der Satz passt nicht"), "{:?}", s.meldungen[1]);
    assert_eq!(s.server.stand(TEIL).map(|(d, q)| (d.to_json(), q)).expect("part"), vorher, "the server changed nothing");
    s.alle_gleich();
}

#[test]
fn undo_takes_back_only_ones_own_steps() {
    let mut s = Sim::neu(&["Anna", "Bert"]);
    s.groesse(0, "dicke", "10 mm");
    s.ruhe();
    s.groesse(1, "breite", "80 mm");
    s.ruhe();
    // Anna's undo: her thickness goes back, Bert's width stays.
    s.clients[0].rueckgaengig().expect("undo");
    s.ruhe();
    for i in 0..2 {
        assert_eq!(s.wert(i, "dicke"), "8 mm", "client {i}");
        assert_eq!(s.wert(i, "breite"), "80 mm", "client {i}");
    }
    // Redo brings it back.
    s.clients[0].wiederholen().expect("redo");
    s.ruhe();
    assert_eq!(s.wert(1, "dicke"), "10 mm");
    s.alle_gleich();
}

#[test]
fn undo_leaves_alone_what_someone_else_changed_since() {
    let mut s = Sim::neu(&["Anna", "Bert"]);
    s.groesse(0, "dicke", "10 mm");
    s.ruhe();
    s.groesse(1, "dicke", "12 mm");
    s.ruhe();
    assert!(s.clients[0].rueckgaengig().is_err(), "nothing of Anna's is left to undo");
    s.ruhe();
    assert!(s.hat_meldung(0, "übersprungen"), "{:?}", s.meldungen[0]);
    for i in 0..2 {
        assert_eq!(s.wert(i, "dicke"), "12 mm", "client {i}");
    }
    s.alle_gleich();
}

#[test]
fn undo_of_an_inserted_and_a_deleted_step() {
    let mut s = Sim::neu(&["Anna", "Bert"]);
    let schritte = s.clients[0].teil().features.len();
    s.befehl(0, "solid.fillet", json!({"edges": [[0, 20, 8]], "radius": 2}));
    s.ruhe();
    s.groesse(1, "breite", "80 mm");
    s.ruhe();
    s.clients[0].rueckgaengig().expect("undo insert");
    s.ruhe();
    assert_eq!(s.clients[1].teil().features.len(), schritte);
    assert_eq!(s.wert(1, "breite"), "80 mm");
    s.alle_gleich();

    let name = s.clients[1].teil().features.last().map(|f| f.name.clone()).expect("steps");
    s.befehl(1, "timeline.delete", json!({"features": [name.clone()]}));
    s.ruhe();
    assert!(!s.clients[0].teil().features.iter().any(|f| f.name == name));
    s.clients[1].rueckgaengig().expect("undo delete");
    s.ruhe();
    assert!(s.clients[0].teil().features.iter().any(|f| f.name == name));
    s.alle_gleich();
}

#[test]
fn presence_travels_and_leaves_with_the_person() {
    let mut s = Sim::neu(&["Anna", "Bert"]);
    let n = s.clients[0].auswahl(vec!["Fläche oben".into()]);
    s.schicke(0, n);
    s.ruhe();
    let anna = s.clients[1].personen().find(|p| p.name == "Anna").expect("Anna is there");
    assert_eq!(anna.auswahl, vec!["Fläche oben".to_owned()]);
    s.trenne(0);
    s.ruhe();
    assert!(s.clients[1].personen().next().is_none());
    assert!(s.hat_meldung(1, "Anna ist gegangen"), "{:?}", s.meldungen[1]);
}

#[test]
fn the_same_parameter_changed_offline_the_later_arrival_wins_with_a_notice() {
    let mut s = Sim::neu(&["Anna", "Bert"]);
    s.trenne(1);
    s.groesse(1, "dicke", "12 mm");
    s.groesse(0, "dicke", "9 mm");
    s.ruhe();
    s.verbinde(1);
    s.ruhe();
    for i in 0..2 {
        assert_eq!(s.wert(i, "dicke"), "12 mm", "client {i}");
    }
    assert!(s.hat_meldung(0, "Hinweis: Bert hat gleichzeitig die Größe „dicke“ (jetzt 12 mm) geändert"), "{:?}", s.meldungen[0]);
    s.alle_gleich();
}

/// The same over TCP: a server on a free port, two connections.
#[test]
fn over_tcp_two_clients_end_on_the_same_bytes() {
    let laufend = starte("127.0.0.1:0", Server::neu(vec![(TEIL.to_owned(), winkelhalter())]).expect("server")).expect("listens");
    let adresse = laufend.adresse.to_string();
    let mut clients = Vec::new();
    for name in ["Anna", "Bert"] {
        let (tx, rx) = kanal();
        let mut l = Leitung::oeffne(&adresse, tx, |e| e).expect("connects");
        let c = Client::neu(name, TEIL);
        l.sende(&c.hallo()).expect("hello");
        clients.push((c, l, rx));
    }
    let pumpe = |clients: &mut Vec<(Client, Leitung, std::sync::mpsc::Receiver<Ereignis>)>, bis: &dyn Fn(&[&Client]) -> bool| {
        let ende = Instant::now() + Duration::from_secs(60);
        loop {
            for (c, l, rx) in clients.iter_mut() {
                while let Ok(e) = rx.try_recv() {
                    match e {
                        Ereignis::Nachricht(n) => c.empfange(n),
                        Ereignis::Getrennt => c.getrennt(),
                    }
                }
                while let Some(n) = c.ausgehend() {
                    l.sende(&n).expect("send");
                }
            }
            let alle: Vec<&Client> = clients.iter().map(|(c, _, _)| c).collect();
            if bis(&alle) {
                return;
            }
            assert!(Instant::now() < ende, "no convergence over TCP");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    pumpe(&mut clients, &|cs| cs.iter().all(|c| c.ist_verbunden()));
    clients[0].0.befehl("parameters.change", &json!({"name": "dicke", "expression": "10 mm"})).expect("change");
    clients[1].0.befehl("parameters.change", &json!({"name": "breite", "expression": "80 mm"})).expect("change");
    pumpe(&mut clients, &|cs| cs.iter().all(|c| c.offen() == 0 && c.bestaetigt().1 == 2));
    let a = clients[0].0.teil().to_json();
    let b = clients[1].0.teil().to_json();
    assert!(a == b, "the two clients differ");
    assert!(a.contains("\"10 mm\"") && a.contains("\"80 mm\""));
}
