//! The server as pure logic: messages in, messages out. The network only moves them (see
//! [`crate::netz`]).
//!
//! One session per part. The server orders change sets: each accepted set gets the next sequence
//! number and goes to everyone. A set built on an older state is applied to the current one, the
//! later change winning ([`crate::vereinen::Art::SpaeterGewinnt`]); whoever made a change it
//! overwrote gets a notice. A set that does not fit is refused whole and the sender gets the
//! current state. The server reads the design only as data: it never computes geometry.
//!
//! Presence (names, selections, which sketch someone works in) lives in memory only and is gone
//! when the person leaves; so are the sketch locks. Nothing here is written to disk.

use std::collections::{BTreeMap, VecDeque};

use cee_aenderung::kanon::{baum, fingerabdruck_baum, teil_aus_baum};
use cee_aenderung::{Aenderungssatz, MAX_AENDERUNGEN, VERSION, berechne, wende_an};
use serde_json::Value;
use solidcraft_doc::Document;

use crate::protokoll::{AnClient, AnServer, Bestaetigt, MAX_AUSWAHL, MAX_NAME, Nachholen, PROTOKOLL, Person, gesaeubert};
use crate::vereinen::{self, Art, schritt_innen, ueberschneiden, ziel};

/// How many accepted sets the server keeps for clients coming back (and for working out what a
/// set built on an older state overwrites).
pub const MAX_JOURNAL: usize = 1000;

/// The most people in one session.
pub const MAX_PERSONEN: usize = 64;

/// One connection, as the network layer numbers them.
pub type Verbindung = u64;

/// What the server sends: to which connection, what.
pub type Ausgang = (Verbindung, AnClient);

struct Eintrag {
    bestaetigt: Bestaetigt,
    ziele: Vec<String>,
}

struct Teilnehmer {
    name: String,
    verbindung: Option<Verbindung>,
    auswahl: Vec<String>,
    skizze: Option<u64>,
    /// The highest set number of this client dealt with.
    erledigt: u64,
}

/// One part's session.
struct Sitzung {
    teil: Document,
    baum: Value,
    fp: String,
    seq: u64,
    journal: VecDeque<Eintrag>,
    teilnehmer: BTreeMap<u64, Teilnehmer>,
    naechster: u64,
}

impl Sitzung {
    fn neu(teil: Document) -> Result<Self, String> {
        let b = baum(&teil).map_err(|e| e.to_string())?;
        let fp = fingerabdruck_baum(&b).map_err(|e| e.to_string())?;
        Ok(Self { teil, baum: b, fp, seq: 0, journal: VecDeque::new(), teilnehmer: BTreeMap::new(), naechster: 1 })
    }

    /// The fingerprint of state `seq`, while the journal reaches back to it.
    fn fp_bei(&self, seq: u64) -> Option<&str> {
        if seq == self.seq {
            return Some(&self.fp);
        }
        let erstes = self.journal.front()?;
        if seq + 1 == erstes.bestaetigt.seq {
            return Some(&erstes.bestaetigt.satz.ausgangsstand);
        }
        self.journal.iter().find(|e| e.bestaetigt.seq == seq).map(|e| e.bestaetigt.satz.zielstand.as_str())
    }

    fn person(&self, client: u64) -> Option<Person> {
        let t = self.teilnehmer.get(&client)?;
        Some(Person { client, name: t.name.clone(), auswahl: t.auswahl.clone(), skizze: t.skizze })
    }

    fn verbunden(&self) -> impl Iterator<Item = (u64, Verbindung)> + '_ {
        self.teilnehmer.iter().filter_map(|(c, t)| t.verbindung.map(|v| (*c, v)))
    }

    fn an_alle(&self, n: &AnClient) -> Vec<Ausgang> {
        self.verbunden().map(|(_, v)| (v, n.clone())).collect()
    }

    fn an_andere(&self, ausser: u64, n: &AnClient) -> Vec<Ausgang> {
        self.verbunden().filter(|(c, _)| *c != ausser).map(|(_, v)| (v, n.clone())).collect()
    }

    fn name(&self, client: u64) -> String {
        self.teilnehmer.get(&client).map(|t| t.name.clone()).unwrap_or_else(|| format!("Nr. {client}"))
    }
}

