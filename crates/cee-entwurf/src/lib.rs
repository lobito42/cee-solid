//! CEE drafts (Entwürfe) of a SolidCraft design, offline, merged later.
//!
//! A *history* ([`verlauf::Verlauf`]) is a chain of change sets ([`cee_aenderung`]) kept as small
//! files in a folder, each named by the hash of its content; each names the entries it builds on,
//! who made it, and the fingerprints of the state it starts from and leads to. A *draft* is a
//! named pointer to its newest entry. People swap work by copying folders: files named by their
//! hash never collide. Two drafts are merged three ways from their common ancestor
//! ([`zusammen`]); conflicts are listed and decided per conflict, by side, or "the later one
//! wins"; the merged design is built again and checked ([`pruefer`]), and the report is kept in
//! the merge entry.
//!
//! The `.sc` file stays the truth: an entry records how it came about, and a draft's state is the
//! design its chain of change sets builds, checked against the fingerprints on the way.

pub mod eintrag;
pub mod pruefer;
pub mod verlauf;
pub mod zusammen;

use std::collections::{BTreeMap, BTreeSet};

use cee_aenderung::kanon::{baum, teil_aus_baum};
use cee_aenderung::{berechne, fingerabdruck, wende_an};
use solidcraft_doc::Document;

use crate::eintrag::{Eintrag, VERSION, Zusammenfuehrung};
use crate::verlauf::{Verlauf, kurz};
use crate::zusammen::{Ergebnis, Konflikt, Seite, fuehre_zusammen};

/// What can go wrong.
#[derive(Debug)]
pub enum Fehler {
    /// A file cannot be read or written, or is not what it should be.
    Datei(String),
    /// An entry file was changed after it was written (its hash does not fit).
    Veraendert(String),
    /// An entry is missing.
    Fehlt(String),
    /// There is no draft of that name.
    KeinEntwurf(String),
    /// The design or a change set does not fit.
    Teil(String),
}

