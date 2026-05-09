use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QualifiedName(String);

impl QualifiedName {
    /// Constructs a QualifiedName, replacing any backtick with `_` to avoid
    /// breaking Cypher identifier-delimited string literals downstream.
    /// (Symbols with backticks in their name are vanishingly rare in practice.)
    pub fn new(s: impl Into<String>) -> Self {
        let s = s.into();
        if s.contains('`') {
            Self(s.replace('`', "_"))
        } else {
            Self(s)
        }
    }
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Signature(pub String);

impl Signature {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Interface,
    Type,
    Component,
    Hook,
    Constant,
    Module,
    Struct,
    Enum,
    Trait,
    Impl,
    Static,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    pub qualified_name: QualifiedName,
    pub kind: SymbolKind,
    pub file_path: Utf8PathBuf,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: Signature,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jsdoc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthesized_description: Option<String>,
    pub exported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    /// blake3 hex of the `signature + body slice` text used for the current
    /// embedding. Set on every cold-index/incremental pass. NULL only on
    /// legacy graphs predating the 2026-05-05 redirection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_hash: Option<String>,
    /// The `body_hash` value at the time `synthesized_description` was last
    /// written. NULL until a description is written. Compared against
    /// `body_hash` on incremental updates to detect description staleness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_source_hash: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_serializes_with_new_fields() {
        let sym = Symbol {
            qualified_name: QualifiedName::new("a::b"),
            kind: SymbolKind::Function,
            file_path: "src/a.rs".into(),
            start_line: 1,
            end_line: 3,
            signature: Signature::new("fn b()"),
            jsdoc: None,
            synthesized_description: None,
            exported: true,
            embedding: None,
            body_hash: Some("deadbeef".into()),
            description_source_hash: Some("cafebabe".into()),
        };
        let json = serde_json::to_string(&sym).unwrap();
        assert!(json.contains("body_hash"));
        assert!(json.contains("description_source_hash"));
        let round: Symbol = serde_json::from_str(&json).unwrap();
        assert_eq!(round.body_hash.as_deref(), Some("deadbeef"));
        assert_eq!(round.description_source_hash.as_deref(), Some("cafebabe"));
    }

    #[test]
    fn symbol_deserializes_legacy_shape_without_new_fields() {
        let legacy = r#"{
            "qualified_name": "a::b",
            "kind": "function",
            "file_path": "src/a.rs",
            "start_line": 1,
            "end_line": 3,
            "signature": "fn b()",
            "exported": true
        }"#;
        let s: Symbol = serde_json::from_str(legacy).unwrap();
        assert!(s.body_hash.is_none());
        assert!(s.description_source_hash.is_none());
    }
}
