//! CEE: working on one SolidCraft design together, live.
//!
//! A server orders the change sets ([`cee_aenderung`]) of everyone in a part's session and sends
//! each accepted set to all; where two people change the same thing at once, the later one wins
//! and the other gets a notice. A sketch is locked while someone works in it; parameters, steps
//! and different sketches can be changed at the same time. Presence (names, selections, who is in
//! which sketch) only travels live and is never stored. Undo takes back only one's own steps.
//! After a short loss of connection, the client sends what it did meanwhile. Geometry never
//! travels: every client computes it with SolidCraft's own engine.
//!
//! The parts are pure logic ([`server::Server`], [`client::Client`]); [`netz`] moves the messages
//! over TCP. The method follows FigCraft's collaboration (see ATTRIBUTION); no code is copied.

pub mod client;
pub mod netz;
pub mod protokoll;
pub mod server;
pub mod vereinen;

pub use client::Client;
pub use server::Server;