impl std::fmt::Display for Fehler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fehler::Datei(s) => write!(f, "{s}"),
            Fehler::Veraendert(p) => write!(f, "{p} wurde verändert: der Inhalt passt nicht mehr zu seinem Namen"),
            Fehler::Fehlt(h) => write!(f, "der Satz {} fehlt in diesem Ordner (mit „holen“ aus dem anderen Ordner kopieren)", kurz(h)),
            Fehler::KeinEntwurf(n) => write!(f, "einen Entwurf „{n}“ gibt es hier nicht"),
            Fehler::Teil(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for Fehler {}

impl From<cee_aenderung::Fehler> for Fehler {
    fn from(e: cee_aenderung::Fehler) -> Self {
        Fehler::Teil(e.to_string())
    }
}

/// Builds states of entries, each once.
#[derive(Default)]
pub struct Staende {
    cache: BTreeMap<String, Document>,
}

impl Staende {
    /// The design at entry `h`: the change sets of its first-predecessor line applied in turn to
    /// an empty design, each checked against its fingerprints.
    pub fn stand(&mut self, v: &Verlauf, h: &str) -> Result<Document, Fehler> {
        let mut kette = Vec::new();
        let mut x = h.to_owned();
        let mut gesehen = BTreeSet::new();
        let mut start = loop {
            if let Some(d) = self.cache.get(&x) {
                break d.clone();
            }
            if !gesehen.insert(x.clone()) || gesehen.len() > verlauf::MAX_EINTRAEGE {
                return Err(Fehler::Datei("der Verlauf ist zu lang oder im Kreis".into()));
            }
            let e = v.eintrag(&x)?;
            let vor = e.vorgaenger.first().cloned();
            kette.push((x.clone(), e));
            match vor {
                Some(p) => x = p,
                None => break Document::default(),
            }
        };
        while let Some((x, e)) = kette.pop() {
            start = wende_an(&start, &e.satz).map_err(|f| Fehler::Teil(format!("Satz {}: {f}", kurz(&x))))?;
            self.cache.insert(x, start.clone());
        }
        Ok(start)
    }
}

/// The current time, seconds since 1970 (UTC).
pub fn jetzt() -> Option<u64> {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// Start a history in `ordner` from `teil`, with the draft `haupt`. The entry's hash.
pub fn neu(ordner: &std::path::Path, teil: &Document, autor: &str, zeit: Option<u64>) -> Result<String, Fehler> {
    let v = Verlauf::anlegen(ordner)?;
    let satz = berechne(&Document::default(), teil)?;
    let h = v.schreibe(&Eintrag { version: VERSION, vorgaenger: Vec::new(), autor: autor.to_owned(), zeit, satz, zusammenfuehrung: None })?;
    v.setze_entwurf("haupt", &h)?;
    Ok(h)
}

/// Save `teil` as the next state of draft `entwurf`: the change set from its last state as a new
/// entry. `None` when nothing changed.
pub fn sichern(v: &Verlauf, entwurf: &str, teil: &Document, autor: &str, zeit: Option<u64>) -> Result<Option<(String, Vec<String>)>, Fehler> {
    let kopf = v.entwurf(entwurf)?;
    let alt = Staende::default().stand(v, &kopf)?;
    let satz = berechne(&alt, teil)?;
    if satz.aenderungen.is_empty() {
        return Ok(None);
    }
    // The set must give exactly this design again.
    wende_an(&alt, &satz)?;
    let text = cee_aenderung::lies(&satz, Some(&alt));
    let h = v.schreibe(&Eintrag { version: VERSION, vorgaenger: vec![kopf], autor: autor.to_owned(), zeit, satz, zusammenfuehrung: None })?;
    v.setze_entwurf(entwurf, &h)?;
    Ok(Some((h, text)))
}

/// A new draft `neu` at the state of draft `von`.
pub fn verzweigen(v: &Verlauf, von: &str, neu: &str) -> Result<(), Fehler> {
    if v.hat_entwurf(neu) {
        return Err(Fehler::Datei(format!("einen Entwurf „{neu}“ gibt es schon")));
    }
    let h = v.entwurf(von)?;
    v.setze_entwurf(neu, &h)
}

/// How conflicts are decided.
pub enum Wahl<'a> {
    /// Not at all: conflicts are listed and nothing is written.
    Keine,
    /// Per conflict number; numbers not listed take `sonst` (if given).
    Je { je: BTreeMap<usize, Seite>, sonst: Option<Seite> },
    /// Whoever changed it later wins (by the recorded times; without them the draft merged in).
    SpaetererGewinnt,
    /// Asked, one by one.
    Fragen(&'a mut dyn FnMut(&Konflikt, &str, &str) -> Option<Seite>),
}

/// What became of a merge.
pub enum Ausgang {
    /// `ein` holds nothing `in` lacks.
    SchonEnthalten,
    /// `in` held nothing `ein` lacks: it now points where `ein` does.
    Vorgespult(String),
    /// Open conflicts: nothing was written.
    Konflikte(Ergebnis),
    /// Merged: the new entry, the result, and the checker's report.
    Zusammengefuehrt { satz: String, ergebnis: Ergebnis, teil: Box<Document>, bericht: pruefer::Bericht },
}

/// Merge draft `ein` into draft `nach`.
pub fn zusammenfuehren(v: &Verlauf, ein: &str, nach: &str, autor: &str, zeit: Option<u64>, wahl: Wahl) -> Result<Ausgang, Fehler> {
    let (hx, hy) = (v.entwurf(ein)?, v.entwurf(nach)?);
    let Some(basis) = v.gemeinsamer_vorfahr(&hx, &hy)? else {
        return Err(Fehler::Datei(format!("„{ein}“ und „{nach}“ haben keinen gemeinsamen Ausgangsstand")));
    };
    if basis == hx {
        return Ok(Ausgang::SchonEnthalten);
    }
    if basis == hy {
        v.setze_entwurf(nach, &hx)?;
        return Ok(Ausgang::Vorgespult(hx));
    }
    let mut staende = Staende::default();
    let (b, x, y) = (staende.stand(v, &basis)?, staende.stand(v, &hx)?, staende.stand(v, &hy)?);
    let (bb, xb, yb) = (baum(&b)?, baum(&x)?, baum(&y)?);
    let zeiten = match wahl {
        Wahl::SpaetererGewinnt => Some((zeiten_seit(v, &hx, &basis)?, zeiten_seit(v, &hy, &basis)?)),
        _ => None,
    };
    let mut wahl = wahl;
    let mut waehle = |k: &Konflikt| -> Option<Seite> {
        match &mut wahl {
            Wahl::Keine => None,
            Wahl::Je { je, sonst } => je.get(&k.nr).copied().or(*sonst),
            Wahl::SpaetererGewinnt => zeiten.as_ref().map(|(zx, zy)| spaeter(&k.ziel, zx, zy)),
            Wahl::Fragen(f) => f(k, ein, nach),
        }
    };
    let ergebnis = fuehre_zusammen(&bb, &xb, &yb, &mut waehle);
    if !ergebnis.entschieden() {
        return Ok(Ausgang::Konflikte(ergebnis));
    }
    let teil = teil_aus_baum(&ergebnis.baum).map_err(|e| Fehler::Teil(format!("das zusammengeführte Teil ist nicht gültig: {e}")))?;
    let bericht = pruefer::pruefe(&teil);
    let satz = berechne(&y, &teil)?;
    wende_an(&y, &satz)?;
    let mut entscheidungen: Vec<String> = ergebnis.konflikte.iter().map(|k| k.zeile(ein, nach)).collect();
    entscheidungen.extend(ergebnis.hinweise.iter().cloned());
    let z = Zusammenfuehrung { ein: ein.to_owned(), nach: nach.to_owned(), entscheidungen, pruefer_ok: bericht.ok, pruefer: bericht.zeilen.clone() };
    let e = Eintrag { version: VERSION, vorgaenger: vec![hy, hx], autor: autor.to_owned(), zeit, satz, zusammenfuehrung: Some(z) };
    let h = v.schreibe(&e)?;
    v.setze_entwurf(nach, &h)?;
    Ok(Ausgang::Zusammengefuehrt { satz: h, ergebnis, teil: Box::new(teil), bericht })
}

/// For the entries `kopf` has and `basis` does not: when each was made and what it touched.
fn zeiten_seit(v: &Verlauf, kopf: &str, basis: &str) -> Result<Vec<(Option<u64>, Vec<String>)>, Fehler> {
    let alt: BTreeSet<String> = v.vorfahren(basis)?.into_iter().collect();
    let mut out = Vec::new();
    for h in v.vorfahren(kopf)? {
        if alt.contains(&h) {
            continue;
        }
        let e = v.eintrag(&h)?;
        out.push((e.zeit, e.satz.aenderungen.iter().map(ziel).collect()));
    }
    Ok(out)
}

/// The side that changed `z` later: by the newest entry touching it on each side (or, where none
/// touches it, the newest entry of the side). Without times, or equal, the draft merged in.
fn spaeter(z: &str, x: &[(Option<u64>, Vec<String>)], y: &[(Option<u64>, Vec<String>)]) -> Seite {
    let neueste = |s: &[(Option<u64>, Vec<String>)]| {
        let treffer = s.iter().filter(|(_, ziele)| ziele.iter().any(|a| ueberschneiden(a, z))).filter_map(|(t, _)| *t).max();
        treffer.or_else(|| s.iter().filter_map(|(t, _)| *t).max())
    };
    match (neueste(x), neueste(y)) {
        (Some(a), Some(b)) if b > a => Seite::In,
        _ => Seite::Ein,
    }
}

/// What a change is about, as a path-like target (the same shape as [`Konflikt::ziel`]).
pub fn ziel(a: &cee_aenderung::Aenderung) -> String {
    use cee_aenderung::Aenderung as A;
    use cee_aenderung::kanon::zeiger_teil;
    match a {
        A::GroesseNeu { name, .. } | A::GroesseGesetzt { name, .. } | A::GroesseGeloescht { name, .. } | A::GroesseVerschoben { name, .. } => {
            format!("groesse/{}", zeiger_teil(name))
        }
        A::SchrittEingefuegt { id, .. } | A::SchrittGeloescht { id, .. } | A::SchrittVerschoben { id, .. } => format!("schritt/{id}"),
        A::SchrittUmbenannt { id, .. } => format!("schritt/{id}/name"),
        A::SchrittUnterdrueckt { id, .. } => format!("schritt/{id}/suppressed"),
        A::SchrittGeaendert { id, pfad, .. } => format!("schritt/{id}{pfad}"),
        A::ElementHinzu { schritt, liste, .. }
        | A::ElementWeg { schritt, liste, .. }
        | A::ElementGeaendert { schritt, liste, .. }
        | A::ElementVerschoben { schritt, liste, .. } => format!("schritt/{schritt}{liste}"),
        A::Eigenschaft { pfad, .. } => format!("teil{pfad}"),
    }
}

/// Whether two targets are about the same thing or one holds the other.
pub fn ueberschneiden(a: &str, b: &str) -> bool {
    let teil_von = |kurz: &str, lang: &str| lang == kurz || lang.strip_prefix(kurz).is_some_and(|r| r.starts_with('/'));
    teil_von(a, b) || teil_von(b, a)
}

/// The fingerprint of `teil`, short, for people.
pub fn kurz_fingerabdruck(teil: &Document) -> String {
    fingerabdruck(teil).map(|f| f.chars().skip(7).take(10).collect()).unwrap_or_default()
}