/// The server: sessions by part name, connections to their client.
#[derive(Default)]
pub struct Server {
    sitzungen: BTreeMap<String, Sitzung>,
    verbindungen: BTreeMap<Verbindung, (String, u64)>,
}

impl Server {
    /// A server that offers these parts (name, design).
    pub fn neu(teile: Vec<(String, Document)>) -> Result<Self, String> {
        let mut s = Self::default();
        for (name, teil) in teile {
            s.sitzungen.insert(name, Sitzung::neu(teil)?);
        }
        Ok(s)
    }

    /// The names of the parts offered.
    pub fn teile(&self) -> Vec<String> {
        self.sitzungen.keys().cloned().collect()
    }

    /// The current state of a part and its sequence number.
    pub fn stand(&self, teil: &str) -> Option<(&Document, u64)> {
        self.sitzungen.get(teil).map(|s| (&s.teil, s.seq))
    }

    /// A message from connection `v`.
    pub fn empfange(&mut self, v: Verbindung, n: AnServer) -> Vec<Ausgang> {
        match n {
            AnServer::Hallo { protokoll, teil, name, wieder } => self.hallo(v, protokoll, &teil, &name, wieder),
            other => {
                let Some((teil, client)) = self.verbindungen.get(&v).cloned() else {
                    return vec![(v, AnClient::Abgewiesen { grund: "erst „Hallo“ schicken".into() })];
                };
                let Some(s) = self.sitzungen.get_mut(&teil) else { return Vec::new() };
                match other {
                    AnServer::Aenderungen { basis, gesehen, nr, satz } => aenderungen(s, v, client, basis, gesehen.min(basis), nr, satz),
                    AnServer::Anwesenheit { auswahl } => {
                        if let Some(t) = s.teilnehmer.get_mut(&client) {
                            t.auswahl = auswahl.iter().take(MAX_AUSWAHL).map(|a| gesaeubert(a, MAX_AUSWAHL)).collect();
                        }
                        s.person(client).map(|p| s.an_andere(client, &AnClient::Person(p))).unwrap_or_default()
                    }
                    AnServer::Skizze { skizze } => sperre(s, v, client, skizze),
                    AnServer::Hallo { .. } => vec![(v, AnClient::Abgewiesen { grund: "schon verbunden".into() })],
                }
            }
        }
    }

    /// Connection `v` is gone: the person leaves, their sketch lock with them.
    pub fn getrennt(&mut self, v: Verbindung) -> Vec<Ausgang> {
        let Some((teil, client)) = self.verbindungen.remove(&v) else { return Vec::new() };
        let Some(s) = self.sitzungen.get_mut(&teil) else { return Vec::new() };
        if let Some(t) = s.teilnehmer.get_mut(&client) {
            t.verbindung = None;
            t.skizze = None;
            t.auswahl.clear();
        }
        s.an_andere(client, &AnClient::Weg { client })
    }

    fn hallo(&mut self, v: Verbindung, protokoll: u32, teil: &str, name: &str, wieder: Option<crate::protokoll::Wieder>) -> Vec<Ausgang> {
        let ab = |grund: String| vec![(v, AnClient::Abgewiesen { grund })];
        if protokoll != PROTOKOLL {
            return ab(format!("Protokoll {protokoll}; dieser Server spricht {PROTOKOLL}"));
        }
        if self.verbindungen.contains_key(&v) {
            return ab("schon verbunden".into());
        }
        let Some(s) = self.sitzungen.get_mut(teil) else {
            return ab(format!("das Teil „{teil}“ gibt es hier nicht"));
        };
        if s.verbunden().count() >= MAX_PERSONEN {
            return ab("die Sitzung ist voll".into());
        }
        let name = match gesaeubert(name, MAX_NAME) {
            n if n.is_empty() => "Unbekannt".to_owned(),
            n => n,
        };
        // Coming back: the same client, if it is known and not connected elsewhere.
        let zurueck = wieder.filter(|w| s.teilnehmer.get(&w.client).is_some_and(|t| t.verbindung.is_none()));
        let client = match &zurueck {
            Some(w) => w.client,
            None => {
                let c = s.naechster;
                s.naechster += 1;
                c
            }
        };
        let erledigt = s.teilnehmer.get(&client).map_or(0, |t| t.erledigt);
        s.teilnehmer.insert(client, Teilnehmer { name, verbindung: Some(v), auswahl: Vec::new(), skizze: None, erledigt });
        self.verbindungen.insert(v, (teil.to_owned(), client));
        let nachholen = match zurueck.as_ref().and_then(|w| verpasst(s, w.seq)) {
            Some(saetze) => Nachholen::Saetze { saetze },
            None => Nachholen::Stand { teil: s.baum.clone() },
        };
        let personen = s.teilnehmer.keys().filter(|c| **c != client && s.teilnehmer.get(c).is_some_and(|t| t.verbindung.is_some()));
        let personen = personen.filter_map(|c| s.person(*c)).collect();
        let mut out = vec![(v, AnClient::Willkommen { client, seq: s.seq, nachholen, personen, erledigt })];
        if let Some(p) = s.person(client) {
            out.extend(s.an_andere(client, &AnClient::Person(p)));
        }
        out
    }
}

