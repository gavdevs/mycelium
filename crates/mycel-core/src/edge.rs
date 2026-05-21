use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Defines,
    Calls,
    Imports,
    Extends,
    Implements,
    UsesType,
    References,
    ReExports,
    CoChanged,
    TestedBy,
    ModifiedIn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeSource {
    TreeSitter,
    Lsp,
    Derived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub source: EdgeSource,
    /// 1-indexed line number in the `from` symbol's source file at which the
    /// call/use/implements site appears. Populated by tree-sitter extractors
    /// for CALLS / USES_TYPE / IMPLEMENTS edges so the LSP refinement layer
    /// can hover that line to resolve the `to` endpoint to a qualified name
    /// across file boundaries. `None` for derived or pre-resolution edges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_line: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_default_has_no_from_line() {
        let e = Edge {
            from: "a".into(),
            to: "b".into(),
            kind: EdgeKind::Calls,
            source: EdgeSource::TreeSitter,
            from_line: None,
        };
        assert!(e.from_line.is_none());
    }

    #[test]
    fn edge_with_from_line_carries_coord() {
        let e = Edge {
            from: "a".into(),
            to: "b".into(),
            kind: EdgeKind::Calls,
            source: EdgeSource::TreeSitter,
            from_line: Some(42),
        };
        assert_eq!(e.from_line, Some(42));
    }
}
