//! Rust extractor — emits `Symbol` and `Edge` records via tree-sitter
//! queries. All edges carry `EdgeSource::TreeSitter`; LSP refines them
//! later (see `mycel-lsp`).

use camino::Utf8Path;
use mycel_core::*;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

use crate::{Extractor, ExtractionOutput};

pub struct RustExtractor {
    lang: Language,
}

impl Default for RustExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl RustExtractor {
    pub fn new() -> Self {
        Self {
            lang: tree_sitter_rust::LANGUAGE.into(),
        }
    }
}

impl Extractor for RustExtractor {
    fn language_name(&self) -> &'static str {
        "rust"
    }

    fn extract(&self, file: &Utf8Path, content: &str) -> Result<ExtractionOutput> {
        let mut parser = Parser::new();
        parser
            .set_language(&self.lang)
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
        let mut symbols = Vec::new();
        let mut edges = Vec::new();
        extract_rust(&self.lang, &tree, content, file, &mut symbols, &mut edges)?;
        Ok(ExtractionOutput {
            symbols,
            edges,
            language: "rust".into(),
        })
    }
}

fn extract_rust(
    lang: &Language,
    tree: &Tree,
    src: &str,
    file: &Utf8Path,
    symbols: &mut Vec<Symbol>,
    edges: &mut Vec<Edge>,
) -> Result<()> {
    let bytes = src.as_bytes();

    // Symbols
    let q = Query::new(lang, RUST_SYMBOLS).map_err(|e| MycelError::Extract {
        file: file.to_string(),
        message: format!("rs query: {e}"),
    })?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&q, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        let mut name: Option<&str> = None;
        let mut kind: Option<SymbolKind> = None;
        let mut def_node: Option<Node> = None;
        for c in m.captures {
            match q.capture_names()[c.index as usize] {
                "fn.name" => {
                    name = node_text(c.node, bytes);
                    kind = Some(SymbolKind::Function);
                }
                "struct.name" => {
                    name = node_text(c.node, bytes);
                    kind = Some(SymbolKind::Struct);
                }
                "enum.name" => {
                    name = node_text(c.node, bytes);
                    kind = Some(SymbolKind::Enum);
                }
                "trait.name" => {
                    name = node_text(c.node, bytes);
                    kind = Some(SymbolKind::Trait);
                }
                "type.name" => {
                    name = node_text(c.node, bytes);
                    kind = Some(SymbolKind::Type);
                }
                "const.name" => {
                    name = node_text(c.node, bytes);
                    kind = Some(SymbolKind::Constant);
                }
                "static.name" => {
                    name = node_text(c.node, bytes);
                    kind = Some(SymbolKind::Static);
                }
                "mod.name" => {
                    name = node_text(c.node, bytes);
                    kind = Some(SymbolKind::Module);
                }
                "fn.def" | "struct.def" | "enum.def" | "trait.def" | "type.def"
                | "const.def" | "static.def" | "mod.def" => def_node = Some(c.node),
                _ => {}
            }
        }
        // Detect `pub` visibility by walking the def_node's children for a visibility_modifier.
        let visible = def_node.map(has_pub_visibility).unwrap_or(false);
        if let (Some(name), Some(kind), Some(def)) = (name, kind, def_node) {
            let signature = def
                .utf8_text(bytes)
                .ok()
                .and_then(|t| t.lines().next().map(|l| l.trim().to_string()))
                .unwrap_or_else(|| name.into());
            symbols.push(Symbol {
                qualified_name: QualifiedName::new(format!("{}::{}", file.as_str(), name)),
                kind,
                file_path: file.to_path_buf(),
                start_line: def.start_position().row as u32 + 1,
                end_line: def.end_position().row as u32 + 1,
                signature: Signature::new(signature),
                jsdoc: None,
                synthesized_description: None,
                exported: visible,
                embedding: None,
                body_hash: None,
                description_source_hash: None,
            });
        }
    }

    // Impl blocks → IMPLEMENTS edges
    let q_impl = Query::new(lang, RUST_IMPLS).map_err(|e| MycelError::Extract {
        file: file.to_string(),
        message: format!("rs impls: {e}"),
    })?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&q_impl, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        let mut trait_name: Option<&str> = None;
        let mut type_name: Option<&str> = None;
        for c in m.captures {
            match q_impl.capture_names()[c.index as usize] {
                "trait" => trait_name = node_text(c.node, bytes),
                "ty" => type_name = node_text(c.node, bytes),
                _ => {}
            }
        }
        if let (Some(trait_name), Some(type_name)) = (trait_name, type_name) {
            edges.push(Edge {
                from: format!("{}::{}", file.as_str(), type_name),
                to: trait_name.into(),
                kind: EdgeKind::Implements,
                source: EdgeSource::TreeSitter,
                from_line: None,
            });
        }
    }

    // use statements → IMPORTS edges
    let q_use = Query::new(lang, RUST_USES).map_err(|e| MycelError::Extract {
        file: file.to_string(),
        message: format!("rs uses: {e}"),
    })?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&q_use, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        for c in m.captures {
            if q_use.capture_names()[c.index as usize] == "use.path" {
                let path = node_text(c.node, bytes).unwrap_or("");
                edges.push(Edge {
                    from: file.as_str().into(),
                    to: path.into(),
                    kind: EdgeKind::Imports,
                    source: EdgeSource::TreeSitter,
                    from_line: None,
                });
            }
        }
    }

    // Calls — resolve enclosing function as caller (heuristic; LSP refines later).
    // Skip calls without an enclosing fn (top-level expressions) to avoid producing
    // edges that the graph layer will silently drop. (Same pattern as TS extractor.)
    let q_call = Query::new(lang, RUST_CALLS).map_err(|e| MycelError::Extract {
        file: file.to_string(),
        message: format!("rs calls: {e}"),
    })?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&q_call, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        for c in m.captures {
            if q_call.capture_names()[c.index as usize] == "callee" {
                let callee = node_text(c.node, bytes).unwrap_or("");
                if callee.is_empty() {
                    continue;
                }
                let Some(caller) = enclosing_fn(c.node, bytes) else {
                    continue;
                };
                edges.push(Edge {
                    from: format!("{}::{}", file.as_str(), caller),
                    to: callee.into(),
                    kind: EdgeKind::Calls,
                    source: EdgeSource::TreeSitter,
                    from_line: None,
                });
            }
        }
    }

    Ok(())
}

fn enclosing_fn<'a>(node: Node, bytes: &'a [u8]) -> Option<&'a str> {
    let mut cur = node;
    while let Some(parent) = cur.parent() {
        if matches!(parent.kind(), "function_item" | "function_signature_item") {
            if let Some(name) = parent.child_by_field_name("name") {
                return name.utf8_text(bytes).ok();
            }
        }
        cur = parent;
    }
    None
}

/// Returns true if `node`'s direct children include a `visibility_modifier`
/// containing `pub`. Adapt if tree-sitter-rust's grammar differs.
fn has_pub_visibility(node: Node) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            return true;
        }
    }
    false
}

fn node_text<'s>(n: Node, src: &'s [u8]) -> Option<&'s str> {
    n.utf8_text(src).ok()
}

const RUST_SYMBOLS: &str = include_str!("rust_queries/symbols.scm");
const RUST_IMPLS: &str = include_str!("rust_queries/impls.scm");
const RUST_USES: &str = include_str!("rust_queries/uses.scm");
const RUST_CALLS: &str = include_str!("rust_queries/calls.scm");