/// The sets after `seq`, when the journal still holds all of them.
fn verpasst(s: &Sitzung, seq: u64) -> Option<Vec<Bestaetigt>> {
    if seq > s.seq {
        return None;
    }
    if seq == s.seq {
        return Some(Vec::new());
    }
    let erstes = s.journal.front()?.bestaetigt.seq;
    if seq + 1 < erstes {
        return None;
    }
    Some(s.journal.iter().filter(|e| e.bestaetigt.seq > seq).map(|e| e.bestaetigt.clone()).collect())
}

fn sperre(s: &mut Sitzung, v: Verbindung, client: u64, skizze: Option<u64>) -> Vec<Ausgang> {
    if let Some(id) = skizze {
        let ist_skizze = s.teil.feature(id).is_some_and(|f| matches!(f.kind, solidcraft_doc::FeatureKind::Sketch { .. }));
        if !ist_skizze {
            return vec![(v, AnClient::Hinweis { text: format!("Schritt {id} ist keine Skizze") })];
        }
        if let Some(t) = s.teilnehmer.iter().find(|(c, t)| **c != client && t.verbindung.is_some() && t.skizze == Some(id)).map(|(_, t)| t) {
            return vec![(v, AnClient::SkizzeBelegt { skizze: id, von: t.name.clone() })];
        }
    }
    if let Some(t) = s.teilnehmer.get_mut(&client) {
        t.skizze = skizze;
    }
    s.person(client).map(|p| s.an_alle(&AnClient::Person(p))).unwrap_or_default()
}

fn aenderungen(s: &mut Sitzung, v: Verbindung, client: u64, basis: u64, gesehen: u64, nr: u64, satz: Aenderungssatz) -> Vec<Ausgang> {
    let Some(t) = s.teilnehmer.get_mut(&client) else { return Vec::new() };
    if nr <= t.erledigt {
        // Sent again after a dropped connection: already dealt with.
        return Vec::new();
    }
    t.erledigt = nr;
    match pruefe_und_wende_an(s, client, basis, &satz) {
        Ok(neu) => annehmen(s, client, nr, neu, gesehen),
        Err(grund) => vec![(v, AnClient::Abgelehnt { nr, grund, seq: s.seq, teil: s.baum.clone() })],
    }
}

/// The design after `satz`.
fn pruefe_und_wende_an(s: &Sitzung, client: u64, basis: u64, satz: &Aenderungssatz) -> Result<Document, String> {
    if satz.version != VERSION {
        return Err(format!("Änderungssatz in Version {}; dieser Server kennt Version {VERSION}", satz.version));
    }
    if satz.aenderungen.len() > MAX_AENDERUNGEN {
        return Err(format!("mehr als {MAX_AENDERUNGEN} Änderungen"));
    }
    let Some(fp) = s.fp_bei(basis) else {
        return Err(format!("der Satz baut auf Stand {basis} auf; der Server kennt die Stände {}–{}", erster_stand(s), s.seq));
    };
    if fp != satz.ausgangsstand {
        return Err("der Satz passt nicht zu dem Stand, auf dem er aufbauen will (Fingerabdruck)".into());
    }
    for a in &satz.aenderungen {
        if let Some(id) = schritt_innen(a)
            && let Some(t) = s.teilnehmer.iter().find(|(c, t)| **c != client && t.verbindung.is_some() && t.skizze == Some(id)).map(|(_, t)| t)
        {
            let name = s.teil.feature(id).map_or_else(|| format!("Nr. {id}"), |f| f.name.clone());
            return Err(format!("in der Skizze „{name}“ arbeitet gerade {}", t.name));
        }
    }
    if basis == s.seq {
        return wende_an(&s.teil, satz).map_err(|e| e.to_string());
    }
    let mut b = s.baum.clone();
    vereinen::wende_an(&mut b, &satz.aenderungen, Art::SpaeterGewinnt)?;
    teil_aus_baum(&b).map_err(|e| e.to_string())
}

