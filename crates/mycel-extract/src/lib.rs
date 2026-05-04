//! Per-language tree-sitter extractors. All edges produced here carry
//! `EdgeSource::TreeSitter`; LSP refinement (mycel-lsp) upgrades them later.

use camino::Utf8Path;
use mycel_core::*;

pub mod languages;

#[derive(Debug, Clone, Default)]
pub struct ExtractionOutput {
    pub symbols: Vec<Symbol>,
    pub edges: Vec<Edge>,
    pub language: String,
}

pub trait Extractor: Send + Sync {
    fn language_name(&self) -> &'static str;
    fn extract(&self, file: &Utf8Path, content: &str) -> Result<ExtractionOutput>;
}

/// Returns the right extractor for a given file extension, or None if
/// no Phase 1 extractor handles it.
pub fn for_language(file: &Utf8Path) -> Option<Box<dyn Extractor>> {
    match file.extension()? {
        "ts" | "tsx" | "js" | "jsx" | "mts" | "cts" =>
            Some(Box::new(languages::typescript::TypeScriptExtractor::new())),
        "rs" =>
            Some(Box::new(languages::rust::RustExtractor::new())),
        _ => None,
    }
}
