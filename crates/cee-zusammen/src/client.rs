//! The client as pure logic: commands and server messages in, messages to the server and lines
//! to show out. The network only moves the messages (see [`crate::netz`]).
//!
//! As in FigCraft's client (see ATTRIBUTION): a command runs once, here, with SolidCraft's own
//! session, and shows at once. The change set between the state before and after it goes to the
//! server. The client keeps the confirmed state (what the server ordered) and its own sets not yet
//! confirmed; what it shows is the confirmed state with its own sets on top. One set is on the way
//! at a time; the next is made again from the confirmed state when the answer comes, so every set
//! the server gets starts from a state the server had. Offline, sets wait and go out after the
//! connection is back.
//!
//! Undo takes back only the person's own steps, and of those only what nobody else changed since.

use std::collections::{BTreeMap, VecDeque};

use cee_aenderung::kanon::{baum, teil_aus_baum};
use cee_aenderung::{Aenderung, Aenderungssatz, berechne, lies, wende_an};
use serde_json::Value;
use solidcraft_doc::Document;
use solidcraft_engine::Session;

use crate::protokoll::{AnClient, AnServer, Bestaetigt, Nachholen, PROTOKOLL, Person, Wieder};
use crate::vereinen::{self, Art};

/// The most undo steps kept.
pub const MAX_RUECKGAENGIG: usize = 200;

/// One of the client's own sets, not yet confirmed.
struct Offen {
    nr: u64,
    /// The changes, made on the state shown when the command ran.
    aenderungen: Vec<Aenderung>,
    /// The set as sent (from a confirmed state), once it is on the way.
    gesendet: Option<Aenderungssatz>,
}

/// One undo (or redo) step: the changes that take a set of the person's back.
struct Schritt {
    /// The set this step takes back (to learn the server's order when it is confirmed).
    nr: u64,
    umkehr: Vec<Aenderung>,
    was: String,
}

/// The client of one person on one part.
pub struct Client {
    name: String,
    teil: String,
    id: Option<u64>,
    verbunden: bool,
    bestaetigt: Document,
    seq: u64,
    ansicht: Document,
    session: Session,
    unterwegs: Option<Offen>,
    warten: VecDeque<Offen>,
    naechste_nr: u64,
    rueck: Vec<Schritt>,
    vor: Vec<Schritt>,
    personen: BTreeMap<u64, Person>,
    skizze: Option<u64>,
    meldungen: Vec<String>,
}

impl Client {
    /// A client for person `name` on part `teil`, not yet connected (it has no state until the
    /// server's welcome).
    pub fn neu(name: &str, teil: &str) -> Self {
        let leer = Document::default();
        Self {
            name: name.to_owned(),
            teil: teil.to_owned(),
            id: None,
            verbunden: false,
            bestaetigt: leer.clone(),
            seq: 0,
            ansicht: leer.clone(),
            session: Session::new(leer),
            unterwegs: None,
            warten: VecDeque::new(),
            naechste_nr: 1,
            rueck: Vec::new(),
            vor: Vec::new(),
            personen: BTreeMap::new(),
            skizze: None,
            meldungen: Vec::new(),
        }
    }

    /// The hello to send on (re)connecting.
    pub fn hallo(&self) -> AnServer {
        let wieder = self.id.map(|client| Wieder { client, seq: self.seq });
        AnServer::Hallo { protokoll: PROTOKOLL, teil: self.teil.clone(), name: self.name.clone(), wieder }
    }

    /// The connection is gone (or was cut on purpose): work goes on locally.
    pub fn getrennt(&mut self) {
        if self.verbunden {
            self.verbunden = false;
            self.skizze = None;
            self.personen.clear();
            self.melde("Offline: Änderungen werden gesammelt und nach dem Wiederverbinden geschickt.".into());
        }
    }

    pub fn ist_verbunden(&self) -> bool {
        self.verbunden
    }

    /// The design as this person sees it (confirmed state and own sets on top).
    pub fn teil(&self) -> &Document {
        &self.ansicht
    }

    /// The state the server confirmed, and its sequence number.
    pub fn bestaetigt(&self) -> (&Document, u64) {
        (&self.bestaetigt, self.seq)
    }

    /// Own sets not yet confirmed (on the way and waiting).
    pub fn offen(&self) -> usize {
        self.warten.len() + usize::from(self.unterwegs.is_some())
    }

    pub fn personen(&self) -> impl Iterator<Item = &Person> {
        self.personen.values()
    }

    pub fn id(&self) -> Option<u64> {
        self.id
    }

    /// The sketch this person works in (locked by the server for them).
    pub fn skizze(&self) -> Option<u64> {
        self.skizze
    }

