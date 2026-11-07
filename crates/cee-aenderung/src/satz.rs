//! The change set (Änderungssatz): what turns one state of a design into another.
//!
//! The wire form is JSON. Every change carries the value it replaces (`vorher`), so a change set
//! can be checked against the design it is applied to, read aloud without the design, and turned
//! back. The set names the state it starts from (`ausgangsstand`) and the state it leads to
//! (`zielstand`) by their fingerprints ([`crate::fingerabdruck`]).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The format version this build writes and reads.
pub const VERSION: u32 = 1;

/// A change set between two states of one design.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aenderungssatz {
    /// Format version ([`VERSION`]).
    pub version: u32,
    /// Fingerprint of the design the set applies to.
    pub ausgangsstand: String,
    /// Fingerprint of the design applying the set gives.
    pub zielstand: String,
    /// The changes, in the order they are applied.
    pub aenderungen: Vec<Aenderung>,
}

/// One change. Positions in ordered lists are given by the predecessor's key (`nach`); `None`
/// means "at the start".
///
/// Paths (`pfad`, `liste`) are JSON pointers (RFC 6901): relative to the document for
/// [`Aenderung::Eigenschaft`], relative to the feature for the feature changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "art", rename_all = "snake_case", deny_unknown_fields)]
pub enum Aenderung {
    /// A parameter was added (the whole parameter as saved: name, expression, unit, …).
    GroesseNeu { name: String, nach: Option<String>, groesse: Value },
    /// A parameter changed (expression, unit, comment …): the whole parameter before and after.
    GroesseGesetzt { name: String, vorher: Value, nachher: Value },
    /// A parameter was deleted.
    GroesseGeloescht { name: String, vorher: Value },
    /// A parameter moved within the parameter list.
    GroesseVerschoben { name: String, nach: Option<String> },
    /// A timeline step was inserted (the whole feature as saved).
    SchrittEingefuegt { id: u64, nach: Option<u64>, schritt: Value },
    /// A timeline step was deleted.
    SchrittGeloescht { id: u64, vorher: Value },
    /// A timeline step moved.
    SchrittVerschoben { id: u64, nach: Option<u64> },
    /// A timeline step was renamed.
    SchrittUmbenannt { id: u64, vorher: String, nachher: String },
    /// A timeline step was suppressed or brought back.
    SchrittUnterdrueckt { id: u64, vorher: bool, nachher: bool },
    /// Any other property of a timeline step (`None`: absent).
    SchrittGeaendert { id: u64, pfad: String, vorher: Option<Value>, nachher: Option<Value> },
    /// An item (point, curve, constraint …) was added to a keyed list of a step.
    ElementHinzu { schritt: u64, liste: String, id: String, nach: Option<String>, element: Value },
    /// An item was removed from a keyed list of a step.
    ElementWeg { schritt: u64, liste: String, id: String, vorher: Value },
    /// An item of a keyed list of a step changed: the whole item before and after.
    ElementGeaendert { schritt: u64, liste: String, id: String, vorher: Value, nachher: Value },
    /// An item moved within its keyed list.
    ElementVerschoben { schritt: u64, liste: String, id: String, nach: Option<String> },
    /// Any other property of the design ("property at path is now value"; `None`: absent).
    Eigenschaft { pfad: String, vorher: Option<Value>, nachher: Option<Value> },
}
