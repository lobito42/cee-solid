//! One entry of a history: a change set with where it comes from.
//!
//! An entry names the entries it builds on (`vorgaenger`: none for the first, one for a saved
//! change, two for a merge: first the draft merged into, then the one merged in), who made it
//! (a name for now, an account id later), optionally when, and the change set itself, which names
//! the state it starts from and the state it leads to by their fingerprints. A merge entry also
//! keeps what was decided and what the checker found.

use cee_aenderung::Aenderungssatz;
use serde::{Deserialize, Serialize};

/// The format version of an entry.
pub const VERSION: u32 = 1;

/// One entry of the history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Eintrag {
    pub version: u32,
    /// The entries this one builds on (hashes); the first is the one the change set applies to.
    pub vorgaenger: Vec<String>,
    /// Who made it.
    pub autor: String,
    /// When (seconds since 1970, UTC), if it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zeit: Option<u64>,
    /// What it changes, from the state of the first predecessor (or an empty design).
    pub satz: Aenderungssatz,
    /// For a merge: what came together, what was decided, what the checker found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zusammenfuehrung: Option<Zusammenfuehrung>,
}

/// What a merge entry keeps besides its change set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Zusammenfuehrung {
    /// The draft merged in.
    pub ein: String,
    /// The draft merged into.
    pub nach: String,
    /// The conflicts and which side was taken, in words.
    pub entscheidungen: Vec<String>,
    /// Whether the checker found nothing broken.
    pub pruefer_ok: bool,
    /// The checker's report, line by line.
    pub pruefer: Vec<String>,
}