    /// What happened since the last call, as lines to show (others' changes, notices, arrivals).
    pub fn meldungen(&mut self) -> Vec<String> {
        std::mem::take(&mut self.meldungen)
    }

    fn melde(&mut self, s: String) {
        self.meldungen.push(s);
    }

    /// Run a SolidCraft command on the design shown. Returns what it changed, in words.
    pub fn befehl(&mut self, id: &str, params: &Value) -> Result<Vec<String>, String> {
        if !self.empfangen() {
            return Err("noch kein Stand vom Server".into());
        }
        let ergebnis = self.session.execute(id, params).map_err(|e| e.to_string());
        // SolidCraft's own snapshot undo is not used while working together.
        self.session.undo.clear();
        self.session.redo.clear();
        if let Err(e) = ergebnis {
            self.session_nachziehen();
            return Err(e);
        }
        let nachher = (*self.session.doc).clone();
        let text = self.eigene(nachher, id)?;
        self.vor.clear();
        Ok(text)
    }

    /// Whether the server's state has come (the welcome).
    fn empfangen(&self) -> bool {
        self.id.is_some()
    }

    /// Take `nachher` as the next state shown, as one own set. `None` if nothing changed.
    fn eigene(&mut self, nachher: Document, was: &str) -> Result<Vec<String>, String> {
        let satz = berechne(&self.ansicht, &nachher).map_err(|e| e.to_string())?;
        if satz.aenderungen.is_empty() {
            return Ok(Vec::new());
        }
        let umkehr = berechne(&nachher, &self.ansicht).map_err(|e| e.to_string())?.aenderungen;
        let text = lies(&satz, Some(&self.ansicht));
        let nr = self.naechste_nr;
        self.naechste_nr += 1;
        self.rueck.push(Schritt { nr, umkehr, was: was.to_owned() });
        if self.rueck.len() > MAX_RUECKGAENGIG {
            self.rueck.remove(0);
        }
        self.warten.push_back(Offen { nr, aenderungen: satz.aenderungen, gesendet: None });
        self.ansicht = nachher;
        Ok(text)
    }

    /// Take back the person's last own step, where nobody changed it since.
    pub fn rueckgaengig(&mut self) -> Result<Vec<String>, String> {
        self.umkehren(true)
    }

    /// Redo the step undone last.
    pub fn wiederholen(&mut self) -> Result<Vec<String>, String> {
        self.umkehren(false)
    }

    fn umkehren(&mut self, rueck: bool) -> Result<Vec<String>, String> {
        loop {
            let schritt = if rueck { self.rueck.pop() } else { self.vor.pop() };
            let Some(schritt) = schritt else {
                return Err(if rueck { "nichts mehr rückgängig zu machen" } else { "nichts zu wiederholen" }.into());
            };
            let mut b = baum(&self.ansicht).map_err(|e| e.to_string())?;
            let gelassen = vereinen::wende_an(&mut b, &schritt.umkehr, Art::NurUnveraendert)?;
            let neu = teil_aus_baum(&b).map_err(|e| e.to_string())?;
            let satz = berechne(&self.ansicht, &neu).map_err(|e| e.to_string())?;
            if satz.aenderungen.is_empty() {
                // Everything of it changed by others since: the next step.
                self.melde(format!("„{}“ übersprungen: inzwischen von anderen geändert.", schritt.was));
                continue;
            }
            if !gelassen.is_empty() {
                self.melde(format!("Nicht zurückgenommen, weil inzwischen von anderen geändert: {}", gelassen.join(", ")));
            }
            let text = lies(&satz, Some(&self.ansicht));
            let umkehr = berechne(&neu, &self.ansicht).map_err(|e| e.to_string())?.aenderungen;
            let nr = self.naechste_nr;
            self.naechste_nr += 1;
            let gegen = Schritt { nr, umkehr, was: schritt.was };
            if rueck {
                self.vor.push(gegen);
            } else {
                self.rueck.push(gegen);
            }
            self.warten.push_back(Offen { nr, aenderungen: satz.aenderungen, gesendet: None });
            self.ansicht = neu;
            self.session_nachziehen();
            return Ok(text);
        }
    }

    /// Ask to work in sketch `skizze` (it is locked for you if nobody else is in it), or leave it.
    pub fn in_skizze(&mut self, skizze: Option<u64>) -> AnServer {
        if skizze.is_none() {
            self.skizze_verlassen();
        }
        AnServer::Skizze { skizze }
    }

    /// Finish the sketch open in the command session, if one is.
    fn skizze_verlassen(&mut self) {
        self.skizze = None;
        if self.session.active_sketch.is_some() {
            match self.befehl("sketch.finish", &Value::Object(Default::default())) {
                Ok(text) => self.meldungen.extend(text),
                Err(e) => self.melde(format!("Skizze schließen: {e}")),
            }
        }
    }

