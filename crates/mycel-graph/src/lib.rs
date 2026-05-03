//! mycel-graph — sole owner of FalkorDB Cypher.

pub mod client;
pub(crate) mod cypher;
pub mod migrations;

pub use client::GraphClient;
