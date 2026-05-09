//! mycel-index — Phase 1 indexing pipeline.
//!
//! Orchestrates: extractor (tree-sitter) → LSP refinement (multilspy) →
//! graph upsert (mycel-graph) → embed signatures (mycel-models) → File
//! record write. Content-hash dedup short-circuits unchanged files.

pub mod body_slice;
pub mod dedup;
pub mod pipeline;
pub mod synthesize;

pub use pipeline::Indexer;
pub use synthesize::{SynthesisOptions, SynthesisOutcome, synthesize_descriptions};
