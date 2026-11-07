//! CEE change sets (Änderungssätze) for SolidCraft designs.
//!
//! [`berechne`] lists what turns one state of a design into another, [`wende_an`] applies such a
//! list to a design (all or nothing), [`lies`] says it in plain German. A change set names the
//! state it starts from and the state it leads to by their [`fingerabdruck`]. This is the
//! foundation for working together on one design (CEE's collaboration plan, step 1): the command
//! runs once where it is given, and what travels is the change set.
//!
//! The crate works on the design as SolidCraft saves it (a JSON tree), so it needs no change to
//! SolidCraft's model or file format.

mod anwenden;
mod berechnen;
pub mod kanon;
mod lesen;
pub mod liste;
pub mod satz;

pub use anwenden::{MAX_AENDERUNGEN, wende_an};
pub use berechnen::berechne;
pub use kanon::fingerabdruck;
pub use lesen::lies;
pub use satz::{Aenderung, Aenderungssatz, VERSION};

/// The largest change set file read (64 MiB).
pub const MAX_SATZ_BYTES: usize = 64 << 20;

/// What can go wrong.
#[derive(Debug, thiserror::Error)]
pub enum Fehler {
    #[error("das Teil lässt sich nicht lesen oder schreiben: {0}")]
    Teil(String),
    #[error("der Änderungssatz lässt sich nicht lesen: {0}")]
    Satz(String),
    #[error("Änderungssatz in Version {0}; dieses Programm kennt Version {VERSION}")]
    Version(u32),
    #[error("der Änderungssatz passt nicht zu diesem Stand des Teils (erwartet {erwartet}, gefunden {gefunden})")]
    FalscherAusgangsstand { erwartet: String, gefunden: String },
    #[error("Änderung {nummer} passt nicht: {grund}; nichts wurde geändert")]
    Ungueltig { nummer: usize, grund: String },
    #[error("das Ergebnis ist nicht der Stand, den der Satz verspricht (erwartet {erwartet}, gefunden {gefunden}); nichts wurde geändert")]
    Zielstand { erwartet: String, gefunden: String },
}

/// A change set from its JSON text (size-limited, unknown fields rejected).
pub fn satz_aus_json(text: &[u8]) -> Result<Aenderungssatz, Fehler> {
    if text.len() > MAX_SATZ_BYTES {
        return Err(Fehler::Satz(format!("größer als {} MiB", MAX_SATZ_BYTES >> 20)));
    }
    serde_json::from_slice(text).map_err(|e| Fehler::Satz(e.to_string()))
}

/// A change set as JSON text: keys sorted, indented, ending in a newline. The same set always
/// gives the same bytes.
pub fn satz_als_json(satz: &Aenderungssatz) -> Result<String, Fehler> {
    let v = serde_json::to_value(satz).map_err(|e| Fehler::Satz(e.to_string()))?;
    let mut s = serde_json::to_string_pretty(&kanon::sortiert(v)).map_err(|e| Fehler::Satz(e.to_string()))?;
    s.push('\n');
    Ok(s)
}
