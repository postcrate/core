//! Scenario inspectors that produce diagnostics about a captured
//! email *after* it's been parsed. None of these touch the network
//! (NFR-PRIV-01) — they're heuristics over the headers + bodies the
//! engine already has on disk.
//!
//! Each inspector returns a small struct so callers can choose
//! which to surface in the UI and how. The HTTP layer exposes them
//! under `/api/v1/messages/:id/scenarios/{spam,links,auth}`.

pub mod auth;
pub mod links;
pub mod list_unsub;
pub mod spam;
