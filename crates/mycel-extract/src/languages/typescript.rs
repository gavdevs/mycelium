//! TypeScript extractor — Phase 1 stub. Real implementation in Task 3.2.

use camino::Utf8Path;
use mycel_core::*;

use crate::{Extractor, ExtractionOutput};

pub struct TypeScriptExtractor;

impl Default for TypeScriptExtractor { fn default() -> Self { Self::new() } }

impl TypeScriptExtractor {
    pub fn new() -> Self { Self }
}

impl Extractor for TypeScriptExtractor {
    fn language_name(&self) -> &'static str { "typescript" }

    fn extract(&self, _file: &Utf8Path, _content: &str) -> Result<ExtractionOutput> {
        // Phase 1 stub — Task 3.2 implements real tree-sitter extraction.
        Ok(ExtractionOutput { symbols: vec![], edges: vec![], language: "typescript".into() })
    }
}