    /// What the person selected (shown to the others, never stored).
    pub fn auswahl(&self, auswahl: Vec<String>) -> AnServer {
        AnServer::Anwesenheit { auswahl }
    }

    /// The next message for the server: the next own set, when none is on the way.
    pub fn ausgehend(&mut self) -> Option<AnServer> {
        if !self.verbunden || self.unterwegs.is_some() {
            return None;
        }
        while let Some(mut offen) = self.warten.pop_front() {
            // Made again from the confirmed state: the server knows that state.
            match self.neu_gemacht(&offen.aenderungen) {
                Ok(satz) if satz.aenderungen.is_empty() => {
                    self.vergiss(offen.nr);
                }
                Ok(satz) => {
                    let n = AnServer::Aenderungen { basis: self.seq, nr: offen.nr, satz: satz.clone() };
                    offen.gesendet = Some(satz);
                    self.unterwegs = Some(offen);
                    return Some(n);
                }
                Err(grund) => {
                    self.vergiss(offen.nr);
                    self.melde(format!("Eigene Änderung verworfen: {grund}"));
                    self.ansicht_neu();
                }
            }
        }
        None
    }

    /// `aenderungen` on the confirmed state, as a set from it.
    fn neu_gemacht(&self, aenderungen: &[Aenderung]) -> Result<Aenderungssatz, String> {
        let mut b = baum(&self.bestaetigt).map_err(|e| e.to_string())?;
        vereinen::wende_an(&mut b, aenderungen, Art::SpaeterGewinnt)?;
        let neu = teil_aus_baum(&b).map_err(|e| e.to_string())?;
        berechne(&self.bestaetigt, &neu).map_err(|e| e.to_string())
    }

    /// Drop the undo and redo steps of set `nr` (it was refused or came to nothing).
    fn vergiss(&mut self, nr: u64) {
        self.rueck.retain(|s| s.nr != nr);
        self.vor.retain(|s| s.nr != nr);
    }

    /// A message from the server.
    pub fn empfange(&mut self, n: AnClient) {
        match n {
            AnClient::Willkommen { client, seq, nachholen, personen, erledigt } => self.willkommen(client, seq, nachholen, personen, erledigt),
            AnClient::Aenderungen(b) => {
                if let Err(e) = self.bestaetigung(b) {
                    self.melde(format!("Fehler beim Abgleich: {e}"));
                }
                self.ansicht_neu();
            }
            AnClient::Abgelehnt { nr, grund, seq, teil } => {
                match teil_aus_baum(&teil) {
                    Ok(t) => {
                        self.bestaetigt = t;
                        self.seq = seq;
                    }
                    Err(e) => self.melde(format!("Fehler beim Abgleich: {e}")),
                }
                if self.unterwegs.as_ref().is_some_and(|o| o.nr == nr) {
                    self.unterwegs = None;
                }
                self.vergiss(nr);
                self.melde(format!("Abgelehnt: {grund}. Der Stand ist wieder der des Servers."));
                self.ansicht_neu();
            }
            AnClient::Hinweis { text } => self.melde(format!("Hinweis: {text}")),
            AnClient::Person(p) => {
                if Some(p.client) == self.id {
                    self.skizze_gewaehrt(p.skizze);
                } else {
                    let neu = !self.personen.contains_key(&p.client);
                    if neu {
                        self.melde(format!("{} ist dabei.", p.name));
                    }
                    let alt = self.personen.get(&p.client).and_then(|a| a.skizze);
                    if alt != p.skizze {
                        match p.skizze {
                            Some(s) => self.melde(format!("{} arbeitet in der Skizze {}.", p.name, self.schritt_name(s))),
                            None if alt.is_some() => self.melde(format!("{} hat die Skizze verlassen.", p.name)),
                            None => {}
                        }
                    }
                    self.personen.insert(p.client, p);
                }
            }
            AnClient::Weg { client } => {
                if let Some(p) = self.personen.remove(&client) {
                    self.melde(format!("{} ist gegangen.", p.name));
                }
            }
            AnClient::SkizzeBelegt { skizze, von } => {
                self.melde(format!("In der Skizze {} arbeitet gerade {von}; sie ist so lange gesperrt.", self.schritt_name(skizze)));
            }
            AnClient::Abgewiesen { grund } => self.melde(format!("Vom Server abgewiesen: {grund}")),
        }
    }

    fn skizze_gewaehrt(&mut self, skizze: Option<u64>) {
        if skizze != self.skizze {
            self.skizze = skizze;
            if let Some(s) = skizze {
                self.melde(format!("Du arbeitest in der Skizze {}; andere können sie so lange nicht ändern.", self.schritt_name(s)));
                // Open it in the command session, so sketch commands go into it.
                let name = self.ansicht.feature(s).map(|f| f.name.clone()).unwrap_or_default();
                match self.befehl("sketch.edit", &serde_json::json!({ "sketch": name })) {
                    Ok(text) => self.meldungen.extend(text),
                    Err(e) => self.melde(format!("Skizze öffnen: {e}")),
                }
            }
        }
    }

