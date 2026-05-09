//! TypeScript / TSX extractor — emits `Symbol` and `Edge` records via
//! tree-sitter queries. All edges carry `EdgeSource::TreeSitter`; LSP
//! refines them later (see `mycel-lsp`).

use std::collections::BTreeMap;

use camino::Utf8Path;
use mycel_core::*;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

use crate::{Extractor, ExtractionOutput};

pub struct TypeScriptExtractor {
    ts_lang: Language,
    tsx_lang: Language,
}

impl Default for TypeScriptExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeScriptExtractor {
    pub fn new() -> Self {
        Self {
            ts_lang: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            tsx_lang: tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    fn language_for(&self, path: &Utf8Path) -> Language {
        match path.extension() {
            Some("tsx") | Some("jsx") => self.tsx_lang.clone(),
            _ => self.ts_lang.clone(),
        }
    }
}

impl Extractor for TypeScriptExtractor {
    fn language_name(&self) -> &'static str {
        "typescript"
    }

    fn extract(&self, file: &Utf8Path, content: &str) -> Result<ExtractionOutput> {
        let mut parser = Parser::new();
        let lang = self.language_for(file);
        parser
            .set_language(&lang)
            .map_err(|e| MycelError::Extract {
                file: file.to_string(),
                message: format!("set_language: {e}"),
            })?;
        let tree: Tree = parser
            .parse(content, None)
            .ok_or_else(|| MycelError::Extract {
                file: file.to_string(),
                message: "parse failed".into(),
            })?;

        // Dedup key (start_line, qualified_name): merges the two query rows that
        // fire on a single exported declaration (the bare pattern and the
        // `export_statement` wrapper) since they share start_line and name. Same
        // name at different lines (overloads) intentionally stay distinct. When
        // both bare and exported patterns match, `exported = true` wins.
        let mut symbol_map: BTreeMap<(u32, String), Symbol> = BTreeMap::new();
        let mut edges = Vec::new();
        extract_symbols_and_edges(&lang, &tree, content, file, &mut symbol_map, &mut edges)?;

        let symbols = symbol_map.into_values().collect();
        Ok(ExtractionOutput {
            symbols,
            edges,
            language: "typescript".into(),
        })
    }
}

fn extract_symbols_and_edges(
    lang: &Language,
    tree: &Tree,
    src: &str,
    file: &Utf8Path,
    symbols: &mut BTreeMap<(u32, String), Symbol>,
    edges: &mut Vec<Edge>,
) -> Result<()> {
    // Symbols
    let q = Query::new(lang, SYMBOLS_QUERY).map_err(|e| MycelError::Extract {
        file: file.to_string(),
        message: format!("ts query: {e}"),
    })?;
    let mut cursor = QueryCursor::new();
    let bytes = src.as_bytes();
    let mut matches = cursor.matches(&q, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        let mut name: Option<&str> = None;
        let mut kind: Option<SymbolKind> = None;
        let mut start_node: Option<Node> = None;
        let mut end_node: Option<Node> = None;
        let mut signature_node: Option<Node> = None;
        let mut exported = false;
        for c in m.captures {
            let cap = q.capture_names()[c.index as usize];
            match cap {
                "fn.name" => {
                    name = Some(c.node.utf8_text(bytes).unwrap_or(""));
                    kind = Some(SymbolKind::Function);
                }
                "method.name" => {
                    name = Some(c.node.utf8_text(bytes).unwrap_or(""));
                    kind = Some(SymbolKind::Method);
                }
                "class.name" => {
                    name = Some(c.node.utf8_text(bytes).unwrap_or(""));
                    kind = Some(SymbolKind::Class);
                }
                "interface.name" => {
                    name = Some(c.node.utf8_text(bytes).unwrap_or(""));
                    kind = Some(SymbolKind::Interface);
                }
                "type.name" => {
                    name = Some(c.node.utf8_text(bytes).unwrap_or(""));
                    kind = Some(SymbolKind::Type);
                }
                "fn.def" | "method.def" | "class.def" | "interface.def" | "type.def" => {
                    start_node = Some(c.node);
                    end_node = Some(c.node);
                    signature_node = Some(c.node);
                }
                "exported" => exported = true,
                _ => {}
            }
        }
        if let (Some(name), Some(kind), Some(start), Some(end)) =
            (name, kind, start_node, end_node)
        {
            let sig_text = signature_node
                .and_then(|n| signature_first_line(n, src))
                .unwrap_or_else(|| name.into());
            let start_line = start.start_position().row as u32 + 1;
            let end_line = end.end_position().row as u32 + 1;
            let qn = format!("{}::{}", file.as_str(), name);
            let key = (start_line, qn.clone());
            // If the same symbol was matched by both a bare and an export
            // pattern, prefer the entry where exported=true.
            symbols
                .entry(key)
                .and_modify(|s| {
                    if exported {
                        s.exported = true;
                    }
                })
                .or_insert(Symbol {
                    qualified_name: QualifiedName::new(qn),
                    kind,
                    file_path: file.to_path_buf(),
                    start_line,
                    end_line,
                    signature: Signature::new(sig_text),
                    jsdoc: None,
                    synthesized_description: None,
                    exported,
                    embedding: None,
                    body_hash: None,
                    description_source_hash: None,
                });
        }
    }

    // Calls
    let q_calls = Query::new(lang, CALLS_QUERY).map_err(|e| MycelError::Extract {
        file: file.to_string(),
        message: format!("ts calls: {e}"),
    })?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&q_calls, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        let mut callee_node: Option<Node> = None;
        let mut site_node: Option<Node> = None;
        for c in m.captures {
            match q_calls.capture_names()[c.index as usize] {
                "callee" => callee_node = Some(c.node),
                "site" => site_node = Some(c.node),
                _ => {}
            }
        }
        let (Some(callee), Some(site)) = (callee_node, site_node) else {
            continue;
        };
        let callee_name = callee.utf8_text(bytes).unwrap_or("");
        if callee_name.is_empty() {
            continue;
        }
        // Skip calls without an enclosing named decl (top-level expressions).
        // The graph layer's upsert_edge_batch MATCHes both endpoints; an edge whose
        // `from` doesn't correspond to an existing Symbol is silently dropped, so a
        // `<file>` sentinel would produce no observable behavior. Top-level call
        // edges are a Phase 4 LSP-refinement concern.
        let Some(caller) = enclosing_named_decl(site, bytes) else {
            continue;
        };
        edges.push(Edge {
            from: format!("{}::{}", file.as_str(), caller),
            to: callee_name.into(),
            kind: EdgeKind::Calls,
            source: EdgeSource::TreeSitter,
        });
    }

