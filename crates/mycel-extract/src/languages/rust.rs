//! Rust extractor — Phase 1 stub. Real implementation in Task 3.3.

use camino::Utf8Path;
use mycel_core::*;

use crate::{Extractor, ExtractionOutput};

pub struct RustExtractor;

impl Default for RustExtractor { fn default() -> Self { Self::new() } }

impl RustExtractor {
    pub fn new() -> Self { Self }
}

impl Extractor for RustExtractor {
    fn language_name(&self) -> &'static str { "rust" }

    fn extract(&self, _file: &Utf8Path, _content: &str) -> Result<ExtractionOutput> {
        // Phase 1 stub — Task 3.3 implements real tree-sitter extraction.
        Ok(ExtractionOutput { symbols: vec![], edges: vec![], language: "rust".into() })
    }
}
