//! A history in a folder: entries as small files named by the hash of their content, drafts as
//! named pointers to an entry.
//!
//! ```text
//! <ordner>/cee-verlauf.json          marks the folder (format version)
//! <ordner>/saetze/<hash>.json        one entry; <hash> is the SHA-256 of the file's bytes
//! <ordner>/entwuerfe/<name>.json     a draft: {"satz": "<hash>"}
//! ```
//!
//! Every file is JSON with sorted keys, indented, ending in a newline, so the folder reads well
//! in a Git repository and the same entry always has the same bytes. Files named by their hash
//! never collide: two people's folders are put together by copying files ([`Verlauf::hole`]).
//! Reading an entry checks its hash, so a changed file is noticed.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::Fehler;
use crate::eintrag::Eintrag;

/// The largest entry file read (64 MiB, as a change set file).
pub const MAX_EINTRAG_BYTES: u64 = cee_aenderung::MAX_SATZ_BYTES as u64;

/// The longest draft name, in characters.
pub const MAX_NAME: usize = 80;

/// The most entries walked through in one history.
pub const MAX_EINTRAEGE: usize = 1_000_000;

const MARKE: &str = "cee-verlauf.json";
const SAETZE: &str = "saetze";
const ENTWUERFE: &str = "entwuerfe";

/// A draft pointer as saved.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Zeiger {
    satz: String,
}

/// JSON text with sorted keys, indented, ending in a newline.
pub fn kanon_text<T: Serialize>(v: &T) -> Result<String, Fehler> {
    let v = serde_json::to_value(v).map_err(|e| Fehler::Datei(e.to_string()))?;
    let mut s = serde_json::to_string_pretty(&cee_aenderung::kanon::sortiert(v)).map_err(|e| Fehler::Datei(e.to_string()))?;
    s.push('\n');
    Ok(s)
}

/// The hash of bytes: hex SHA-256.
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Whether `h` looks like a hash (64 lowercase hex digits).
pub fn ist_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Why `name` is no good as a draft name, if it is not.
pub fn name_problem(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("der Name des Entwurfs ist leer".into());
    }
    if name.chars().count() > MAX_NAME {
        return Some(format!("der Name des Entwurfs ist länger als {MAX_NAME} Zeichen"));
    }
    if name.starts_with('.') || name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return Some(format!("„{name}“ geht nicht als Name: keine / \\ : * ? \" < > | und kein Punkt am Anfang"));
    }
    None
}

fn lies_begrenzt(pfad: &Path) -> Result<Vec<u8>, Fehler> {
    let meta = std::fs::metadata(pfad).map_err(|e| Fehler::Datei(format!("{}: {e}", pfad.display())))?;
    if meta.len() > MAX_EINTRAG_BYTES {
        return Err(Fehler::Datei(format!("{}: größer als {} MiB", pfad.display(), MAX_EINTRAG_BYTES >> 20)));
    }
    std::fs::read(pfad).map_err(|e| Fehler::Datei(format!("{}: {e}", pfad.display())))
}

/// Write through a temporary file, so a half-written file never carries the name.
fn schreibe_datei(pfad: &Path, text: &str) -> Result<(), Fehler> {
    let tmp = pfad.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| Fehler::Datei(format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, pfad).map_err(|e| Fehler::Datei(format!("{}: {e}", pfad.display())))
}

/// A history folder.
pub struct Verlauf {
    ordner: PathBuf,
}

impl Verlauf {
    /// Start a history in `ordner` (made if missing; refused if it already holds one).
    pub fn anlegen(ordner: &Path) -> Result<Self, Fehler> {
        if ordner.join(MARKE).exists() {
            return Err(Fehler::Datei(format!("{} enthält schon einen Verlauf", ordner.display())));
        }
        for d in [SAETZE, ENTWUERFE] {
            std::fs::create_dir_all(ordner.join(d)).map_err(|e| Fehler::Datei(format!("{}: {e}", ordner.display())))?;
        }
        schreibe_datei(&ordner.join(MARKE), &kanon_text(&serde_json::json!({ "cee_verlauf": crate::eintrag::VERSION }))?)?;
        Ok(Self { ordner: ordner.to_owned() })
    }

