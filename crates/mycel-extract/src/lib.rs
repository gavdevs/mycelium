//! Per-language tree-sitter extractors. All edges produced here carry
//! `EdgeSource::TreeSitter`; LSP refinement (mycel-lsp) upgrades them later.

use camino::Utf8Path;
use mycel_core::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod languages;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtractionOutput {
    pub symbols: Vec<Symbol>,
    pub edges: Vec<Edge>,
    pub language: String,
}

pub trait Extractor: Send + Sync {
    fn language_name(&self) -> &'static str;
    fn extract(&self, file: &Utf8Path, content: &str) -> Result<ExtractionOutput>;
}

/// Rewrite tree-sitter edges that reference a bare callee name to the full
/// `<file>::<name>` qualified form when a same-file symbol matches.
///
/// Tree-sitter has no symbol table — extractors emit edges like
/// `from = "<file>::caller", to = "callee"`. The graph upsert MATCHes both
/// endpoints by qualified_name, so any edge whose `to` is bare gets silently
/// dropped (no Symbol exists with `qualified_name = "callee"`). This pass
/// resolves the easy case — same-file calls — without needing LSP. Cross-file
/// resolution is still LSP territory and will be tackled in Phase 2/3 once
/// LSP refinement produces canonical qnames.
///
/// Idempotent: edges whose `to` is already qualified (contains `::` or a path
/// separator) are left alone.
pub fn resolve_same_file_edges(file: &Utf8Path, edges: &mut [Edge], symbols: &[Symbol]) {
    // name -> qualified_name, for symbols defined in this file
    let mut by_name: HashMap<&str, &str> = HashMap::with_capacity(symbols.len());
    for s in symbols {
        // qname format: "<file>::<short>"; the short is what tree-sitter emits as `to`
        if let Some((_, short)) = s.qualified_name.as_str().rsplit_once("::") {
            by_name.insert(short, s.qualified_name.as_str());
        }
    }
    let _ = file; // reserved for future cross-file fallback heuristics
    for e in edges {
        // Only rewrite if `to` is a bare identifier with no qname separators.
        if e.to.contains("::") || e.to.contains('/') {
            continue;
        }
        if let Some(qname) = by_name.get(e.to.as_str()) {
            e.to = (*qname).to_string();
        }
    }
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