fn erster_stand(s: &Sitzung) -> u64 {
    s.journal.front().map_or(s.seq, |e| e.bestaetigt.seq.saturating_sub(1))
}

fn annehmen(s: &mut Sitzung, client: u64, nr: u64, neu: Document, gesehen: u64) -> Vec<Ausgang> {
    let (wirksam, neu_baum) = match (berechne(&s.teil, &neu), baum(&neu)) {
        (Ok(w), Ok(b)) => (w, b),
        (Err(e), _) | (_, Err(e)) => {
            let v = s.teilnehmer.get(&client).and_then(|t| t.verbindung);
            return v.map(|v| vec![(v, AnClient::Abgelehnt { nr, grund: e.to_string(), seq: s.seq, teil: s.baum.clone() })]).unwrap_or_default();
        }
    };
    let ziele: Vec<String> = wirksam.aenderungen.iter().map(ziel).filter(|z| z != "teil/next_id").collect();
    let mut out = Vec::new();
    // Who made a change this set overwrites without the sender having seen it gets a notice.
    if gesehen < s.seq {
        let autor_name = s.name(client);
        let mut betroffen: BTreeMap<u64, Vec<String>> = BTreeMap::new();
        for e in &s.journal {
            if e.bestaetigt.seq <= gesehen || e.bestaetigt.autor == client {
                continue;
            }
            for z in &ziele {
                if e.ziele.iter().any(|alt| ueberschneiden(alt, z)) {
                    betroffen.entry(e.bestaetigt.autor).or_default().push(z.clone());
                }
            }
        }
        for (anderer, mut was) in betroffen {
            was.sort();
            was.dedup();
            if let Some(v) = s.teilnehmer.get(&anderer).and_then(|t| t.verbindung) {
                let text = format!("{autor_name} hat gleichzeitig {} geändert; es gilt jetzt der spätere Wert.", lesbar(&was, &neu));
                out.push((v, AnClient::Hinweis { text }));
            }
        }
    }
    s.seq += 1;
    let bestaetigt = Bestaetigt { seq: s.seq, autor: client, autor_name: s.name(client), nr, satz: wirksam };
    s.fp = bestaetigt.satz.zielstand.clone();
    s.teil = neu;
    s.baum = neu_baum;
    out.extend(s.an_alle(&AnClient::Aenderungen(bestaetigt.clone())));
    s.journal.push_back(Eintrag { bestaetigt, ziele });
    while s.journal.len() > MAX_JOURNAL {
        s.journal.pop_front();
    }
    out
}

/// Targets in words: "die Größe „dicke“ (jetzt 6 mm)", "Schritt „Kante vorn“".
fn lesbar(ziele: &[String], teil: &Document) -> String {
    let teile: Vec<String> = ziele
        .iter()
        .map(|z| {
            let mut stuecke = z.splitn(3, '/');
            match (stuecke.next(), stuecke.next()) {
                (Some("groesse"), Some(n)) => {
                    let n = n.replace("~1", "/").replace("~0", "~");
                    match teil.params.iter().find(|p| p.name == n) {
                        Some(p) => format!("die Größe „{n}“ (jetzt {})", p.expr),
                        None => format!("die Größe „{n}“ (jetzt gelöscht)"),
                    }
                }
                (Some("schritt"), Some(id)) => match id.parse::<u64>().ok().and_then(|id| teil.feature(id)) {
                    Some(f) => format!("den Schritt „{}“", f.name),
                    None => format!("den Schritt Nr. {id} (jetzt gelöscht)"),
                },
                _ => format!("„{z}“"),
            }
        })
        .collect();
    let mut teile = teile;
    teile.dedup();
    teile.join(", ")
}