    /// The history in `ordner`.
    pub fn oeffnen(ordner: &Path) -> Result<Self, Fehler> {
        let marke = ordner.join(MARKE);
        let bytes = lies_begrenzt(&marke).map_err(|_| Fehler::Datei(format!("{} ist kein Verlauf (mit „neu“ anlegen)", ordner.display())))?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| Fehler::Datei(format!("{}: {e}", marke.display())))?;
        match v.get("cee_verlauf").and_then(Value::as_u64) {
            Some(n) if n == u64::from(crate::eintrag::VERSION) => Ok(Self { ordner: ordner.to_owned() }),
            Some(n) => Err(Fehler::Datei(format!("Verlauf in Version {n}; dieses Programm kennt Version {}", crate::eintrag::VERSION))),
            None => Err(Fehler::Datei(format!("{} ist kein Verlauf", ordner.display()))),
        }
    }

    pub fn ordner(&self) -> &Path {
        &self.ordner
    }

    fn satz_pfad(&self, h: &str) -> PathBuf {
        self.ordner.join(SAETZE).join(format!("{h}.json"))
    }

    fn entwurf_pfad(&self, name: &str) -> PathBuf {
        self.ordner.join(ENTWUERFE).join(format!("{name}.json"))
    }

    /// Whether the entry `h` is here.
    pub fn hat(&self, h: &str) -> bool {
        ist_hash(h) && self.satz_pfad(h).is_file()
    }

    /// The entry `h`, its hash checked.
    pub fn eintrag(&self, h: &str) -> Result<Eintrag, Fehler> {
        if !ist_hash(h) {
            return Err(Fehler::Datei(format!("„{h}“ ist keine Satz-Kennung")));
        }
        let pfad = self.satz_pfad(h);
        if !pfad.is_file() {
            return Err(Fehler::Fehlt(h.to_owned()));
        }
        let bytes = lies_begrenzt(&pfad)?;
        if hash(&bytes) != h {
            return Err(Fehler::Veraendert(pfad.display().to_string()));
        }
        let e: Eintrag = serde_json::from_slice(&bytes).map_err(|e| Fehler::Datei(format!("{}: {e}", pfad.display())))?;
        if e.version != crate::eintrag::VERSION {
            return Err(Fehler::Datei(format!("{}: Version {}", pfad.display(), e.version)));
        }
        if e.vorgaenger.len() > 2 || e.vorgaenger.iter().any(|v| !ist_hash(v)) {
            return Err(Fehler::Datei(format!("{}: die Vorgänger sind ungültig", pfad.display())));
        }
        Ok(e)
    }

    /// Save an entry; its hash.
    pub fn schreibe(&self, e: &Eintrag) -> Result<String, Fehler> {
        let text = kanon_text(e)?;
        let h = hash(text.as_bytes());
        let pfad = self.satz_pfad(&h);
        if !pfad.exists() {
            schreibe_datei(&pfad, &text)?;
        }
        Ok(h)
    }

    /// The entry draft `name` points to.
    pub fn entwurf(&self, name: &str) -> Result<String, Fehler> {
        if let Some(p) = name_problem(name) {
            return Err(Fehler::Datei(p));
        }
        let pfad = self.entwurf_pfad(name);
        if !pfad.is_file() {
            return Err(Fehler::KeinEntwurf(name.to_owned()));
        }
        let z: Zeiger = serde_json::from_slice(&lies_begrenzt(&pfad)?).map_err(|e| Fehler::Datei(format!("{}: {e}", pfad.display())))?;
        if !ist_hash(&z.satz) {
            return Err(Fehler::Datei(format!("{}: keine Satz-Kennung", pfad.display())));
        }
        Ok(z.satz)
    }

    /// Whether draft `name` exists.
    pub fn hat_entwurf(&self, name: &str) -> bool {
        name_problem(name).is_none() && self.entwurf_pfad(name).is_file()
    }

    /// Point draft `name` at entry `h`.
    pub fn setze_entwurf(&self, name: &str, h: &str) -> Result<(), Fehler> {
        if let Some(p) = name_problem(name) {
            return Err(Fehler::Datei(p));
        }
        if !self.hat(h) {
            return Err(Fehler::Fehlt(h.to_owned()));
        }
        schreibe_datei(&self.entwurf_pfad(name), &kanon_text(&Zeiger { satz: h.to_owned() })?)
    }

    /// The drafts: name and entry, sorted by name.
    pub fn entwuerfe(&self) -> Result<Vec<(String, String)>, Fehler> {
        let mut out = Vec::new();
        let dir = self.ordner.join(ENTWUERFE);
        let liste = std::fs::read_dir(&dir).map_err(|e| Fehler::Datei(format!("{}: {e}", dir.display())))?;
        for d in liste.flatten() {
            let p = d.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".json")) else { continue };
            if name_problem(name).is_none() {
                out.push((name.to_owned(), self.entwurf(name)?));
            }
        }
        out.sort();
        Ok(out)
    }

    /// The hashes of all entry files here (by name).
    pub fn alle(&self) -> Result<Vec<String>, Fehler> {
        let dir = self.ordner.join(SAETZE);
        let liste = std::fs::read_dir(&dir).map_err(|e| Fehler::Datei(format!("{}: {e}", dir.display())))?;
        let mut out: Vec<String> = liste
            .flatten()
            .filter_map(|d| d.path().file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".json")).map(str::to_owned))
            .filter(|h| ist_hash(h))
            .collect();
        out.sort();
        Ok(out)
    }

    /// Check every entry file and every draft: what is wrong, in words (empty: all fine).
    pub fn pruefe_dateien(&self) -> Result<Vec<String>, Fehler> {
        let mut out = Vec::new();
        for h in self.alle()? {
            match self.eintrag(&h) {
                Ok(e) => {
                    for v in &e.vorgaenger {
                        if !self.hat(v) {
                            out.push(format!("Satz {} baut auf {} auf, der fehlt", kurz(&h), kurz(v)));
                        }
                    }
                }
                Err(e) => out.push(e.to_string()),
            }
        }
        for (name, h) in self.entwuerfe()? {
            if !self.hat(&h) {
                out.push(format!("Entwurf „{name}“ zeigt auf {}, den es nicht gibt", kurz(&h)));
            }
        }
        Ok(out)
    }

    /// `h` and every entry it builds on, each once, newest first (breadth first).
    pub fn vorfahren(&self, h: &str) -> Result<Vec<String>, Fehler> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        let mut q = VecDeque::from([h.to_owned()]);
        while let Some(x) = q.pop_front() {
            if !seen.insert(x.clone()) {
                continue;
            }
            if seen.len() > MAX_EINTRAEGE {
                return Err(Fehler::Datei(format!("mehr als {MAX_EINTRAEGE} Sätze")));
            }
            let e = self.eintrag(&x)?;
            q.extend(e.vorgaenger.iter().cloned());
            out.push(x);
        }
        Ok(out)
    }

    /// The common ancestor of `a` and `b` to merge from: a common ancestor that no other common
    /// ancestor builds on. When there are several (criss-cross merges), the smallest hash, so the
    /// choice does not depend on the order of `a` and `b`.
    pub fn gemeinsamer_vorfahr(&self, a: &str, b: &str) -> Result<Option<String>, Fehler> {
        let va: BTreeSet<String> = self.vorfahren(a)?.into_iter().collect();
        let gemeinsam: BTreeSet<String> = self.vorfahren(b)?.into_iter().filter(|x| va.contains(x)).collect();
        let mut beste = Vec::new();
        for c in &gemeinsam {
            // `c` is not the best if another common ancestor builds on it.
            let mut ueberholt = false;
            for d in &gemeinsam {
                if d != c && self.vorfahren(d)?.iter().any(|x| x == c) {
                    ueberholt = true;
                    break;
                }
            }
            if !ueberholt {
                beste.push(c.clone());
            }
        }
        Ok(beste.into_iter().min())
    }

    /// Copy what `andere` has and this history lacks: entries (each checked first) and drafts,
    /// as `<name>@<quelle>`. Drafts that already carry an `@` there are not copied. Returns the
    /// number of entries copied, the drafts set and what was skipped.
    pub fn hole(&self, andere: &Verlauf, quelle: &str) -> Result<(usize, Vec<String>, Vec<String>), Fehler> {
        let mut kopiert = 0;
        let mut ausgelassen = Vec::new();
        for h in andere.alle()? {
            if self.hat(&h) {
                continue;
            }
            match andere.eintrag(&h) {
                Ok(e) => {
                    let neu = self.schreibe(&e)?;
                    if neu == h {
                        kopiert += 1;
                    } else {
                        ausgelassen.push(format!("Satz {}: anders geschrieben", kurz(&h)));
                    }
                }
                Err(e) => ausgelassen.push(e.to_string()),
            }
        }
        let mut gesetzt = Vec::new();
        for (name, h) in andere.entwuerfe()? {
            if name.contains('@') {
                continue;
            }
            let neu = format!("{name}@{quelle}");
            if name_problem(&neu).is_some() || !self.hat(&h) {
                ausgelassen.push(format!("Entwurf „{name}“"));
                continue;
            }
            self.setze_entwurf(&neu, &h)?;
            gesetzt.push(neu);
        }
        Ok((kopiert, gesetzt, ausgelassen))
    }

    /// Every entry of the history of `h`, oldest first: each comes after the entries it builds
    /// on; among those ready at once, the first predecessor's line first, then by hash.
    pub fn reihenfolge(&self, h: &str) -> Result<Vec<(String, Eintrag)>, Fehler> {
        let alle = self.vorfahren(h)?;
        let mut eintraege: BTreeMap<String, Eintrag> = BTreeMap::new();
        for x in &alle {
            eintraege.insert(x.clone(), self.eintrag(x)?);
        }
        let mut fertig: BTreeSet<String> = BTreeSet::new();
        let mut out = Vec::new();
        // Depth first from the head, predecessors before the entry (iteratively).
        let mut stapel: Vec<(String, bool)> = vec![(h.to_owned(), false)];
        while let Some((x, bereit)) = stapel.pop() {
            if fertig.contains(&x) {
                continue;
            }
            let Some(e) = eintraege.get(&x) else { continue };
            if bereit {
                fertig.insert(x.clone());
                out.push((x, e.clone()));
                continue;
            }
            stapel.push((x.clone(), true));
            // The second predecessor's line is pushed first, so the first's comes out first.
            for v in e.vorgaenger.iter().rev() {
                if !fertig.contains(v) {
                    stapel.push((v.clone(), false));
                }
            }
        }
        Ok(out)
    }
}

/// The first 10 characters of a hash, for people.
pub fn kurz(h: &str) -> String {
    h.chars().take(10).collect()
}
