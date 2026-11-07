//! The messages between the server and the clients, one JSON object per line.
//!
//! The shape follows FigCraft's collaboration protocol (see ATTRIBUTION): a client joins with a
//! hello, sends one user action's change set with the server sequence number it builds on, and the
//! server answers everyone with the accepted set and its sequence number, or the sender alone with
//! a rejection and the right state. Presence (names, selections, sketch locks) only travels live
//! and is never written anywhere. Geometry never travels: every client computes it itself.

use cee_aenderung::Aenderungssatz;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version this build speaks.
pub const PROTOKOLL: u32 = 1;

/// The longest line read (64 MiB, the size limit of a change set file).
pub const MAX_ZEILE: usize = cee_aenderung::MAX_SATZ_BYTES;

/// The longest name kept, in characters.
pub const MAX_NAME: usize = 40;

/// The most selection entries kept per person, and their length in characters.
pub const MAX_AUSWAHL: usize = 64;

/// Coming back after a dropped connection: who the client was and the last sequence number it has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wieder {
    pub client: u64,
    pub seq: u64,
}

/// Client to server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum AnServer {
    /// Join the session of one part (`teil`); `wieder` when coming back.
    Hallo { protokoll: u32, teil: String, name: String, wieder: Option<Wieder> },
    /// One user action: the change set from the state with sequence number `basis`. `nr` counts
    /// the client's sets (it only grows), so a set sent twice is taken once. `gesehen` is the
    /// newest state the person had seen when they acted (at most `basis`): whoever changed the same
    /// thing after that gets a notice, also when the set was made again on a newer state.
    Aenderungen { basis: u64, gesehen: u64, nr: u64, satz: Aenderungssatz },
    /// What the person has selected (ephemeral, latest wins).
    Anwesenheit { auswahl: Vec<String> },
    /// Work in the sketch with this step id (lock it), or leave it (`None`).
    Skizze { skizze: Option<u64> },
}

/// Someone else in the session, as the others see them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Person {
    pub client: u64,
    pub name: String,
    pub auswahl: Vec<String>,
    /// The sketch this person works in (and has locked).
    pub skizze: Option<u64>,
}

/// An accepted change set, as everyone gets it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bestaetigt {
    /// The sequence number of the state this set leads to.
    pub seq: u64,
    pub autor: u64,
    pub autor_name: String,
    /// The author's own number of the set.
    pub nr: u64,
    /// The set as the server applied it: from the state `seq - 1` to the state `seq`.
    pub satz: Aenderungssatz,
}

/// What a joining client gets to catch up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Nachholen {
    /// The whole design as saved (a JSON tree).
    Stand { teil: Value },
    /// The sets missed since the sequence number the client came back with.
    Saetze { saetze: Vec<Bestaetigt> },
}

/// Server to client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum AnClient {
    /// Joined: the client's id, the current sequence number, what to catch up with, who else is
    /// there, and the highest set number of this client the server has dealt with (taken or
    /// refused), so a client coming back knows what became of the set it had on the way.
    Willkommen { client: u64, seq: u64, nachholen: Nachholen, personen: Vec<Person>, erledigt: u64 },
    /// An accepted set, to everyone (the author gets its own back as confirmation).
    Aenderungen(Bestaetigt),
    /// The sender's set `nr` was refused as a whole: why, and the current state to go back to.
    Abgelehnt { nr: u64, grund: String, seq: u64, teil: Value },
    /// A notice for this person (for example: someone changed the same thing after you).
    Hinweis { text: String },
    /// Someone joined or their presence changed.
    Person(Person),
    /// Someone left.
    Weg { client: u64 },
    /// The sketch could not be locked: someone else works in it.
    SkizzeBelegt { skizze: u64, von: String },
    /// The connection is refused.
    Abgewiesen { grund: String },
}

/// One message as a line of JSON (with the newline).
pub fn zeile<T: Serialize>(n: &T) -> Result<String, String> {
    let mut s = serde_json::to_string(n).map_err(|e| e.to_string())?;
    s.push('\n');
    Ok(s)
}

/// A message from a line of JSON (size-limited, unknown fields refused).
pub fn lies_zeile<T: for<'de> Deserialize<'de>>(z: &[u8]) -> Result<T, String> {
    if z.len() > MAX_ZEILE {
        return Err(format!("Nachricht größer als {} MiB", MAX_ZEILE >> 20));
    }
    serde_json::from_slice(z).map_err(|e| format!("Nachricht nicht lesbar: {e}"))
}

/// `s` cut to `max` characters, control characters removed.
pub fn gesaeubert(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_owned()
}