    // Imports
    let q_imports = Query::new(lang, IMPORTS_QUERY).map_err(|e| MycelError::Extract {
        file: file.to_string(),
        message: format!("ts imports: {e}"),
    })?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&q_imports, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        for c in m.captures {
            if q_imports.capture_names()[c.index as usize] == "import.source" {
                let src_text = c
                    .node
                    .utf8_text(bytes)
                    .unwrap_or("")
                    .trim_matches('"')
                    .trim_matches('\'');
                edges.push(Edge {
                    from: file.as_str().into(),
                    to: src_text.into(),
                    kind: EdgeKind::Imports,
                    source: EdgeSource::TreeSitter,
                });
            }
        }
    }

    Ok(())
}

fn enclosing_named_decl<'a>(node: Node, bytes: &'a [u8]) -> Option<&'a str> {
    let mut cur = node;
    while let Some(parent) = cur.parent() {
        match parent.kind() {
            "function_declaration" | "function" | "method_definition" | "arrow_function" => {
                if let Some(name_node) = parent.child_by_field_name("name") {
                    return name_node.utf8_text(bytes).ok();
                }
                return Some("<anonymous>");
            }
            // Function-arms always return (named or anonymous fallback). The class
            // arm only returns when the class has a name; an unnamed class (rare:
            // `function f() { return class {}; }`) falls through to keep walking
            // parents, which (correctly) ends up returning the enclosing `f`.
            "class_declaration" => {
                if let Some(name_node) = parent.child_by_field_name("name") {
                    return name_node.utf8_text(bytes).ok();
                }
            }
            _ => {}
        }
        cur = parent;
    }
    None
}

fn signature_first_line(n: Node, src: &str) -> Option<String> {
    let text = n.utf8_text(src.as_bytes()).ok()?;
    text.lines().next().map(|s| s.trim().to_string())
}

/// Phase 1 SYMBOLS_QUERY captures the structural declarations we can resolve
/// without LSP: function/class/interface/type/method declarations and their
/// `export_statement`-wrapped variants. Out of scope (deferred to Phase 4 LSP
/// refinement):
/// - `export const Foo = () => {...}` (arrow-bound consts)
/// - `export const x = function() {...}` (function-expression-bound consts)
/// - `export default function/class` (default exports)
/// - `enum`, `namespace`, `module` declarations
/// - Generator and async-arrow specifics
///
/// Real TS codebases use these heavily; the Indexer (Task 6.1) will under-count
/// symbols on real repos until LSP fills these in.
///
/// Note: `method_definition` matches both class methods AND object-literal
/// method-shorthand (e.g. `const obj = { greet(name) {...} }`). Both are
/// captured as SymbolKind::Method. LSP refinement may re-classify
/// object-literal shorthands later.
const SYMBOLS_QUERY: &str = include_str!("typescript_queries/symbols.scm");
const CALLS_QUERY: &str = include_str!("typescript_queries/calls.scm");
const IMPORTS_QUERY: &str = include_str!("typescript_queries/imports.scm");