    fn schritt_name(&self, id: u64) -> String {
        self.ansicht.feature(id).map_or_else(|| format!("Nr. {id}"), |f| format!("„{}“", f.name))
    }

    fn willkommen(&mut self, client: u64, seq: u64, nachholen: Nachholen, personen: Vec<Person>, erledigt: u64) {
        let zurueck = self.id == Some(client);
        self.id = Some(client);
        self.verbunden = true;
        self.personen = personen.into_iter().map(|p| (p.client, p)).collect();
        match nachholen {
            Nachholen::Stand { teil } => match teil_aus_baum(&teil) {
                Ok(t) => {
                    if !zurueck {
                        // A fresh start: a session on the server's design.
                        self.ansicht = t.clone();
                        self.session = Session::new(t.clone());
                    }
                    self.bestaetigt = t;
                    self.seq = seq;
                }
                Err(e) => self.melde(format!("Fehler beim Abgleich: {e}")),
            },
            Nachholen::Saetze { saetze } => {
                let anzahl = saetze.len();
                for b in saetze {
                    if let Err(e) = self.bestaetigung(b) {
                        self.melde(format!("Fehler beim Abgleich: {e}"));
                    }
                }
                if anzahl > 0 {
                    self.melde(format!("{anzahl} verpasste Änderung(en) nachgeholt."));
                }
            }
        }
        // The set that was on the way when the connection dropped: confirmed above, refused while
        // away, or never arrived (then it goes out again).
        if let Some(o) = self.unterwegs.take() {
            if o.nr <= erledigt {
                self.vergiss(o.nr);
                self.melde("Eine Änderung von vor dem Verbindungsabbruch wurde abgelehnt.".into());
            } else {
                self.warten.push_front(Offen { gesendet: None, ..o });
            }
        }
        let n = self.warten.len();
        if zurueck && n > 0 {
            self.melde(format!("Wieder verbunden: {n} eigene Änderung(en) werden nachgeschickt."));
        } else if zurueck {
            self.melde("Wieder verbunden.".into());
        }
        self.ansicht_neu();
    }

    /// An accepted set: on the confirmed state. Our own comes back as confirmation.
    fn bestaetigung(&mut self, b: Bestaetigt) -> Result<(), String> {
        if b.seq != self.seq + 1 {
            return Err(format!("Stand {} erwartet, {} bekommen", self.seq + 1, b.seq));
        }
        let vorher = self.bestaetigt.clone();
        let neu = wende_an(&vorher, &b.satz).map_err(|e| e.to_string())?;
        let eigen = Some(b.autor) == self.id;
        if eigen && self.unterwegs.as_ref().is_some_and(|o| o.nr == b.nr) {
            self.unterwegs = None;
            // The undo step learns what the set replaced in the server's order.
            let umkehr = berechne(&neu, &vorher).map_err(|e| e.to_string())?.aenderungen;
            for s in self.rueck.iter_mut().chain(self.vor.iter_mut()).filter(|s| s.nr == b.nr) {
                s.umkehr = umkehr.clone();
            }
        } else if !eigen && !b.satz.aenderungen.is_empty() {
            for z in lies(&b.satz, Some(&vorher)) {
                self.melde(format!("{}: {z}", b.autor_name));
            }
        }
        self.bestaetigt = neu;
        self.seq = b.seq;
        Ok(())
    }

    /// The design shown: the confirmed state with the own sets on top.
    fn ansicht_neu(&mut self) {
        let neu = match baum(&self.bestaetigt) {
            Ok(mut b) => {
                let eigene: Vec<&Offen> = self.unterwegs.iter().chain(self.warten.iter()).collect();
                for o in eigene {
                    let aenderungen = o.gesendet.as_ref().map_or(&o.aenderungen, |s| &s.aenderungen);
                    let mut versuch = b.clone();
                    if vereinen::wende_an(&mut versuch, aenderungen, Art::SpaeterGewinnt).is_ok() {
                        b = versuch;
                    }
                }
                teil_aus_baum(&b).unwrap_or_else(|_| self.bestaetigt.clone())
            }
            Err(_) => self.bestaetigt.clone(),
        };
        if neu != self.ansicht {
            self.ansicht = neu;
            self.session_nachziehen();
        }
    }

    /// The command session works on the design shown.
    fn session_nachziehen(&mut self) {
        if *self.session.doc != self.ansicht {
            *self.session.doc_mut() = self.ansicht.clone();
            self.session.refresh();
        }
    }
}
